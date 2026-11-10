#!/usr/bin/env python3
"""lo-li mesh 公开探针上报：每 30 秒把本机负载和 EasyTier 链路签名后推给准入站的 /v1/probe/report。

只读：读 /proc、statvfs 和 `easytier-cli -o json peer`。报接入点之间的链路（overlay 10.188.0.0/24），
设备只报匿名 id（配置里的 devid_key 对主机名做 HMAC）、是否直连和延迟，名字和地址不出 relay。
只用标准库，relay 上的 python3（3.12）直接跑。

配置（JSON，600；systemd 用 LoadCredential 传进来）：
  {"name": "<relay 名>", "key": "<hex，pair 的派生密钥>", "url": "https://<准入站>/v1/probe/report"}
签名：X-Probe-Sig = hex(HMAC-SHA256(key, "<ts>." + body))，key = HMAC-SHA256(master, "probe:<name>")。
master 只在 Worker 和$LOLINET_HOME/state/mesh-probe.json 里；一台 relay 被攻破只能伪造它自己的卡片。

设备模式（配置里 "mode": "device"，name 是 d-<10 位十六进制>，由 probe/device.py 生成，和设备名无关）。
装了 lolinet 的设备用 lolinet 内置的探针（lolinet probe，同样的格式和签名，配对时装）；这里的设备模式留给 Docker 里的设备：
只报硬件和位置（型号、处理器、内存、存储总量、系统、架构、配置的位置或最近的接入点），每 60 秒一次心跳；
不报主机名、地址、序列号、负载。macOS 和 Linux 都能跑（macOS 自带的 python3 3.9 就行）。

用法:
  python3 reporter.py                    # 常驻，systemd mesh-probe.service
  python3 reporter.py --once             # 采一次（3 秒），打印将要上报的 JSON，不发送
  python3 reporter.py --config FILE      # 指定配置（设备上：launchd / docker compose）
"""
from __future__ import annotations

import hashlib
import hmac
import ipaddress
import json
import os
import platform
import shutil
import subprocess
import sys
import time
import urllib.request

CLI = "/opt/easytier/easytier-cli"
RPC = "127.0.0.1:15888"
INTERVAL = 30
DEVICE_INTERVAL = 60
INFRA = ipaddress.ip_network("10.188.0.0/24")  # ipam.md：relay / 基础设施段


def config() -> dict:
    if "--config" in sys.argv:
        path = sys.argv[sys.argv.index("--config") + 1]
    elif os.environ.get("CREDENTIALS_DIRECTORY"):
        path = os.path.join(os.environ["CREDENTIALS_DIRECTORY"], "probe")
    else:
        path = "/etc/mesh-probe/reporter.json"
    with open(path) as f:
        return json.load(f)


def run(*cmd: str) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return ""


def cpu_model() -> str | None:
    if sys.platform == "darwin":
        return run("sysctl", "-n", "machdep.cpu.brand_string") or None
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    return " ".join(line.split(":", 1)[1].split())
    except OSError:
        pass
    return None


def cpu_times() -> tuple[int, int]:
    with open("/proc/stat") as f:
        v = [int(x) for x in f.readline().split()[1:]]
    idle = v[3] + (v[4] if len(v) > 4 else 0)
    return idle, sum(v[:8])


def default_iface() -> str:
    with open("/proc/net/route") as f:
        for line in f.readlines()[1:]:
            p = line.split()
            if p[1] == "00000000":
                return p[0]
    return "eth0"


def net_bytes(iface: str) -> tuple[int, int]:
    with open("/proc/net/dev") as f:
        for line in f:
            if line.strip().startswith(iface + ":"):
                v = line.split(":", 1)[1].split()
                return int(v[0]), int(v[8])
    return 0, 0


def meminfo() -> dict[str, int]:
    out = {}
    with open("/proc/meminfo") as f:
        for line in f:
            k, v = line.split(":", 1)
            out[k] = int(v.split()[0]) * 1024
    return out


def os_name() -> str:
    try:
        with open("/etc/os-release") as f:
            for line in f:
                if line.startswith("PRETTY_NAME="):
                    return line.split("=", 1)[1].strip().strip('"')
    except OSError:
        pass
    return "Linux"


def num(s) -> float | None:
    try:
        v = float(str(s).rstrip("%"))
    except ValueError:
        return None
    return v


