# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx[socks]", "pyyaml", "cryptography"]
# ///
"""签发设备配对码：控制台签凭据 -> 渲染设备配置 -> 用配对码加密 -> 放进 KV（15 分钟、一次性）。

设备上执行：curl -fsSL <准入站>/i | sudo sh -s <配对码>（准入站 = LOLINET_BASE_URL，不设就是节点清单的 network.join）
默认连公开探针一起装：配置末尾加一行 `# probe = {...}`（设备公开 id + 上报密钥，probe/device.py 同一套派生），
lolinet up 把它存成 /etc/lolinet/probe.json、装成系统服务（lolinet 内置的探针），不留在 EasyTier 配置里；
--no-probe 不装，--location 写死公开的城市。

控制台只在 overlay 上（节点清单的 network.console，跑在 network.issuer 那台 relay 上）。本机要么已装 TUN 版，要么先起一个
no-tun 实例并用 --socks 指过去。连接直接打 issuer 的 overlay IP，SNI 和证书校验用控制台域名（本机解析器可能是 fake-IP）。
凭据明文只进加密后的配对包，不落盘；设备登记（不含密钥）追加到$LOLINET_HOME/state/mesh-devices.json。
--out 例外：配置直接写本地文件，给装不了 join 脚本的设备（Docker 版 EasyTier、没有 sudo 的 NAS）手工下发。

用法:
  uv run scripts/pair.py new --name laptop [--ip 10.188.16.11 | --pool userspace] [--ttl-days 365] [--socks 127.0.0.1:18080]
  uv run scripts/pair.py new --name nas --ip 10.188.1.10 --transport tcp --peers chen,hoshiguma,saria \
      --out /tmp/nas.toml --socks 127.0.0.1:18080
"""
from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import ipaddress
import json
import os
import secrets
import sys
from pathlib import Path
from urllib.parse import quote, urlsplit

import httpx
from cryptography.hazmat.primitives import hashes, padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

sys.path.insert(0, str(Path(__file__).resolve().parent))
from device_config import BASE_URL, DEFAULT_PEERS, HOME_DIR, NET, NODES, STATE_DIR, render  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "probe"))
from device import device_config as probe_config  # noqa: E402

# 控制台：域名只用来做 SNI 和证书校验，连接直接打签发 relay 的 overlay IP
CONSOLE_HOST = urlsplit(NET["console"]).hostname
CONSOLE_IP = next(r["overlay_ip"] for r in NODES["relays"] if r["name"] == NET["issuer"])
KV_TITLE = (NODES.get("cloudflare") or {}).get("kv", "lolinet-pairing")
PAIR_TTL = 900  # 默认 15 分钟；--code-minutes 可调
# 地址池：和 web/agent/mesh_manager.py 的 POOLS 一致，节点清单的 network.pools 可以改；手机走管理面的 WireGuard 入口，这里不签
POOLS = {"server": "10.188.1.0/24", "desktop": "10.188.16.0/20", "userspace": "10.188.32.0/20"}
CODE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"
CODE_LEN = 12
PBKDF2_ITER = 200000  # 和 join/install.sh 的 openssl -iter 一致


def load_env() -> None:
    """Cloudflare 凭据：环境变量 CLOUDFLARE_API_TOKEN / CLOUDFLARE_ACCOUNT_ID，或者 $LOLINET_HOME/.env 的 CLOUDFLARE_API / CLOUDFLARE_ID（和 join/deploy.py 一样）。"""
    for short, std in (("CLOUDFLARE_API", "CLOUDFLARE_API_TOKEN"), ("CLOUDFLARE_ID", "CLOUDFLARE_ACCOUNT_ID")):
        if os.environ.get(std):
            os.environ.setdefault(short, os.environ[std])
    env = HOME_DIR / ".env"
    if not env.is_file():
        return
    for line in env.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            os.environ.setdefault(k.strip(), v.strip().strip('"').strip("'"))


