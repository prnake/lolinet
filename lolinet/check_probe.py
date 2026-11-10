#!/usr/bin/env python3
"""实测 lolinet 内置的公开探针（lolinet/src/probe.rs）：会以 root 装系统服务，只在 CI runner 上跑（要环境变量 CI）。

起一个和 join/worker.js 一样验签的假准入站（127.0.0.1，随机端口），然后：
1. 摆一个 python 版探针（join/install.sh 以前装的：/etc/mesh-probe/reporter.json 设备模式 + co.lo-li.mesh-probe /
   mesh-probe 服务 + reporter.py），`lolinet up -c <不带探针的配置>` 要接管：公开 id 不变、python 版的服务和文件撤掉，
   内置探针以 nobody 跑、上报验签通过；
2. `lolinet up -c <末尾带 # probe = 的配置>`（换了 id）：旧 id 收到 bye，新 id 上报，/etc/lolinet/loli.toml 里没有那一行；
3. `lolinet upgrade`（清单版本和本机一样）：探针没变就不重启；探针的服务文件被删了就装回来；
4. `lolinet probe off`：新 id 收到 bye，服务、配置、进程都没了；没配置时 `lolinet up` 不装探针。最后 `lolinet down`。
只用标准库。用法: sudo CI=1 python3 lolinet/check_probe.py <lolinet 二进制>
"""
from __future__ import annotations

import hashlib
import hmac
import json
import os
import platform
import re
import secrets
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MAC = sys.platform == "darwin"
BIN = "/usr/local/bin/lolinet"
PROBE_JSON = Path("/etc/lolinet/probe.json")
LEGACY_JSON = Path("/etc/mesh-probe/reporter.json")
if MAC:
    UNIT = Path("/Library/LaunchDaemons/co.lo-li.lolinet-probe.plist")
    LEGACY_UNIT = Path("/Library/LaunchDaemons/co.lo-li.mesh-probe.plist")
    LEGACY_PY = Path("/usr/local/libexec/mesh-probe/reporter.py")
    LOG = ["cat", "/var/log/lolinet/probe.log"]
else:
    UNIT = Path("/etc/systemd/system/lolinet-probe.service")
    LEGACY_UNIT = Path("/etc/systemd/system/mesh-probe.service")
    LEGACY_PY = Path("/opt/mesh-probe/reporter.py")
    LOG = ["journalctl", "-u", "lolinet-probe", "--no-pager", "-n", "50"]

# 没有 TUN、没有 peer 的最小配置：只要内核起得来、管理口在
BASE_TOML = """hostname = "probe-ci"
instance_name = "probe-ci"
ipv4 = "10.188.250.1/16"
dhcp = false
listeners = []

[network_identity]
network_name = "probe-ci"
network_secret = "{secret}"

[flags]
no_tun = true
"""

lock = threading.Lock()
received: list[tuple[str, bool, object]] = []  # (X-Probe-Node, 验签过没过, body)
keys: dict[str, bytes] = {}
manifest: dict = {}


class Join(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        node, ts, sig = (self.headers.get(h, "") for h in ("X-Probe-Node", "X-Probe-Ts", "X-Probe-Sig"))
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        key = keys.get(node)
        ok = bool(
            self.path == "/v1/probe/report" and key and re.fullmatch(r"d-[0-9a-f]{10}", node)
            and ts.isdigit() and abs(time.time() - int(ts)) < 300
            and hmac.compare_digest(sig, hmac.new(key, ts.encode() + b"." + body, hashlib.sha256).hexdigest())
        )
        try:
            data = json.loads(body)
        except ValueError:
            data, ok = None, False
        with lock:
            received.append((node, ok, data))
        self.send_response(200 if ok else 403)
        self.end_headers()
        self.wfile.write(b"ok" if ok else b"bad signature")

    def do_GET(self) -> None:
        if self.path != "/lolinet.json":
            self.send_response(404)
            self.end_headers()
            return
        data = json.dumps(manifest).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args) -> None:
        pass


def fail(msg: str) -> None:
    raise SystemExit(f"FAIL {msg}")


def check(cond: object, what: str) -> None:
    if not cond:
        fail(what)
    print(f"ok   {what}", flush=True)


