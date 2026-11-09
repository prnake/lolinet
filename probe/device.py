# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""给设备装公开探针上报（probe/reporter.py 的设备模式）：只公开硬件和位置，不公开设备名。

设备的公开 id = d- + HMAC(devid_key, EasyTier 主机名) 前 10 位，devid_key = HMAC(master, "devid")；
relay 用同一把 devid_key 把它看到的主机名换成 id，所以 relay 报的链路和设备自己报的硬件能对上，名字不出机器。上报密钥 = HMAC(master, "probe:<id>")，Worker 不需要设备名单。
master 在 $LOLINET_HOME/state/mesh-probe.json（和 Worker secret PROBE_MASTER 相同）。

位置：--location 写死（城市名），或者 --near <relay> 作为兜底；设备上能读到 easytier-cli 时，用实测最近的接入点，
公开成「<城市>附近」。

用法:
  uv run probe/device.py id --name nas
  uv run probe/device.py config --name nas --near chen --kind NAS --model "<型号>" \\
      --os "<系统名>" --os-release /host/os-release --disk /mnt/vol1=存储1 --disk /mnt/vol2=存储2 --out /tmp/x.json
  uv run probe/device.py install-mac --near chen                  # 本机：LaunchAgent，不要 root；主机名读本机 easytier
  uv run probe/device.py bye --name <主机名>                       # 从公开页面撤下（卸载了 / 改名了）
  uv run probe/device.py uninstall-mac"""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
# 运行时数据（state/mesh-probe.json）：LOLINET_HOME，兼容旧的 FLEET_REPO，默认 ~/.config/lolinet
HOME_DIR = Path(os.environ.get("LOLINET_HOME") or os.environ.get("FLEET_REPO") or Path.home() / ".config/lolinet")
LABEL = "co.lo-li.mesh-probe"
MAC_DIR = Path.home() / "Library/Application Support/lo-li mesh/probe"
PLIST = Path.home() / f"Library/LaunchAgents/{LABEL}.plist"


def report_url() -> str:
    """准入站的上报接口：LOLINET_BASE_URL，不设就按行找节点清单里的 network.join（这个脚本只用标准库）。"""
    base = os.environ.get("LOLINET_BASE_URL")
    if not base:
        p = Path(os.environ.get("LOLINET_NODES") or HERE.parent / "nodes.yaml")
        p = p if p.is_file() else HERE.parent / "nodes.example.yaml"
        base = "https://" + re.search(r"^\s+join:\s*([^\s#]+)", p.read_text(), re.M).group(1)
    return base.rstrip("/") + "/v1/probe/report"


def master() -> str:
    return json.loads((HOME_DIR / "state" / "mesh-probe.json").read_text())["master"]


def devid_key() -> str:
    """设备 id 的派生密钥：HMAC(master, "devid")。relay 也拿这把（probe/deploy.py 下发），拿它把看到的主机名换成同一个 id。"""
    return hmac.new(bytes.fromhex(master()), b"devid", hashlib.sha256).hexdigest()


def device_id(name: str) -> str:
    """name 必须是设备在 EasyTier 里的主机名，relay 看到的就是它，才能和 relay 报的链路对上。"""
    return "d-" + hmac.new(bytes.fromhex(devid_key()), name.encode(), hashlib.sha256).hexdigest()[:10]


def device_config(a: argparse.Namespace) -> dict:
    node = device_id(a.name)
    cfg = {"name": node, "mode": "device", "url": report_url(),
           "key": hmac.new(bytes.fromhex(master()), f"probe:{node}".encode(), hashlib.sha256).hexdigest()}
    for k in ("near", "location", "kind", "model", "os", "os_release", "rpc", "cli"):
        if getattr(a, k, None):
            cfg[k] = getattr(a, k)
    if getattr(a, "disk", None):
        cfg["disks"] = [dict(zip(("path", "label"), d.split("=", 1))) for d in a.disk]
    return cfg


def write_secret(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n")
    os.chmod(path, 0o600)


def local_hostname() -> str | None:
    """本机 easytier-core 的主机名（relay 看到的就是它）：TUN 系统服务 15888，菜单栏 App 15897。"""
    cli = shutil.which("easytier-cli") or "/usr/local/libexec/easytier/easytier-cli"
    for rpc in ("127.0.0.1:15888", "127.0.0.1:15897"):
        try:
            out = subprocess.run([cli, "-p", rpc, "-o", "json", "node"], capture_output=True, text=True, timeout=10).stdout
            name = json.loads(out).get("hostname")
        except (OSError, subprocess.TimeoutExpired, ValueError, AttributeError):
            continue
        if name:
            return name
    return None


def bye(node: str) -> None:
    """用这个 id 自己的 key 签一条 bye，Worker 立刻把它从公开页面删掉。"""
    sys.path.insert(0, str(HERE))
    import reporter  # noqa: PLC0415
    key = hmac.new(bytes.fromhex(master()), f"probe:{node}".encode(), hashlib.sha256).hexdigest()
    try:
        reporter.send({"name": node, "key": key, "url": report_url()}, {"v": 1, "name": node, "bye": True})
        print(f"{node}：已从公开页面撤下")
    except Exception as e:  # noqa: BLE001
        print(f"{node}：bye 没发出去（{type(e).__name__}: {e}），7 天后自动消失")


def install_mac(a: argparse.Namespace) -> None:
    old = MAC_DIR / "reporter.json"
    prev = json.loads(old.read_text()).get("name") if old.is_file() else None
    if prev and prev != device_id(a.name):
        bye(prev)  # 主机名改过：旧 id 的卡片撤掉，免得页面上多一台「离线」
    MAC_DIR.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(HERE / "reporter.py", MAC_DIR / "reporter.py")
    write_secret(MAC_DIR / "reporter.json", device_config(a))
    PLIST.parent.mkdir(parents=True, exist_ok=True)
    PLIST.write_text(f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array>
    <string>/usr/bin/python3</string><string>-I</string><string>{MAC_DIR}/reporter.py</string>
    <string>--config</string><string>{MAC_DIR}/reporter.json</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>StandardErrorPath</key><string>{MAC_DIR}/reporter.log</string>
</dict></plist>
""")
    uid = os.getuid()
    subprocess.run(["launchctl", "bootout", f"gui/{uid}/{LABEL}"], capture_output=True)
    subprocess.run(["launchctl", "bootstrap", f"gui/{uid}", str(PLIST)], check=True)
    print(f"{a.name} -> {device_id(a.name)}：LaunchAgent {LABEL} 已启动，日志 {MAC_DIR}/reporter.log")