def encrypt(code: str, plain: bytes) -> str:
    """OpenSSL `enc -aes-256-cbc -pbkdf2 -iter N -md sha256 -a -A` 兼容格式。"""
    salt = os.urandom(8)
    km = PBKDF2HMAC(hashes.SHA256(), 48, salt, PBKDF2_ITER).derive(code.encode())
    p = padding.PKCS7(128).padder()
    data = p.update(plain) + p.finalize()
    e = Cipher(algorithms.AES(km[:32]), modes.CBC(km[32:])).encryptor()
    return base64.b64encode(b"Salted__" + salt + e.update(data) + e.finalize()).decode()


def console_credential(name: str, ttl_days: int, socks: str | None) -> tuple[str, str, int]:
    st = json.loads((STATE_DIR / "mesh-console.json").read_text())
    ext = {"sni_hostname": CONSOLE_HOST}
    with httpx.Client(base_url=f"https://{CONSOLE_IP}", headers={"Host": CONSOLE_HOST}, timeout=30,
                      proxy=f"socks5://{socks}" if socks else None) as c:
        r = c.post("/api/v1/auth/login", extensions=ext,
                   json={"username": st["admin_user"], "password": hashlib.md5(st["admin_pw"].encode()).hexdigest()})
        r.raise_for_status()
        cid = f"{name}-{dt.datetime.now().strftime('%Y%m%d%H%M%S')}"
        r = c.post(f"/api/v1/networks/{st['network_id']}/credentials", extensions=ext,
                   json={"ttl_seconds": ttl_days * 86400, "credential_id": cid, "reusable": False})
        r.raise_for_status()
        d = r.json()
        return d["credential_id"], d["credential_secret"], d["expiry_unix"]


def kv_put(key: str, value: str, ttl: int = PAIR_TTL) -> None:
    acct = f"https://api.cloudflare.com/client/v4/accounts/{os.environ['CLOUDFLARE_ID']}"
    h = {"Authorization": f"Bearer {os.environ['CLOUDFLARE_API']}"}
    with httpx.Client(headers=h, timeout=30) as c:
        ns = next(n for n in c.get(f"{acct}/storage/kv/namespaces?per_page=100").json()["result"] if n["title"] == KV_TITLE)
        r = c.put(f"{acct}/storage/kv/namespaces/{ns['id']}/values/{quote(key, safe=':')}?expiration_ttl={ttl}",
                  content=value.encode(), headers={"Content-Type": "text/plain"})
        if not r.json().get("success"):
            raise SystemExit(f"KV put failed: {r.text[:200]}")


def select_ip(name: str, requested: str | None, pool: str, taken: list[dict]) -> str:
    """签凭据前定地址。给了 --ip 就核对：在网段里、没分给别的设备。没给就沿用同名设备上次的地址（重新配对不换地址），
    再不然取地址池里第一个空位（末位 0 和 255 不用，和 mesh_manager.allocate 一样）。taken = relay + 节点清单的设备 + 配对登记。"""
    owners: dict[str, set[str]] = {}
    for d in taken:
        if d.get("overlay_ip"):
            owners.setdefault(str(d["overlay_ip"]), set()).add(d.get("name", ""))
    if requested:
        try:
            ip = ipaddress.ip_address(requested)
        except ValueError:
            ip = None
        if ip is None or ip not in ipaddress.ip_network(NET["cidr"]):
            raise SystemExit(f"--ip 要是 {NET['cidr']} 里的地址")
        others = owners.get(str(ip), set()) - {name}
        if others:
            raise SystemExit(f"--ip {ip} 已经分给了 {'、'.join(sorted(others))}")
        return str(ip)
    for d in reversed(taken):
        if d.get("name") == name and d.get("overlay_ip"):
            return str(d["overlay_ip"])
    net = ipaddress.ip_network({**POOLS, **(NET.get("pools") or {})}[pool])
    for ip in net.hosts():
        if ip.packed[3] not in (0, 255) and str(ip) not in owners:
            return str(ip)
    raise SystemExit(f"地址池 {pool}（{net}）已经用完")