def sh(*cmd: str, ok: bool = True) -> str:
    p = subprocess.run(cmd, capture_output=True, text=True)
    out = p.stdout + p.stderr
    print(f"$ {' '.join(cmd)}  -> {p.returncode}\n{out}", flush=True)
    if ok and p.returncode != 0:
        fail(f"{' '.join(cmd)} 退出码 {p.returncode}")
    return out


def wait_for(what: str, pred, secs: float = 30) -> None:
    end = time.time() + secs
    while time.time() < end:
        if pred():
            print(f"ok   {what}", flush=True)
            return
        time.sleep(0.5)
    fail(f"等了 {secs:.0f} 秒：{what}")


def got(node: str, bye: bool = False) -> list:
    with lock:
        return [b for n, good, b in received if n == node and good and isinstance(b, dict) and bool(b.get("bye")) == bye]


def device(url: str) -> dict:
    """和 scripts/pair.py 写进配对包的一样：公开 id、这台的上报密钥、上报地址"""
    pid, key = "d-" + secrets.token_hex(5), secrets.token_bytes(32)
    keys[pid] = key
    return {"name": pid, "mode": "device", "url": url, "key": key.hex(), "location": "CI"}


def probe_pids() -> list[str]:
    return subprocess.run(["pgrep", "-f", "lolinet probe run"], capture_output=True, text=True).stdout.split()


def user_of(pid: str) -> str:
    return subprocess.run(["ps", "-o", "user=", "-p", pid], capture_output=True, text=True).stdout.strip()


def legacy_loaded() -> bool:
    if MAC:
        return subprocess.run(["launchctl", "print", "system/co.lo-li.mesh-probe"], capture_output=True).returncode == 0
    return subprocess.run(["systemctl", "is-active", "--quiet", "mesh-probe"]).returncode == 0


def legacy_setup(cfg: dict) -> None:
    """join/install.sh 以前装的 python 版：配置、reporter.py、服务（这里只跑 sleep，看它会不会被撤掉）"""
    LEGACY_JSON.parent.mkdir(parents=True, exist_ok=True)
    LEGACY_JSON.write_text(json.dumps(cfg))
    os.chmod(LEGACY_JSON, 0o600)
    LEGACY_PY.parent.mkdir(parents=True, exist_ok=True)
    LEGACY_PY.write_text("# python 版 reporter.py 的占位\n")
    if MAC:
        LEGACY_UNIT.write_text("""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>co.lo-li.mesh-probe</string>
  <key>ProgramArguments</key><array><string>/bin/sleep</string><string>86400</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
""")
        sh("launchctl", "bootstrap", "system", str(LEGACY_UNIT))
    else:
        LEGACY_UNIT.write_text("[Service]\nExecStart=/bin/sleep infinity\n\n[Install]\nWantedBy=multi-user.target\n")
        sh("systemctl", "daemon-reload")
        sh("systemctl", "enable", "--now", "mesh-probe")
    check(legacy_loaded(), "python 版探针（占位）跑起来了")


