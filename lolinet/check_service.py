#!/usr/bin/env python3
"""lolinet 装的两个可选服务在 root 下真装一遍（CI 用 sudo 跑，build.yml 在本机能跑的平台上）：

- 出口节点：一份 userspace 的 /etc/lolinet/loli.toml（不用真起内核），`lolinet exit on --port <P>` 装服务
  （launchd co.lo-li.lolinet-exit / systemd lolinet-exit.service），服务降到 nobody 后在 127.0.0.1:<P> 上完成 SOCKS5 握手，
  `lolinet exit` 报运行中；`lolinet exit off` 之后服务没了、端口关了。
- mihomo：一个假的 mihomo（-v 报版本，-t 说配置没问题，平时一直睡），一个替身管理面给配置（会变，测 refresh），
  `lolinet mihomo on --bin <假的> --manager …` 写 /etc/lolinet/mihomo/config.yaml、装服务并跑起来，
  服务的命令行是 `<假的> -d /etc/lolinet/mihomo`；`lolinet status` 里有 mihomo 一行；`lolinet mihomo off` 之后服务没了。

只在 CI 里跑（要设 CI=1）：它会写 /etc/lolinet，装到一台真设备上会动到那台的配置。
用法: sudo CI=1 python3 lolinet/check_service.py <lolinet 二进制>
"""
from __future__ import annotations

import http.server
import os
import platform
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ETC = Path("/etc/lolinet")
CONF = ETC / "loli.toml"
MACOS = platform.system() == "Darwin"
FAILS: list[str] = []


def check(ok: bool, what: str) -> None:
    print(("  ok    " if ok else "  FAIL  ") + what)
    if not ok:
        FAILS.append(what)


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(list(args), capture_output=True, text=True, timeout=120)


def service_running(label: str, unit: str) -> bool:
    if MACOS:
        r = run("launchctl", "print", f"system/{label}")
        return r.returncode == 0 and "state = running" in r.stdout
    return run("systemctl", "is-active", "--quiet", unit).returncode == 0


def service_exists(label: str, unit: str) -> bool:
    if MACOS:
        return Path(f"/Library/LaunchDaemons/{label}.plist").exists() or run("launchctl", "print", f"system/{label}").returncode == 0
    return Path(f"/etc/systemd/system/{unit}.service").exists()


def wait(fn, timeout: float = 20) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        if fn():
            return True
        time.sleep(0.5)
    return False


def socks_ok(port: int) -> bool:
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=2)
        s.sendall(b"\x05\x01\x00")
        ok = s.recv(2) == b"\x05\x00"
        s.close()
        return ok
    except OSError:
        return False


