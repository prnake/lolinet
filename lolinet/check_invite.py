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
- 签发节点没有 credential_file 时拒绝（503）；
- 手机：lolinet invite --kind phone 在 issuer 的 WireGuard 入口加了客户端（10.188.48.1），配对包是 mihomo 配置
  （wireguard 出站：地址、密钥、服务器和端口都对上入口给的）；
- mihomo 集中下发：成员登记成出口，手机（经 WireGuard 来的地址）取到出口列表（socks5，dialer-proxy 是 lolinet，
  最后一个是直连）和规则集；lolinet mihomo 出电脑的配置（tun / userspace 两种），YAML 能解析（有 PyYAML 或 ruby 时）。

用法: python3 lolinet/check_invite.py <lolinet 二进制>
"""
from __future__ import annotations

import base64
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

        def do_GET(self):
            self.do_POST(method="GET")

        def do_POST(self, method="POST"):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0)) if method == "POST" else None
            headers = {k: v for k, v in self.headers.items() if k.lower() not in ("content-length", "x-forwarded-for", "connection")}
            headers["X-Forwarded-For"] = client_ip[0]
            req = urllib.request.Request(f"http://127.0.0.1:{upstream}{self.path}", data=body, method=method, headers=headers)
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


def yaml_ok(text: str) -> bool:
    """有 PyYAML 用它，没有就用 ruby 的；都没有算过（只少一项语法检查）"""
    try:
        import yaml  # noqa: PLC0415
        return isinstance(yaml.safe_load(text), dict)
    except ImportError:
        pass
    if shutil.which("ruby"):
        r = subprocess.run(["ruby", "-ryaml", "-e", "exit(YAML.safe_load(STDIN.read).is_a?(Hash) ? 0 : 1)"], input=text, text=True,
                           capture_output=True)
        return r.returncode == 0
    print("  （没有 PyYAML 也没有 ruby，跳过 YAML 语法检查）")
    return True


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
    pa, ra, rm, rn, sm, sn, pw = (free_port() for _ in range(7))
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

[vpn_portal_config]
wireguard_listen = "127.0.0.1:{pw}"
wireguard_private_key = "{base64.b64encode(os.urandom(32)).decode()}"

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
        (state / "policy.json").write_text(json.dumps({"per_day": 3}))
        (state / "mihomo.json").write_text(json.dumps({"reject": ["DOMAIN-SUFFIX,ads.example.com"], "direct": ["GEOIP,CN"],
                                                       "exit": ["DOMAIN-SUFFIX,openai.com"], "exit_order": ["laptop"]}))
        cfg = {
            "network": {"name": NETWORK, "cidr": "10.188.0.0/16", "sni": "speedtest.example.com", "edge_zone": "edge.example.com",
                        "issuer": "issuer", "console": "", "local_suffix": "-local.edge.example.com"},
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

        # ---- 手机：WireGuard 入口 + mihomo 配置 ----
        r = invite("phone")
        ph = json.loads(r.stdout) if r.returncode == 0 else {}
        check(r.returncode == 0 and ph.get("ip") == "10.188.48.1" and ph.get("command", "").endswith("/m#" + ph.get("code", "?")),
              f"lolinet invite --kind phone：{ph.get('name')} {ph.get('ip')}，兑换地址 {ph.get('command')}（{r.stderr.strip()[-200:]}）")
        portal = cli_json(str(cli), ra, "vpn-portal")
        wgc = next((c for c in portal.get("clients", []) if c.get("name") == ph.get("name")), {})
        check(wgc.get("virtual_ip", "").startswith("10.188.48.1"), f"issuer 的 WireGuard 入口里有这台手机：{wgc.get('virtual_ip')}")
        pcode = ph.get("code", "").replace("-", "")
        pblob = FakeWorker.blobs.get(hashlib.sha256(pcode.encode()).hexdigest()[:32], "")
        pdec = subprocess.run(["openssl", "enc", "-d", "-aes-256-cbc", "-pbkdf2", "-iter", "200000", "-md", "sha256", "-a", "-A",
                               "-pass", "env:K"], input=pblob, capture_output=True, text=True, env={**os.environ, "K": pcode})
        phone_yaml = pdec.stdout
        wg_priv = next((ln.split("=", 1)[1].strip() for ln in wgc.get("client_config", "").splitlines() if ln.startswith("PrivateKey")), "?")
        check(pdec.returncode == 0 and "type: wireguard" in phone_yaml and 'ip: "10.188.48.1"' in phone_yaml
              and f'private-key: "{wg_priv}"' in phone_yaml and 'server: "127.0.0.1"' in phone_yaml and f"port: {pw}" in phone_yaml
              and "/api/mesh/mihomo/proxies.yaml?via=lolinet" in phone_yaml and "IP-CIDR,10.188.0.0/16,lolinet,no-resolve" in phone_yaml,
              "手机的配对包是 mihomo 配置：wireguard 出站的地址、私钥、服务器、端口都对，规则和出口经内网取")
        check(yaml_ok(phone_yaml), "手机的 mihomo 配置是合法 YAML")
        if FAILS:
            print(phone_yaml[:1500])

        def get(path: str, src: str) -> tuple:
            client[0] = src
            req = urllib.request.Request(f"http://127.0.0.1:{nport}{path}", method="GET", headers={"Host": "manager.test"})
            try:
                with urllib.request.urlopen(req, timeout=30) as resp:
                    return resp.status, resp.read().decode()
            except urllib.error.HTTPError as e:
                return e.code, e.read().decode()

        def post(path: str, src: str, obj: dict) -> int:
            client[0] = src
            req = urllib.request.Request(f"http://127.0.0.1:{nport}{path}", data=json.dumps(obj).encode(), method="POST",
                                         headers={"Host": "manager.test", "X-Mesh-Agent": "1", "Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=30) as resp:
                    return resp.status
            except urllib.error.HTTPError as e:
                return e.code

        check(post("/api/mesh/exit", "10.188.16.10", {"port": 10180}) == 200, "成员 laptop 登记成出口（lolinet exit on 做的事）")
        code_, prox = get("/api/mesh/mihomo/proxies.yaml?via=lolinet", "10.188.48.1")
        check(code_ == 200 and '"laptop 出口"' in prox and 'server: "10.188.16.10"' in prox and "port: 10180" in prox
              and 'dialer-proxy: "lolinet"' in prox and "type: direct" in prox and prox.index("laptop") < prox.index("type: direct"),
              f"手机取到出口列表：laptop 的 socks5 经 WireGuard 拨，直连在最后（{code_}）")
        check(yaml_ok(prox), "出口列表是合法 YAML")
        code_, rules = get("/api/mesh/mihomo/rules/exit.yaml", "10.188.48.1")
        check(code_ == 200 and '"DOMAIN-SUFFIX,openai.com"' in rules and yaml_ok(rules), "手机取到集中下发的「经出口」规则")
        code_, _ = get("/api/mesh/mihomo/rules/exit.yaml", "10.188.99.98")
        check(code_ == 403, f"网外来源取不到规则（{code_}）")
        client[0] = "10.188.16.10"
        outs = {}
        for mode in ("userspace", "tun"):
            r = subprocess.run([binary, "mihomo", "--mode", mode, "--manager", f"http://manager.test:{nport}@127.0.0.1"],
                               capture_output=True, text=True, timeout=60)
            outs[mode] = r.stdout if r.returncode == 0 else r.stderr
        check("type: socks5, server: 127.0.0.1, port: 18080" in outs["userspace"]
              and "IP-CIDR,10.188.0.0/16,lolinet,no-resolve" in outs["userspace"]
              and "DOMAIN-REGEX,^[a-z0-9-]+\\-local\\.edge\\.example\\.com$,lolinet" in outs["userspace"],
              "lolinet mihomo --mode userspace：网内地址交给本机 SOCKS5")
        check("route-exclude-address" in outs["tun"] and "IP-CIDR,10.188.0.0/16,DIRECT,no-resolve" in outs["tun"] and "proxies: []" in outs["tun"],
              "lolinet mihomo --mode tun：mihomo 的 TUN 让开 overlay 网段，网内地址直连")
        check(all(yaml_ok(v) for v in outs.values()), "两份电脑配置都是合法 YAML")
        if FAILS:
            print(outs["userspace"][:2500])
        check(post("/api/mesh/exit", "10.188.16.10", {"port": 0}) == 200
              and '"laptop 出口"' not in get("/api/mesh/mihomo/proxies.yaml", "10.188.16.10")[1],
              "lolinet exit off 撤下之后出口列表里没有它")
        client[0] = "10.188.16.10"

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
        check(r.returncode != 0 and "429" in r.stderr, f"超过每天 3 个被拒：{r.stderr.strip()[-120:]}")

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