def new(args) -> None:
    if not 1 <= args.code_minutes <= 1440:
        raise SystemExit("--code-minutes 要在 1–1440 之间")
    reg = STATE_DIR / "mesh-devices.json"
    devices = json.loads(reg.read_text()) if reg.exists() else []
    args.ip = select_ip(args.name, args.ip, args.pool, list(NODES.get("relays") or []) + list(NODES.get("devices") or []) + devices)
    load_env()
    cid, secret, expiry = console_credential(args.name, args.ttl_days, args.socks)
    config = render(args.name, args.ip, secret, args.peers.split(",") if args.peers else None, transport=args.transport)
    # lolinet status 读这两行注释显示「凭据」和剩余天数（菜单栏 App 之类的客户端也可以读）；TOML 注释，easytier-core 忽略。
    config += f"\n# credential_id = {cid}\n# credential_expiry_unix = {expiry}\n"
    probe = None
    if not args.out and not args.no_probe:
        # near 只是兜底：设备上的 reporter 会按实测延迟找最近的接入点
        near = (args.peers.split(",") if args.peers else DEFAULT_PEERS)[0]
        probe = probe_config(argparse.Namespace(name=args.name, near=near, location=args.location))
        config += f"# probe = {json.dumps(probe, separators=(',', ':'))}\n"
    if NET.get("manager"):
        # 内网管理面（lolinet invite 用）：lolinet up 存进 /etc/lolinet/lolinet.json；和 web/agent/mesh_manager.py 写的一样
        config += f"# lolinet = {json.dumps({'manager': NET['manager'], 'manager_ip': CONSOLE_IP}, separators=(',', ':'))}\n"
    devices.append({"name": args.name, "overlay_ip": args.ip, "credential_id": cid, "expiry_unix": expiry,
                    "paired_at": dt.datetime.now().isoformat(timespec="seconds")})
    if args.out:
        # 不走配对码：配置直接落本地（600），由调用方手工下发后删掉（Docker / 没有 sudo 的设备）
        args.out.write_text(config)
        os.chmod(args.out, 0o600)
    else:
        code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(CODE_LEN))
        kv_put(f"pair:{hashlib.sha256(code.encode()).hexdigest()[:32]}", encrypt(code, config.encode()), args.code_minutes * 60)
    reg.write_text(json.dumps(devices, ensure_ascii=False, indent=2) + "\n")
    os.chmod(reg, 0o600)
    if args.out:
        print(f"{args.name} ({args.ip}) 凭据 {cid}，配置已写到 {args.out}（600，下发后删掉）")
        return
    shown = "-".join(code[i:i + 4] for i in range(0, CODE_LEN, 4))
    print(f"{args.name} ({args.ip}) 凭据 {cid}，配对码 {args.code_minutes} 分钟内有效、只能用一次：\n\n  {shown}\n\n"
          f"  curl -fsSL {BASE_URL}/i | sudo sh -s {shown}")
    print(f"\n  公开探针 {probe['name']} 一起装（只报硬件和位置）" if probe else "\n  不装公开探针")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("new")
    p.add_argument("--name", required=True)
    p.add_argument("--ip", help="overlay IP；不给就沿用同名设备的地址，或者取 --pool 里第一个空位（地址段见 nodes.example.yaml）")
    p.add_argument("--pool", choices=tuple(POOLS), default="desktop",
                   help="不给 --ip 时从哪个地址池取：desktop 10.188.16.0/20（人用电脑，默认）、userspace 10.188.32.0/20（无 TUN）、server 10.188.1.0/24")
    p.add_argument("--ttl-days", type=int, default=365)
    p.add_argument("--code-minutes", type=int, default=15, help="配对码有效期（分钟，1–1440），一次性不变")
    p.add_argument("--peers", help=f"逗号分隔的 relay 名，默认 {','.join(DEFAULT_PEERS)}")
    p.add_argument("--socks", help="本机 no-tun 实例的 SOCKS5 地址，如 127.0.0.1:18080")
    p.add_argument("--transport", choices=("wss", "tcp"), default="wss",
                   help="tcp：连 relay 的 EasyTier tcp 端口，给没有 SNI 补丁的官方二进制 / Docker 镜像用")
    p.add_argument("--out", type=Path, help="配置直接写到这个文件（600），不生成配对码（也不带探针）")
    p.add_argument("--no-probe", action="store_true", help="不装公开探针")
    p.add_argument("--location", help="探针公开的位置（城市名）；不给就按最近的接入点写「<城市>附近」")
    args = ap.parse_args()
    new(args)


if __name__ == "__main__":
    main()