class Manager(http.server.BaseHTTPRequestHandler):
    """替身管理面：给 mihomo 配置（带版本号，测 refresh），收出口登记"""
    rev = 1

    def log_message(self, *a):
        pass

    def reply(self, status: int, body: bytes, ctype: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/api/mesh/mihomo/config.yaml?mode="):
            mode = self.path.rsplit("=", 1)[1]
            body = f"# check_service {mode} rev {Manager.rev}\nmixed-port: 7890\nmode: rule\nrules:\n  - MATCH,DIRECT\n"
            return self.reply(200, body.encode(), "text/yaml")
        self.reply(404, b'{"message":"not found"}', "application/json")

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        self.reply(200, b'{"ok":true}', "application/json")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    if os.geteuid() != 0:
        raise SystemExit("要 root：sudo CI=1 python3 lolinet/check_service.py <lolinet>")
    if not os.environ.get("CI"):
        raise SystemExit("只在 CI 里跑（设 CI=1）：它会写 /etc/lolinet，装到真设备上会动到那台的配置")
    binary = os.path.abspath(sys.argv[1])
    ETC.mkdir(parents=True, exist_ok=True)
    # 前面的检查（check_probe.py）留下的配置和设置：先记下，最后放回（--manager 会把管理面地址记进 lolinet.json）
    saved = {p: p.read_bytes() for p in (CONF, ETC / "lolinet.json") if p.exists()}
    tmp = Path(tempfile.mkdtemp(prefix="lolinet-svc-"))
    os.chmod(tmp, 0o755)
    port = free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Manager)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    mport = srv.server_address[1]
    try:
        CONF.write_text('hostname = "svc-check"\nipv4 = "10.188.32.250/16"\nlisteners = []\n'
                        'socks5_proxy = "socks5://127.0.0.1:18080"\n\n[network_identity]\nnetwork_name = "chk"\n\n[flags]\nno_tun = true\n')
        os.chmod(CONF, 0o600)

        # ---- 出口节点 ----
        r = run(binary, "exit", "on", "--port", str(port))
        check(r.returncode == 0, f"lolinet exit on（{(r.stderr or r.stdout).strip()[-200:]}）")
        check(wait(lambda: service_running("co.lo-li.lolinet-exit", "lolinet-exit")), "出口服务在跑")
        check(wait(lambda: socks_ok(port)), f"降权后的出口在 127.0.0.1:{port} 上完成 SOCKS5 握手")
        if not MACOS:
            pid = run("systemctl", "show", "lolinet-exit", "-p", "MainPID", "--value").stdout.strip()
            user = run("ps", "-o", "user=", "-p", pid).stdout.strip() if pid and pid != "0" else ""
            check(user == "nobody", f"出口进程以 nobody 跑（{user or '?'}）")
        r = run(binary, "exit")
        check("运行中" in r.stdout, f"lolinet exit 报运行中：{r.stdout.strip()[:120]}")
        r = run(binary, "exit", "off")
        check(r.returncode == 0 and not service_exists("co.lo-li.lolinet-exit", "lolinet-exit") and wait(lambda: not socks_ok(port), 10),
              "lolinet exit off：服务撤了，端口关了")

        # ---- mihomo ----
        fake = tmp / "mihomo"
        fake.write_text('#!/bin/sh\ncase "$1" in\n  -v) echo "Mihomo Meta v0.0.0-check darwin arm64"; exit 0 ;;\n'
                        '  -t) exit 0 ;;\nesac\nexec sleep 3600\n')
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        r = run(binary, "mihomo", "on", "--bin", str(fake), "--mode", "userspace", "--manager", f"http://manager.test:{mport}@127.0.0.1")
        check(r.returncode == 0, f"lolinet mihomo on（{(r.stderr or r.stdout).strip()[-200:]}）")
        cfg = ETC / "mihomo" / "config.yaml"
        check(cfg.is_file() and "# check_service userspace" in cfg.read_text(), "配置从管理面取来、写进 /etc/lolinet/mihomo/config.yaml")
        check(wait(lambda: service_running("co.lo-li.lolinet-mihomo", "lolinet-mihomo")), "mihomo 服务在跑")
        # 服务跑的是拷到 /etc/lolinet/mihomo/mihomo 的那份（root 的、755），不是 --bin 给的那个（可能是用户写得了的）
        copy = ETC / "mihomo" / "mihomo"
        unit = Path("/Library/LaunchDaemons/co.lo-li.lolinet-mihomo.plist" if MACOS else "/etc/systemd/system/lolinet-mihomo.service")
        utext = unit.read_text() if unit.exists() else ""
        st = copy.stat() if copy.exists() else None
        check(st is not None and st.st_uid == 0 and stat.S_IMODE(st.st_mode) == 0o755 and str(copy) in utext and str(fake) not in utext,
              f"服务跑的是 root 的拷贝 {copy}（755），不是 --bin 给的文件")
        before = copy.read_bytes() if st else b""
        fake.write_text(fake.read_text() + "# 用户身份的进程改了它\n")
        check(st is not None and copy.read_bytes() == before, "之后改 --bin 给的那个文件，服务跑的那份不变")
        Manager.rev = 2
        r = run(binary, "mihomo", "refresh")
        check(r.returncode == 0 and "换成了" in r.stdout and "rev 2" in cfg.read_text(), f"管理面的配置变了：mihomo refresh 换上（{r.stdout.strip()}）")
        r = run(binary, "mihomo", "refresh")
        check(r.returncode == 0 and "没动" in r.stdout, "没变就不动")
        check(wait(lambda: service_running("co.lo-li.lolinet-mihomo", "lolinet-mihomo")), "换完 mihomo 服务还在跑")
        r = run(binary, "status")
        # 找以 mihomo 开头的那一行（别的行也会提到 mihomo，比如 TUN 模式那行的「给 mihomo 用」）
        mline = next((ln for ln in r.stdout.splitlines() if ln.startswith("mihomo ")), "")
        check("运行中" in mline and "/etc/lolinet/mihomo/mihomo" in mline, f"lolinet status 里有 mihomo 一行：{mline or r.stdout[-600:]}")
        r = run(binary, "mihomo", "off")
        check(r.returncode == 0 and not service_exists("co.lo-li.lolinet-mihomo", "lolinet-mihomo") and not copy.exists(),
              "lolinet mihomo off：服务撤了，拷贝删了")
        bad = tmp / "notmihomo"
        bad.write_text("#!/bin/sh\necho hello\n")
        bad.chmod(0o755)
        r = run(binary, "mihomo", "on", "--bin", str(bad), "--manager", f"http://manager.test:{mport}@127.0.0.1")
        check(r.returncode != 0 and "不像是 mihomo" in r.stderr, "不是 mihomo 的程序不装")
    finally:
        run(binary, "exit", "off")
        run(binary, "mihomo", "off")
        srv.shutdown()
        for p in (ETC / "exit.json", ETC / "mihomo.json"):
            p.unlink(missing_ok=True)
        shutil.rmtree(ETC / "mihomo", ignore_errors=True)
        for p in (CONF, ETC / "lolinet.json"):
            if p in saved:
                p.write_bytes(saved[p])
                os.chmod(p, 0o600 if p == CONF else 0o644)
            else:
                p.unlink(missing_ok=True)
        shutil.rmtree(tmp, ignore_errors=True)
    if FAILS:
        raise SystemExit(f"{len(FAILS)} 项没过")
    print("全部通过")


if __name__ == "__main__":
    main()