def device_id(devid_key: str, hostname: str) -> str:
    """设备的公开 id：d- + HMAC-SHA256(devid_key, EasyTier 主机名) 前 10 位。和 probe/device.py 一致；没有 devid_key 反推不出主机名。"""
    return "d-" + hmac.new(bytes.fromhex(devid_key), hostname.encode(), hashlib.sha256).hexdigest()[:10]


def easytier(cfg: dict | None = None) -> dict:
    devid_key = (cfg or {}).get("devid_key")
    try:
        p = subprocess.run([CLI, "-p", RPC, "-o", "json", "peer"], capture_output=True, text=True, timeout=10)
        peers = json.loads(p.stdout) if p.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired, ValueError):
        peers = None
    if peers is None:
        return {"ok": False, "links": [], "devices": 0}
    links, devices, version = [], {}, None
    for x in peers:
        if x.get("cost") == "Local":
            version = x.get("version")
            continue
        ip = (x.get("ipv4") or "").split("/")[0]
        if not ip:
            continue  # 控制台网关等无地址节点
        try:
            infra = ipaddress.ip_address(ip) in INFRA
        except ValueError:
            continue
        if not infra:
            # 设备：主机名在这里就换成匿名 id，名字和地址都不出这台 relay
            host = x.get("hostname") or ""
            key = device_id(devid_key, host) if devid_key and host else f"peer:{x.get('id')}"
            direct = x.get("cost") == "p2p"
            lat = num(x.get("lat_ms")) if direct else None
            prev = devices.get(key)
            if prev is None or (lat is not None and (prev["lat"] is None or lat < prev["lat"])):
                devices[key] = {"id": key, "direct": direct, "lat": lat}
            continue
        direct = x.get("cost") == "p2p"
        loss = num(x.get("loss_rate"))
        links.append({
            "peer": x.get("hostname"),
            "direct": direct,
            "lat": num(x.get("lat_ms")) if direct else None,
            "loss": loss / 100 if direct and loss is not None else None,
            "proto": (x.get("tunnel_proto") or None) if direct else None,
        })
    peers_out = [d for d in devices.values() if d["id"].startswith("d-")]
    return {"ok": True, "version": version, "links": links, "devices": len(devices), "peers": peers_out}


def sample(seconds: float) -> dict:
    iface = default_iface()
    i0, t0 = cpu_times()
    r0, s0 = net_bytes(iface)
    w0 = time.monotonic()
    time.sleep(seconds)
    i1, t1 = cpu_times()
    r1, s1 = net_bytes(iface)
    dt = max(time.monotonic() - w0, 0.001)
    m = meminfo()
    st = os.statvfs("/")
    with open("/proc/uptime") as f:
        uptime = int(float(f.read().split()[0]))
    busy = 1 - (i1 - i0) / max(t1 - t0, 1)
    return {
        "os": os_name(),
        "cpu_model": cpu_model(),
        "cores": os.cpu_count(),
        "cpu": round(max(0.0, min(1.0, busy)) * 100, 1),
        "load": [round(x, 2) for x in os.getloadavg()],
        "mem": [m["MemTotal"] - m.get("MemAvailable", m.get("MemFree", 0)), m["MemTotal"]],
        "swap": [m.get("SwapTotal", 0) - m.get("SwapFree", 0), m.get("SwapTotal", 0)],
        "disk": [(st.f_blocks - st.f_bfree) * st.f_frsize, st.f_blocks * st.f_frsize],
        "net": {"rx": round((r1 - r0) / dt), "tx": round((s1 - s0) / dt), "rx_total": r1, "tx_total": s1},
        "uptime": uptime,
    }


# ---------- 设备模式：只报硬件和位置 ----------

def read(path: str) -> str | None:
    try:
        with open(path) as f:
            return f.read().strip() or None
    except OSError:
        return None


def mac_hardware() -> dict:
    try:
        hw = json.loads(run("system_profiler", "SPHardwareDataType", "-json"))["SPHardwareDataType"][0]
    except (ValueError, KeyError, IndexError):
        hw = {}
    model = hw.get("machine_name")  # 只取型号和芯片；序列号、UUID 一律不碰
    chip = hw.get("chip_type") or hw.get("cpu_type") or cpu_model()
    ver = run("sw_vers", "-productVersion")
    return {"model": model, "chip": chip, "mem": int(run("sysctl", "-n", "hw.memsize") or 0) or None,
            "os": f"macOS {ver}" if ver else "macOS", "kind": "笔记本" if model and "Book" in model else "Mac"}


