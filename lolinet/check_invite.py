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
  最后一个是直连）和规则集；lolinet mihomo 出电脑的配置（tun / userspace 两种），YAML 能解析（有 PyYAML 或 ruby 时）；
- 给了 MIHOMO：真的 mihomo 用手机配置经 WireGuard 握手进网，连到成员 laptop 上的一个 HTTP 服务。

用法: [MIHOMO=<mihomo 可执行文件>] python3 lolinet/check_invite.py <lolinet 二进制>
"""
from __future__ import annotations

import base64
import hashlib
import hmac
import http.client
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


# 测试里的 HTTP 都在回环上：不经代理（urllib 默认读环境变量里的代理，macOS 上还读系统的代理设置）
DIRECT = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http_up(port: int) -> bool:
    """回环上这个端口回 HTTP 了（什么状态码都算）"""
    try:
        with DIRECT.open(f"http://127.0.0.1:{port}/", timeout=3):
            return True
    except urllib.error.HTTPError:
        return True


def nginx(upstream: int, client_ip: list):
    """替 nginx：Host 原样，X-Forwarded-For 设成 client_ip[0]（线上是 $remote_addr，成员的 overlay 地址）。
    连管理面这一步连不上就重试（短超时，打出来）；连上以后请求只发一次：POST 重发会多发一个邀请"""

    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            self.do_POST(method="GET")

        def do_POST(self, method="POST"):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0)) if method == "POST" else None
            headers = {k: v for k, v in self.headers.items() if k.lower() not in ("content-length", "x-forwarded-for", "connection")}
            headers["X-Forwarded-For"] = client_ip[0]
            conn = None
            for attempt in range(1, 5):
                c = http.client.HTTPConnection("127.0.0.1", upstream, timeout=10)
                try:
                    c.connect()
                    conn = c
                    break
                except OSError as e:
                    c.close()
                    print(f"  （替 nginx：第 {attempt} 次连不上管理面 127.0.0.1:{upstream}：{e}）", flush=True)
                    time.sleep(1)
            if conn is None:
                self.send_error(502, "manager unreachable")
                return
            conn.sock.settimeout(60)
            try:
                conn.request(method, self.path, body=body, headers=headers)
                r = conn.getresponse()
                status, data = r.status, r.read()
            finally:
                conn.close()
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


def route_key(name: str) -> str:
    """和 web/agent/mesh_manager.py 的 route_key 一样"""
    return "r" + hashlib.sha256(name.encode()).hexdigest()[:8]


def t_order(text: str) -> bool:
    keys = ["PROCESS-NAME,lolinet,DIRECT", "IP-CIDR,10.188.0.0/16,lolinet", "RULE-SET,lolinet-reject,REJECT",
            "RULE-SET,lolinet-direct,DIRECT", "RULE-SET,lolinet-exit,", "MATCH,DIRECT"]
    pos = [text.find(k) for k in keys]
    return all(p >= 0 for p in pos) and pos == sorted(pos)


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


def mihomo_over_wireguard(mihomo: str | None, phone_yaml: str, tmp: Path, procs: list) -> None:
    """真的 mihomo 用手机那份配置进网：WireGuard 握手到 issuer 的入口，经网里转到成员 laptop（userspace，
    overlay 地址上的连接转给它本机的 127.0.0.1），拿到那边一个 HTTP 服务的回应。手机上的客户端（Clash Meta、
    ClashMi 这些）用的就是这个内核，这里只是不开 TUN、从 mixed 端口进。给了 MIHOMO（可执行文件）才跑。"""
    if not mihomo:
        print("  （没给 MIHOMO，跳过真 mihomo 经 WireGuard 进网；CI 里下钉住的那版来跑）")
        return
    marker = b"lolinet-over-wireguard-" + os.urandom(4).hex().encode()

    class Hello(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Length", str(len(marker)))
            self.end_headers()
            self.wfile.write(marker)

    _, hport = serve(Hello)
    mp = free_port()
    home = tmp / "mihomo"
    home.mkdir()
    # 只换 mixed 端口（7890 可能被占）；配置别的地方原样：手机配置本来就不带 TUN（手机的 app 自己开），DNS 不监听端口
    conf = phone_yaml.replace("mixed-port: 7890", f"mixed-port: {mp}").replace("log-level: warning", "log-level: info")
    (home / "config.yaml").write_text(conf)
    log = open(home / "mihomo.log", "w")
    procs.append(subprocess.Popen([mihomo, "-d", str(home), "-f", str(home / "config.yaml")], stdout=log, stderr=subprocess.STDOUT))
    up = wait_for(lambda: socket.create_connection(("127.0.0.1", mp), timeout=1).close() or True, 20)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({"http": f"http://127.0.0.1:{mp}"}))

    def fetch():
        with opener.open(f"http://10.188.16.10:{hport}/", timeout=5) as r:
            return r.read()

    body = wait_for(fetch, 40) if up else None
    check(body == marker,
          f"真 mihomo（{subprocess.run([mihomo, '-v'], capture_output=True, text=True).stdout.split(chr(10))[0][:60]}）"
          f"用手机配置经 WireGuard 进网，连到成员 laptop 的 10.188.16.10:{hport}")
    if body != marker:
        log.flush()
        print("--- mihomo.log\n" + (home / "mihomo.log").read_text()[-3000:])


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
bind_device = false
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
                                                       "exit": ["DOMAIN-SUFFIX,openai.com"], "exit_order": ["laptop"],
                                                       "routes": [{"name": "办公室", "exit": "laptop", "rules": ["DOMAIN-SUFFIX,web.com"]},
                                                                  {"name": "美国", "exit": "nobox", "rules": ["DOMAIN-SUFFIX,example.org"]}]}))
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
        # 管理面回 HTTP 了才往下走；起不来就停在这里，别让后面一串都跟着错
        if not wait_for(lambda: http_up(mport), 30):
            mlog.flush()
            raise SystemExit("管理面 30 秒没起来：" + (tmp / "manager.log").read_text()[-2000:])

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
        # bind_device = false：Linux 上 EasyTier 默认把出站 socket 绑到默认网卡，连不到回环上的 issuer（测试才要）
        start("newbie", conf.replace("127.0.0.1:18080", f"127.0.0.1:{sn}").replace("[flags]\n", "[flags]\nbind_device = false\n", 1), rn)
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
        mihomo_over_wireguard(os.environ.get("MIHOMO"), phone_yaml, tmp, procs)

        def get(path: str, src: str) -> tuple:
            client[0] = src
            req = urllib.request.Request(f"http://127.0.0.1:{nport}{path}", method="GET", headers={"Host": "manager.test"})
            try:
                with DIRECT.open(req, timeout=30) as resp:
                    return resp.status, resp.read().decode()
            except urllib.error.HTTPError as e:
                return e.code, e.read().decode()

        def post(path: str, src: str, obj: dict) -> int:
            client[0] = src
            req = urllib.request.Request(f"http://127.0.0.1:{nport}{path}", data=json.dumps(obj).encode(), method="POST",
                                         headers={"Host": "manager.test", "X-Mesh-Agent": "1", "Content-Type": "application/json"})
            try:
                with DIRECT.open(req, timeout=30) as resp:
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
        tun, us = outs["tun"], outs["userspace"]
        check("route-exclude-address" in tun and "type: socks5, server: 127.0.0.1, port: 18080" in tun
              and "IP-CIDR,10.188.0.0/16,lolinet,no-resolve" in tun and "proxy: lolinet" in tun and "via=lolinet" in tun,
              "lolinet mihomo --mode tun：mihomo 的 TUN 让开 overlay 网段；它自己拨的（出口、拉规则）经 lolinet 的回环 SOCKS5")
        check(all("dns-hijack: [any:53, tcp://any:53]" in t and "sniffer:" in t and "PROCESS-NAME,lolinet,DIRECT" in t
                  and "IP-CIDR,127.0.0.1/32,DIRECT,no-resolve" in t for t in (tun, us))
              and "tun:" in us and "route-exclude-address" not in us,
              "电脑两种配置都劫持 DNS、开 sniffer，lolinet 的底层流量（进程、relay 公网地址）最先直连；userspace 下 mihomo 是唯一的 TUN")
        check(t_order(tun), "规则顺序：底层直连 → 网内 → 拒绝 → 直连 → 出口 → 其余直连")

        # ---- 线路：一组规则固定从某台出口出去 ----
        office, usa = route_key("办公室"), route_key("美国")
        for t in (tun, us, phone_yaml):
            ok = (f"RULE-SET,lolinet-{office},线路·办公室" in t and f"RULE-SET,lolinet-{usa},线路·美国" in t
                  and t.index(f"RULE-SET,lolinet-{office}") < t.index("RULE-SET,lolinet-exit,")
                  and t.index("RULE-SET,lolinet-direct,DIRECT") < t.index(f"RULE-SET,lolinet-{office}")
                  and f"/api/mesh/mihomo/route/{office}/exit.yaml?via=lolinet" in t)
            if not ok:
                break
        check(ok, "电脑和手机的配置里每条线路一个规则集和组，排在拒绝 / 直连之后、通用出口之前")
        code_, body = get(f"/api/mesh/mihomo/route/{office}/rules.yaml", "10.188.48.1")
        check(code_ == 200 and '"DOMAIN-SUFFIX,web.com"' in body and yaml_ok(body), "线路「办公室」的规则从内网取")
        code_, body = get(f"/api/mesh/mihomo/route/{office}/exit.yaml?via=lolinet", "10.188.48.1")
        check(code_ == 200 and '"线路·办公室 → laptop"' in body and 'server: "10.188.16.10"' in body and "port: 10180" in body
              and 'dialer-proxy: "lolinet"' in body and yaml_ok(body), f"线路「办公室」的出口是 laptop（{body.strip()[:120]}）")
        code_, body = get(f"/api/mesh/mihomo/route/{usa}/exit.yaml?via=lolinet", "10.188.48.1")
        check(code_ == 200 and "nobox 没开出口" in body and 'server: "127.0.0.1"' in body and "port: 9" in body,
              "线路「美国」的节点没开出口：给一个连不通的地址，不变成直连")
        code_, body = get("/api/mesh/mihomo/route/r00000000/rules.yaml", "10.188.48.1")
        code2, body2 = get("/api/mesh/mihomo/route/r00000000/exit.yaml", "10.188.48.1")
        check(code_ == 200 and body.strip() == "payload: []" and "线路已删除" in body2,
              "删掉的线路：老配置取到空规则和连不通的出口")
        code_, _ = get(f"/api/mesh/mihomo/route/{office}/exit.yaml", "10.188.99.98")
        check(code_ == 403, "网外来源取不到线路")
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
