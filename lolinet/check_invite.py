#!/usr/bin/env python3
"""成员邀请的端到端检查：lolinet invite -> 内网管理面（web/agent/mesh_manager.py）-> 新机器拿配对码入网。只用标准库和 openssl，不要 root。

回环上搭一套小网：
- 签发节点 issuer（network_secret + secure mode + credential_file，userspace），管理面经它的 RPC 签凭据；
- 成员 laptop：用 issuer 签的凭据入网（10.188.16.10）；
- 假的准入站 Worker：核对管理面的 HMAC 签名，收配对包（KV）、给探针密钥；
- 一个替 nginx 的小反代：把请求转给管理面，带上 X-Forwarded-For = 成员的 overlay 地址（线上 nginx 用 $remote_addr）。

检查：
- lolinet invite --kind userspace 拿到配对码，地址是 userspace 池里的第一个（10.188.32.1）；
- 用 install.sh 一样的 openssl 参数解开配对包，配置里有 no_tun、socks5_proxy、探针那一行和 # lolinet（管理面地址）；
- 新机器用这份配置真的入网：issuer 的路由表里出现 10.188.32.1；
- 凭据写进了 issuer 的 credential_file；
- 策略：成员要 server 被拒（403）；不在路由表里的来源被拒（403）；超过每天次数被拒（429）；
- 签发节点没有 credential_file 时拒绝（503）。

用法: python3 lolinet/check_invite.py <lolinet 二进制>
"""
from __future__ import annotations

import hashlib
import hmac
import http.server
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANAGER = ROOT / "web" / "agent" / "mesh_manager.py"
FAILS: list[str] = []
MANAGER_KEY = "11" * 32
NETWORK = "chk-invite"
SECRET = "chk-invite-secret"


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