def linux_hardware(cfg: dict) -> dict:
    vendor = read("/sys/class/dmi/id/sys_vendor")
    product = read("/sys/class/dmi/id/product_name")
    mem = None
    try:
        mem = meminfo()["MemTotal"]
    except (OSError, KeyError):
        pass
    osr = cfg.get("os_release", "/etc/os-release")
    pretty = None
    for line in (read(osr) or "").splitlines():
        if line.startswith("PRETTY_NAME="):
            pretty = line.split("=", 1)[1].strip().strip('"')
    return {"model": " ".join(x for x in (vendor, product) if x) or None, "chip": cpu_model(), "mem": mem,
            "os": pretty or "Linux", "kind": "服务器"}


def hardware(cfg: dict) -> dict:
    hw = mac_hardware() if sys.platform == "darwin" else linux_hardware(cfg)
    disks = []
    for d in cfg.get("disks") or [{"path": "/"}]:
        try:
            st = os.statvfs(d["path"])
            disks.append({"label": d.get("label"), "total": st.f_blocks * st.f_frsize})
        except OSError:
            pass
    hw.update(cores=os.cpu_count(), arch=platform.machine(), disks=disks)
    for k in ("kind", "model", "chip", "os"):  # 配置里写了就以配置为准（比如 DMI 里是厂商代号）
        if cfg.get(k):
            hw[k] = cfg[k]
    return hw


def nearest_relay(cfg: dict) -> dict | None:
    """本机 easytier-core 看到的最近接入点（只用来推断大致位置，公开时换成城市）。读不到就用配置里的 near。"""
    cli = cfg.get("cli") or shutil.which("easytier-cli") or next(
        (c for c in ("/usr/local/libexec/easytier/easytier-cli", CLI) if os.path.exists(c)), None)
    best = None
    for rpc in ([cfg["rpc"]] if cfg.get("rpc") else ["127.0.0.1:15888", "127.0.0.1:15897"]) if cli else []:
        try:
            peers = json.loads(run(cli, "-p", rpc, "-o", "json", "peer") or "null")
        except ValueError:
            peers = None
        for x in peers or []:
            ip = (x.get("ipv4") or "").split("/")[0]
            lat = num(x.get("lat_ms"))
            if x.get("cost") == "p2p" and ip and lat is not None and ipaddress.ip_address(ip) in INFRA:
                if best is None or lat < best["lat"]:
                    best = {"peer": x.get("hostname"), "lat": lat}
        if best:
            break
    return best or ({"peer": cfg["near"], "lat": None} if cfg.get("near") else None)


def device_report(cfg: dict, hw: dict) -> dict:
    return {"v": 1, "name": cfg["name"], "device": {**hw, "location": cfg.get("location"), "near": nearest_relay(cfg)}}


def report(cfg: dict, seconds: float) -> dict:
    return {"v": 1, "name": cfg["name"], "host": sample(seconds), "et": easytier(cfg)}


def send(cfg: dict, payload: dict) -> int:
    body = json.dumps(payload, separators=(",", ":")).encode()
    ts = str(int(time.time()))
    sig = hmac.new(bytes.fromhex(cfg["key"]), ts.encode() + b"." + body, hashlib.sha256).hexdigest()
    req = urllib.request.Request(cfg["url"], data=body, method="POST", headers={
        "Content-Type": "application/json", "User-Agent": "mesh-probe/1",
        "X-Probe-Node": cfg["name"], "X-Probe-Ts": ts, "X-Probe-Sig": sig,
    })
    with urllib.request.urlopen(req, timeout=20) as r:
        return r.status


def main() -> None:
    cfg = config()
    if cfg.get("mode") == "device":
        hw, at = hardware(cfg), time.time()
        if "--once" in sys.argv:
            json.dump(device_report(cfg, hw), sys.stdout, ensure_ascii=False, indent=1)
            print()
            return
        while True:
            if time.time() - at > 6 * 3600:  # 硬件不常变，6 小时重读一次
                hw, at = hardware(cfg), time.time()
            try:
                send(cfg, device_report(cfg, hw))
            except Exception as e:  # noqa: BLE001
                print(f"report failed: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
            time.sleep(DEVICE_INTERVAL)
    if "--once" in sys.argv:
        json.dump(report(cfg, 3), sys.stdout, ensure_ascii=False, indent=1)
        print()
        return
    while True:
        # 采样窗口本身就是间隔：CPU 和网速是这 30 秒的平均值
        payload = report(cfg, INTERVAL)
        try:
            send(cfg, payload)
        except Exception as e:  # noqa: BLE001 —— 上报失败等下一轮，不积压
            print(f"report failed: {type(e).__name__}: {e}", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