def uninstall_mac() -> None:
    cfg = MAC_DIR / "reporter.json"
    if cfg.is_file():
        bye(json.loads(cfg.read_text())["name"])
    subprocess.run(["launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}"], capture_output=True)
    PLIST.unlink(missing_ok=True)
    shutil.rmtree(MAC_DIR, ignore_errors=True)
    print("已卸载")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("id").add_argument("--name", required=True)
    sub.add_parser("bye", help="把一台设备从公开页面撤下（设备卸载了、或者改了主机名）").add_argument("--name", required=True)
    for name in ("config", "install-mac"):
        p = sub.add_parser(name)
        p.add_argument("--name", required=(name == "config"),
                       help="设备在 EasyTier 里的主机名（easytier-cli node 的 hostname），只用来派生 id，不上报；install-mac 不给就读本机的")
        p.add_argument("--near", help="兜底的最近接入点（relay 名）")
        p.add_argument("--location", help="公开的位置（城市名），写了就不按接入点推断")
        p.add_argument("--kind", help="类型，如 NAS / 笔记本 / 服务器")
        p.add_argument("--model", help="型号（DMI 里是厂商代号时用）")
        p.add_argument("--os", help="系统名（容器里读不到宿主时用）")
        p.add_argument("--os-release", dest="os_release", help="宿主 os-release 的路径（容器里）")
        p.add_argument("--disk", action="append", help="PATH=标签，统计这个挂载点的总容量；不给就统计 /")
        p.add_argument("--rpc", help="easytier-core 管理口，默认依次试 15888、15897")
        p.add_argument("--cli", help="easytier-cli 路径")
        if name == "config":
            p.add_argument("--out", type=Path, required=True)
    sub.add_parser("uninstall-mac")
    a = ap.parse_args()
    if a.cmd == "id":
        print(device_id(a.name))
    elif a.cmd == "config":
        write_secret(a.out, device_config(a))
        print(f"{a.name} -> {device_id(a.name)}：{a.out}（600，含上报密钥）")
    elif a.cmd == "bye":
        bye(device_id(a.name))
    elif a.cmd == "install-mac":
        a.name = a.name or local_hostname()
        if not a.name:
            raise SystemExit("读不到本机 easytier-core 的主机名，用 --name 指定")
        install_mac(a)
    else:
        uninstall_mac()


if __name__ == "__main__":
    main()