class FakeWorker(http.server.BaseHTTPRequestHandler):
    """准入站 Worker 的 /v1/manager/*：签名照 worker.js 核对（HMAC(manager_key, "<ts>.<path>." + body)）"""
    blobs: dict = {}
    bad_sig = 0

    def log_message(self, *a):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        ts = self.headers.get("X-Manager-Ts", "")
        want = hmac.new(bytes.fromhex(MANAGER_KEY), f"{ts}.{self.path}.".encode() + body, hashlib.sha256).hexdigest()
        if not hmac.compare_digest(want, self.headers.get("X-Manager-Sig", "")):
            FakeWorker.bad_sig += 1
            return self.reply(401, {"error": "bad signature"})
        req = json.loads(body)
        if self.path == "/v1/manager/probe-key":
            return self.reply(200, {"key": hmac.new(b"probe", req["id"].encode(), hashlib.sha256).hexdigest()})
        if self.path == "/v1/manager/pair":
            FakeWorker.blobs[req["id"]] = req["blob"]
            return self.reply(200, {"expires_at": int(time.time()) + req["ttl"]})
        self.reply(404, {})

    def reply(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def nginx(upstream: int, client_ip: list):
    """替 nginx：Host 原样，X-Forwarded-For 设成 client_ip[0]（线上是 $remote_addr，成员的 overlay 地址）"""

    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            headers = {k: v for k, v in self.headers.items() if k.lower() not in ("content-length", "x-forwarded-for", "connection")}
            headers["X-Forwarded-For"] = client_ip[0]
            req = urllib.request.Request(f"http://127.0.0.1:{upstream}{self.path}", data=body, method="POST", headers=headers)
            try:
                with urllib.request.urlopen(req, timeout=60) as r:
                    status, data = r.status, r.read()
            except urllib.error.HTTPError as e:
                status, data = e.code, e.read()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    return H


def serve(handler) -> tuple[http.server.ThreadingHTTPServer, int]:
    port = free_port()
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, port


def cli_json(cli: str, rpc: int, *args: str):
    out = subprocess.run([cli, "-p", f"127.0.0.1:{rpc}", "-o", "json", *args], capture_output=True, text=True, timeout=20)
    if out.returncode != 0:
        raise RuntimeError(out.stderr or out.stdout)
    return json.loads(out.stdout)


def wait_for(fn, timeout: float = 30):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
            if v:
                return v
        except Exception:  # noqa: BLE001
            pass
        time.sleep(0.5)
    return None


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    binary = os.path.abspath(sys.argv[1])
    tmp = Path(tempfile.mkdtemp(prefix="lolinet-invite-"))
    core, cli = tmp / "easytier-core", tmp / "easytier-cli"
    os.symlink(binary, core)
    os.symlink(binary, cli)
    pa, ra, rm, rn, sm, sn = (free_port() for _ in range(6))
    procs: list[subprocess.Popen] = []

    def start(name: str, conf: str, rpc: int) -> None:
        path = tmp / f"{name}.toml"
        path.write_text(conf)
        log = open(tmp / f"{name}.log", "w")
        procs.append(subprocess.Popen([str(core), "-c", str(path), "--rpc-portal", f"127.0.0.1:{rpc}", "--console-log-level", "warn"],
                                      stdout=log, stderr=subprocess.STDOUT))

    try:
        start("issuer", f'''hostname = "issuer"
instance_name = "chk-issuer"
ipv4 = "10.188.0.14/16"
dhcp = false
listeners = ["tcp://127.0.0.1:{pa}"]
credential_file = "{tmp / 'credentials.json'}"

[network_identity]
network_name = "{NETWORK}"
network_secret = "{SECRET}"

[secure_mode]
enabled = true

[flags]
no_tun = true
private_mode = true
''', ra)
        cred = wait_for(lambda: cli_json(str(cli), ra, "credential", "generate", "--ttl", "3600", "--credential-id", "laptop-1",
                                         "--reusable", "false"))
        if not cred:
            raise SystemExit("issuer 没起来：" + (tmp / "issuer.log").read_text()[-2000:])
        start("laptop", f'''hostname = "laptop"
instance_name = "chk-laptop"
ipv4 = "10.188.16.10/16"
dhcp = false
listeners = []
socks5_proxy = "socks5://127.0.0.1:{sm}"

[network_identity]
network_name = "{NETWORK}"

[secure_mode]
enabled = true
local_private_key = "{cred['credential_secret']}"

[[peer]]
uri = "tcp://127.0.0.1:{pa}"

[flags]
no_tun = true
private_mode = true
''', rm)
        peers_have = lambda ip: any(str(p.get("ipv4", "")).split("/")[0] == ip for p in cli_json(str(cli), ra, "peer"))  # noqa: E731
        check(bool(wait_for(lambda: peers_have("10.188.16.10"))), "成员 laptop 用 issuer 签的凭据入网")

        wsrv, wport = serve(FakeWorker)
        mport = free_port()
        state = tmp / "manager"
        state.mkdir()
        (state / "settings.json").write_text(json.dumps({
            "pair": {"ttl_days": 30, "code_minutes": 15, "peers": ["issuer"], "transport": "tcp", "probe": True},
            "device": {"latency_first": True, "enable_kcp_proxy": False, "udp_peers": False}}))
        (state / "policy.json").write_text(json.dumps({"per_day": 2}))
        cfg = {
            "network": {"name": NETWORK, "cidr": "10.188.0.0/16", "sni": "speedtest.example.com", "edge_zone": "edge.example.com",
                        "issuer": "issuer", "console": ""},
            "relays": [{"name": "issuer", "overlay_ip": "10.188.0.14", "eip": "127.0.0.1", "et_port": pa, "wss_port": 443}],
            "base_url": f"http://127.0.0.1:{wport}",
            "network_id": "chk",
        }
        (tmp / "config.json").write_text(json.dumps(cfg))
        (tmp / "manager.json").write_text(json.dumps({"manager_key": MANAGER_KEY, "devid_key": "22" * 32}))
        mlog = open(tmp / "manager.log", "w")
        procs.append(subprocess.Popen([sys.executable, str(MANAGER), "--config", str(tmp / "config.json"), "--secrets", str(tmp / "manager.json"),
                                       "--state", str(state), "--rpc", f"127.0.0.1:{ra}", "--cli", str(cli),
                                       "--listen", f"127.0.0.1:{mport}", "--allow-host", "manager.test"],
                                      stdout=mlog, stderr=subprocess.STDOUT))
        client = ["10.188.16.10"]
        nsrv, nport = serve(nginx(mport, client))
        wait_for(lambda: socket.create_connection(("127.0.0.1", mport), timeout=1).close() or True, 10)

        def invite(kind: str) -> subprocess.CompletedProcess:
            # 管理面地址按 install.sh 记下的写法：https 域名 @ overlay 地址；测试里是 http，--resolve 把 manager.test 钉到回环
            return subprocess.run([binary, "invite", "--kind", kind, "--manager", f"http://manager.test:{nport}@127.0.0.1", "--json"],
                                  capture_output=True, text=True, timeout=90)

        r = invite("userspace")
        out = json.loads(r.stdout) if r.returncode == 0 else {}
        check(r.returncode == 0 and out.get("ip") == "10.188.32.1" and out.get("kind") == "userspace",
              f"lolinet invite --kind userspace：{out.get('name')} {out.get('ip')}（{r.stderr.strip()[-200:]}）")
        check(FakeWorker.bad_sig == 0 and len(FakeWorker.blobs) == 1, "管理面经 Worker 签名写进了一份配对包")
        code = out.get("code", "").replace("-", "")
        check("LOLINET_USERSPACE=1" in out.get("command", ""), f"给新机器的命令带无 TUN：{out.get('command')}")

        # 新机器：照 install.sh 的参数解密
        blob = FakeWorker.blobs.get(hashlib.sha256(code.encode()).hexdigest()[:32], "")
        dec = subprocess.run(["openssl", "enc", "-d", "-aes-256-cbc", "-pbkdf2", "-iter", "200000", "-md", "sha256", "-a", "-A",
                              "-pass", "env:K"], input=blob, capture_output=True, text=True, env={**os.environ, "K": code})
        conf = dec.stdout
        check(dec.returncode == 0 and "no_tun = true" in conf and "socks5_proxy" in conf and "# probe = " in conf
              and "# lolinet = " in conf and '"manager":"https://manager.test"' in conf,
              "配对包能用配对码解开，带 no_tun、socks5_proxy、探针和管理面地址")
        new_ip = out.get("ip", "")
        start("newbie", conf.replace("127.0.0.1:18080", f"127.0.0.1:{sn}"), rn)
        check(bool(wait_for(lambda: peers_have(new_ip), 40)), f"新机器用邀请来的凭据入网：issuer 的节点表里有 {new_ip}")
        creds = json.loads((tmp / "credentials.json").read_text())
        check(any(k.startswith(out.get("name", "?")) for k in creds), "邀请签的凭据写进了 issuer 的 credential_file")
        devs = json.loads((state / "devices.json").read_text())
        check(any(d.get("overlay_ip") == new_ip and d.get("paired_by", "").endswith("@10.188.16.10") for d in devs),
              "设备登记里记了是谁邀请的")

        r = invite("server")
        check(r.returncode != 0 and "403" in r.stderr, f"成员不能邀请 server：{r.stderr.strip()[-120:]}")
        client[0] = "10.188.99.99"
        r = invite("desktop")
        check(r.returncode != 0 and "403" in r.stderr, f"不在路由表里的来源被拒：{r.stderr.strip()[-120:]}")
        client[0] = "10.188.16.10"
        r = invite("desktop")
        out2 = json.loads(r.stdout) if r.returncode == 0 else {}
        check(out2.get("ip") == "10.188.16.1", f"第二个邀请（desktop）分到 desktop 池的第一个：{out2.get('ip')}")
        r = invite("desktop")
        check(r.returncode != 0 and "429" in r.stderr, f"超过每天 2 个被拒：{r.stderr.strip()[-120:]}")

        # 签发节点没有 credential_file（它签的凭据重启就丢）：再起一个管理面，RPC 指向成员 laptop 的内核（它没配）
        m2 = free_port()
        state2 = tmp / "manager2"
        state2.mkdir()
        procs.append(subprocess.Popen([sys.executable, str(MANAGER), "--config", str(tmp / "config.json"), "--secrets", str(tmp / "manager.json"),
                                       "--state", str(state2), "--rpc", f"127.0.0.1:{rm}", "--cli", str(cli),
                                       "--listen", f"127.0.0.1:{m2}", "--allow-host", "manager.test"],
                                      stdout=mlog, stderr=subprocess.STDOUT))
        wait_for(lambda: socket.create_connection(("127.0.0.1", m2), timeout=1).close() or True, 10)
        n2srv, n2port = serve(nginx(m2, client))
        r = subprocess.run([binary, "invite", "--manager", f"http://manager.test:{n2port}@127.0.0.1"], capture_output=True, text=True, timeout=90)
        check(r.returncode != 0 and "503" in r.stderr and "credential_file" in r.stderr, f"签发节点没有 credential_file 就不签：{r.stderr.strip()[-120:]}")
        for srv in (nsrv, n2srv, wsrv):
            srv.shutdown()
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()
        if FAILS:
            for f in ("issuer.log", "manager.log", "newbie.log"):
                if (tmp / f).exists():
                    print(f"--- {f}\n" + (tmp / f).read_text()[-1500:])
        shutil.rmtree(tmp, ignore_errors=True)
    if FAILS:
        raise SystemExit(f"{len(FAILS)} 项没过")
    print("全部通过")


if __name__ == "__main__":
    main()
