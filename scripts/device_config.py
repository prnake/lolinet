# /// script
# requires-python = ">=3.11"
# dependencies = ["pyyaml"]
# ///
"""渲染终端设备的 EasyTier 配置（凭据入网，不含 network_secret）。

凭据由 issuer relay / 控制台签发；这里只负责拼配置。peers 走 wss 443，URL 带 ?sni=<network.sni>
（打了 SNI 补丁的客户端才认）。peer 地址默认直接写 relay 的 EIP（--by-name 改用 <relay>.<edge_zone>）：
本机所在网络的解析器会把域名换成 198.18.x 的 fake-IP；SNI 由 ?sni= 决定，和 URL 里写 IP 还是域名无关。

用法: uv run scripts/device_config.py --name <设备> --out <文件>
      （读节点清单的 devices 和 $LOLINET_HOME/state/mesh-cred-<name>-mac.json）
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

import yaml

MESH_DIR = Path(__file__).resolve().parent.parent
# 运行时数据（.env、state/）：LOLINET_HOME，兼容旧的 FLEET_REPO，默认 ~/.config/lolinet
HOME_DIR = Path(os.environ.get("LOLINET_HOME") or os.environ.get("FLEET_REPO") or Path.home() / ".config/lolinet")
STATE_DIR = HOME_DIR / "state"


def nodes_path() -> Path:
    """节点清单：LOLINET_NODES，否则仓库根的 nodes.yaml，没有就用 nodes.example.yaml（公开仓库只有这份）。"""
    if os.environ.get("LOLINET_NODES"):
        return Path(os.environ["LOLINET_NODES"])
    p = MESH_DIR / "nodes.yaml"
    return p if p.is_file() else MESH_DIR / "nodes.example.yaml"


NODES = yaml.safe_load(nodes_path().read_text())
NET = NODES["network"]
# 准入站地址：LOLINET_BASE_URL，不设就是 https://<network.join>
BASE_URL = (os.environ.get("LOLINET_BASE_URL") or f"https://{NET['join']}").rstrip("/")
# 设备默认连的 relay：network.device_peers，没写就取前三台
DEFAULT_PEERS = NET.get("device_peers") or [r["name"] for r in NODES["relays"][:3]]


def q(s: str) -> str:
    return json.dumps(s)


def network() -> dict:
    return NET


def relays() -> dict[str, dict]:
    out = {}
    for f in STATE_DIR.glob("provision-*.json"):
        d = json.loads(f.read_text())
        if d.get("role") == "mesh-relay" and d.get("eip"):
            out[d["name"]] = d
    return out


def render(name: str, overlay_ip: str, credential_secret: str, peers: list[str] | None = None,
           by_name: bool = False, transport: str = "wss") -> str:
    """transport=tcp 连 relay 的 EasyTier tcp 端口，给没有 SNI 补丁的官方二进制 / Docker 镜像用（内地家宽直连内地 relay 没问题）。"""
    net = network()
    rs = relays()
    lines = [
        f"hostname = {q(name)}",
        f"instance_name = {q(net['name'])}",
        f"ipv4 = {q(overlay_ip + '/16')}",
        "dhcp = false",
        "listeners = []",
        "",
        "[network_identity]",
        f"network_name = {q(net['name'])}",
        "",
        "[secure_mode]",
        "enabled = true",
        f"local_private_key = {q(credential_secret)}",
    ]
    for p in peers or DEFAULT_PEERS:
        host = f"{p}.{net['edge_zone']}" if by_name else rs[p]["eip"]
        peer_uri = f"tcp://{host}:{rs[p]['et_port']}" if transport == "tcp" else f"wss://{host}:443/?sni={net['sni']}"
        lines += ["", "[[peer]]", f"# {p}.{net['edge_zone']}", f"uri = {q(peer_uri)}"]
        # 再连一条 udp：lolinet 的隧道偏好补丁让内层 UDP（lolidesk 的 QUIC 等）走 UDP 隧道，没有队头阻塞，
        # KCP 代理包仍走上面的 wss。udp 被运营商挡了就退回 wss。transport=tcp 是给官方二进制用的，
        # 它们没有这个偏好，会按延迟把 KCP 也放到 udp 上（实测 KCP 套 UDP 隧道上传会崩），所以不加
        if transport == "wss":
            udp_uri = f"udp://{host}:{rs[p]['et_port']}"
            lines += ["", "[[peer]]", f"# {p}.{net['edge_zone']} udp", f"uri = {q(udp_uri)}"]
    # 终端设备开延迟优先：默认「最少跳数」有直连就直发，不看延迟；开了才会按全网延迟表经最近 relay 中转。relay 保持默认。
    # enable_kcp_proxy：TCP 流包进 KCP/UDP，绕开运营商对出境 TCP 的整形（2026-11-08 实测本机到香港下载 67 -> 150 Mbps）
    lines += ["", "[flags]", "private_mode = true", "latency_first = true", "enable_kcp_proxy = true", ""]
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--name", required=True)
    ap.add_argument("--cred", help="凭据 ID，默认 <name>-mac")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--by-name", action="store_true", help="peer 用 edge 域名而不是 EIP")
    args = ap.parse_args()
    doc = NODES
    dev = next(d for d in doc["devices"] if d["name"] == args.name)
    cred = json.loads((STATE_DIR / f"mesh-cred-{args.cred or args.name + '-mac'}.json").read_text())
    args.out.write_text(render(dev["name"], dev["overlay_ip"], cred["credential_secret"], dev.get("peers"), args.by_name,
                               dev.get("transport", "wss")))
    os.chmod(args.out, 0o600)
    print(f"{args.name}: {args.out}")


if __name__ == "__main__":
    main()