def run(exe: str) -> None:
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Join)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    url = f"{base}/v1/probe/report"
    tmp = Path(tempfile.mkdtemp())
    toml = BASE_TOML.format(secret=secrets.token_hex(8))

    # 1. 接管 python 版
    a = device(url)
    legacy_setup(a)
    plain = tmp / "plain.toml"
    plain.write_text(toml)
    out = sh(exe, "up", "-c", str(plain), "--auto-upgrade", "off")
    check("python 版公开探针换成了 lolinet 内置的（公开 id 不变）" in out, "up 说接管了 python 版")
    check(json.loads(PROBE_JSON.read_text())["name"] == a["name"], "接管后公开 id 不变")
    st = PROBE_JSON.stat()
    check(st.st_uid == 0 and st.st_mode & 0o777 == 0o600, "/etc/lolinet/probe.json 是 root 的 600")
    check(not LEGACY_UNIT.exists() and not LEGACY_JSON.exists() and not LEGACY_PY.exists(), "python 版的服务文件、配置、reporter.py 都删了")
    check(not legacy_loaded(), "python 版的服务停了")
    check(UNIT.exists(), f"内置探针的服务文件 {UNIT}")
    wait_for(f"{a['name']} 上报且验签通过", lambda: got(a["name"]))
    dev = got(a["name"])[0]["device"]
    check({"kind", "model", "chip", "cores", "mem", "os", "arch", "disks", "location", "near"} <= set(dev), f"上报字段齐：{dev}")
    check(dev["location"] == "CI" and dev["cores"] and dev["arch"] and dev["disks"], "上报内容有值")
    check(not {"hostname", "ip", "ipv4", "serial", "load"} & set(dev), "没有主机名、地址、序列号、负载")
    wait_for("探针进程在", lambda: len(probe_pids()) == 1)
    check(user_of(probe_pids()[0]) == "nobody", "探针进程降到了 nobody")
    check(f"运行中 · {a['name']}" in sh(BIN, "status"), "lolinet status 显示探针在跑")

    # 2. 重新配对换了 id：旧 id 撤下，新 id 上报，配置里那一行不进 EasyTier
    b = device(url)
    paired = tmp / "pair.toml"
    paired.write_text(toml + f"\n# credential_id = probe-ci\n# probe = {json.dumps(b, separators=(',', ':'))}\n")
    out = sh(exe, "up", "-c", str(paired), "--auto-upgrade", "off")
    check(f"公开探针 {b['name']} 已启动" in out, "up -c 带探针：探针装上")
    check(got(a["name"], bye=True), f"旧 id {a['name']} 收到 bye")
    check(json.loads(PROBE_JSON.read_text())["name"] == b["name"], "probe.json 换成新 id")
    loli = Path("/etc/lolinet/loli.toml").read_text()
    check("# probe = " not in loli and "# credential_id = probe-ci" in loli, "loli.toml 里没有探针那一行，别的注释还在")
    wait_for(f"{b['name']} 上报且验签通过", lambda: got(b["name"]))

    # 3. 自动升级：版本一样时不碰探针；服务文件没了就装回来
    version = subprocess.run([BIN, "--version"], capture_output=True, text=True).stdout.split()[-1]
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(platform.machine(), platform.machine())
    plat = f"{'macos' if MAC else 'linux'}-{arch}"
    manifest.update({"version": version, "lolinet": {plat: {"file": f"lolinet-{plat}.tar.gz", "sha256": "0" * 64}}})
    wait_for("探针进程在", lambda: len(probe_pids()) == 1)
    before = probe_pids()
    out = sh(BIN, "upgrade", "--source", f"{base}/lolinet.json")
    check("和发布的一致" in out and "探针" not in out, "版本一样时 upgrade 不动探针")
    check(probe_pids() == before, "探针没被重启")
    if MAC:
        sh("launchctl", "bootout", "system/co.lo-li.lolinet-probe", ok=False)
    else:
        sh("systemctl", "disable", "--now", "lolinet-probe", ok=False)
    UNIT.unlink()
    wait_for("探针进程停了", lambda: not probe_pids())
    out = sh(BIN, "upgrade", "--source", f"{base}/lolinet.json")
    check(f"公开探针 {b['name']} 已启动" in out, "服务文件没了：upgrade 装回来")
    wait_for("探针进程回来了", lambda: len(probe_pids()) == 1)

    # 4. 关掉
    out = sh(BIN, "probe", "off")
    check(got(b["name"], bye=True), f"{b['name']} 收到 bye")
    check(not PROBE_JSON.exists() and not UNIT.exists(), "off 之后配置和服务文件都没了")
    wait_for("off 之后探针进程没了", lambda: not probe_pids())
    check("探针    没装" in sh(BIN, "status"), "lolinet status 显示没装")
    sh(exe, "up", "--auto-upgrade", "off")
    check(not UNIT.exists() and not PROBE_JSON.exists(), "没有探针配置时 up 不装探针")

    with lock:
        bad = [(n, b) for n, good, b in received if not good]
    check(not bad, f"收到的上报都验签通过（共 {len(received)} 条）")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    if os.geteuid() != 0 or not os.environ.get("CI"):
        raise SystemExit("要 root，而且只在 CI 里跑（会装系统服务）：sudo CI=1 python3 lolinet/check_probe.py <lolinet>")
    try:
        run(os.path.abspath(sys.argv[1]))
    except BaseException:
        print("---- 探针日志", flush=True)
        subprocess.run(LOG)
        raise
    finally:
        subprocess.run([BIN, "down"], capture_output=True)
    print("公开探针实测通过")


if __name__ == "__main__":
    main()
