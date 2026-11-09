#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = []
# ///
"""lo-li mesh 控制台 agent：把本机 easytier-core 的状态和 easytier-web 控制台，经一个本地 HTTP 口交给 web 前端。

  GET  /api/mesh/info       agent 能力：控制台鉴权方式、能不能配对、network_id
  GET  /api/mesh/snapshot   easytier-cli -o json 的 node / peer / route / peer-center（去掉配置文本和公网 IP）
  GET  /api/mesh/devices    $LOLINET_HOME/state/mesh-devices.json（pair.py 写的设备登记，不含密钥）
  POST /api/mesh/pair       调 scripts/pair.py 签凭据、出配对码（启动时要带 --allow-pair）
  POST /api/mesh/forget     从设备登记里删一条（控制台吊销之后调）
  *    /api/v1/...          反代 easytier-web 控制台；写操作只放行白名单；响应里的 *_secret / 私钥一律删掉
  GET  其他                  web/dist 静态文件

默认只听 127.0.0.1。防 DNS rebinding：Host 必须是 127.0.0.1 / localhost（或 --allow-host 里的名字）；
非 GET 请求要带 X-Mesh-Agent: 1，跨站页面带不上这个头。--listen 到局域网地址时强制只读（同网段谁都能自己带这个头）。
能读到主仓库 state/mesh-console.json 时由 agent 自己登录控制台，浏览器不碰控制台密码。

用法:
  python3 web/agent/mesh_agent.py                                   # 读 127.0.0.1:15888（TUN 版系统服务）
  python3 web/agent/mesh_agent.py --rpc 127.0.0.1:15897 --socks 127.0.0.1:18080 --allow-pair
  python3 web/agent/mesh_agent.py --listen <局域网 IP>:10188 --socks 127.0.0.1:18080 ...   # 局域网只读
  python3 web/agent/mesh_agent.py --dump > web/src/data/demo-snapshot.json   # 导出演示数据
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import hashlib
import http.client
import http.server
import json
import mimetypes
import os
import re
import shutil
import socket
import ssl
import struct
import subprocess
import sys
import threading
import time
from pathlib import Path
from urllib.parse import urlsplit

VERSION = "0.1.0"
WEB_DIR = Path(__file__).resolve().parent.parent
MESH_DIR = WEB_DIR.parent
# 运行时数据（.env、state/）：LOLINET_HOME，兼容旧的 FLEET_REPO，默认 ~/.config/lolinet
HOME_DIR = Path(os.environ.get("LOLINET_HOME") or os.environ.get("FLEET_REPO") or Path.home() / ".config/lolinet")
STATE_DIR = HOME_DIR / "state"
SECRET_KEYS = {"credential_secret", "network_secret", "local_private_key", "private_key", "secret"}


def nodes_text() -> str:
    """节点清单：LOLINET_NODES，否则仓库根的 nodes.yaml，没有就用 nodes.example.yaml（只用标准库，按行找字段）。"""
    p = Path(os.environ.get("LOLINET_NODES") or MESH_DIR / "nodes.yaml")
    return (p if p.is_file() else MESH_DIR / "nodes.example.yaml").read_text()


def nodes_field(key: str, value: str = r"[^\s#]+") -> str | None:
    m = re.search(rf"^\s+{key}:\s*({value})", nodes_text(), re.M)
    return m.group(1) if m else None


def issuer_ip() -> str | None:
    """签发凭据、跑控制台的那台 relay（network.issuer）的 overlay IP"""
    issuer = nodes_field("issuer")
    m = issuer and re.search(rf"\{{name:\s*{re.escape(issuer)},.*?overlay_ip:\s*([\d.]+)", nodes_text())
    return m.group(1) if m else None


# 准入站：LOLINET_BASE_URL，不设就是节点清单的 network.join
JOIN_HOST = urlsplit(os.environ.get("LOLINET_BASE_URL") or f"https://{nodes_field('join')}").hostname
# 经 agent 代理的控制台写操作白名单；其他写操作去官方控制台做。
WRITE_ALLOW = [
    ("POST", re.compile(r"^/api/v1/auth/login$")),
    ("DELETE", re.compile(r"^/api/v1/networks/[0-9a-f-]{36}/credentials/[A-Za-z0-9._:-]{1,128}$")),
]
# 反代的读操作也只放前端用到的；sessions、成员配置这类带 token 的接口不经 agent
READ_ALLOW = re.compile(r"^/api/v1/(auth/check_login_status|networks|networks/[0-9a-f-]{36}(/credentials)?)$")
LOOPBACK = {"127.0.0.1", "localhost", "::1"}


def hostport(s: str) -> tuple[str, int]:
    h, p = s.rsplit(":", 1)
    return h.strip("[]"), int(p)


def scrub(x):
    """递归删掉密钥字段。"""
    if isinstance(x, dict):
        return {k: scrub(v) for k, v in x.items() if k not in SECRET_KEYS}
    if isinstance(x, list):
        return [scrub(v) for v in x]
    return x


# ---------- easytier-cli ----------

def cli(args, sub: list[str]):
    p = subprocess.run([args.cli, "-p", args.rpc, "-o", "json", *sub], capture_output=True, text=True, timeout=10)
    if p.returncode != 0:
        msg = (p.stderr or p.stdout).strip().splitlines()
        raise RuntimeError(msg[-1][:300] if msg else f"exit {p.returncode}")
    return json.loads(p.stdout)


def slim_node(n: dict | None) -> dict | None:
    """node 的输出带完整配置（含 secure_mode 私钥）和公网 IP，只留前端要的字段。"""
    if not n:
        return None
    conf = n.get("config") or ""
    flag = lambda k: bool(re.search(rf"^{k}\s*=\s*true\s*$", conf, re.M))  # noqa: E731
    socks = re.search(r'^socks5_proxy\s*=\s*"([^"]+)"', conf, re.M)
    stun = n.get("stun_info") or {}
    return {
        "peer_id": str(n.get("peer_id", "")),
        "hostname": n.get("hostname"),
        "ipv4_addr": n.get("ipv4_addr"),
        "version": n.get("version"),
        "udp_nat_type": stun.get("udp_nat_type"),
        "tcp_nat_type": stun.get("tcp_nat_type"),
        "listeners": [x for x in n.get("listeners") or [] if not x.startswith("ring://")],
        "peer_uris": re.findall(r'^uri\s*=\s*"([^"]+)"', conf, re.M),
        "socks5": socks.group(1) if socks else None,
        "no_tun": flag("no_tun"),
        "latency_first": flag("latency_first"),
        "private_mode": flag("private_mode"),
        "secure_mode": "[secure_mode]" in conf,
        "credential_peer": bool((n.get("feature_flag") or {}).get("is_credential_peer")),
    }


class Snapshotter:
    TTL = 1.5  # 多个标签页同时轮询时合并成一次 cli 调用

    def __init__(self, args):
        self.args = args
        self.lock = threading.Lock()
        self.at = 0.0
        self.data: dict | None = None

    def get(self) -> dict:
        with self.lock:
            if self.data is None or time.time() - self.at > self.TTL:
                self.data, self.at = self.take(), time.time()
            return self.data

    def take(self) -> dict:
        jobs = {"node": ["node"], "peers": ["peer"], "routes": ["route"], "graph": ["peer-center"]}
        res: dict = {}
        errors = []
        with cf.ThreadPoolExecutor(len(jobs)) as ex:
            futs = {k: ex.submit(cli, self.args, v) for k, v in jobs.items()}
            for k, f in futs.items():
                try:
                    res[k] = f.result()
                except Exception as e:  # noqa: BLE001
                    res[k] = None if k == "node" else []
                    errors.append(f"{k}: {e}")
        return {
            "fetched_at": int(time.time()),
            "rpc": self.args.rpc,
            "node": slim_node(res["node"]),
            "peers": res["peers"],
            "routes": res["routes"],
            "graph": res["graph"],
            "errors": errors,
        }


# ---------- 控制台反代 ----------

def recvn(s: socket.socket, n: int) -> bytes:
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise OSError("socks5: connection closed")
        buf += chunk
    return buf


def socks5(proxy: tuple[str, int], host: str, port: int) -> socket.socket:
    s = socket.create_connection(proxy, timeout=20)
    s.sendall(b"\x05\x01\x00")
    if recvn(s, 2) != b"\x05\x00":
        raise OSError("socks5: no-auth refused")
    try:
        addr = b"\x01" + socket.inet_aton(host)
    except OSError:
        hb = host.encode()
        addr = b"\x03" + bytes([len(hb)]) + hb
    s.sendall(b"\x05\x01\x00" + addr + struct.pack(">H", port))
    rep = recvn(s, 4)
    if rep[1] != 0:
        raise OSError(f"socks5: connect failed (code {rep[1]})")
    skip = {1: 4, 4: 16}.get(rep[3]) or recvn(s, 1)[0]
    recvn(s, skip + 2)
    return s


class Console:
    """easytier-web REST。https 走 --console-ip 直连（TUN）或 --socks（no-tun 内核）；SNI 和证书校验用 URL 里的域名。"""

    def __init__(self, args):
        u = urlsplit(args.console)
        self.https = u.scheme == "https"
        self.host = u.hostname or "127.0.0.1"
        self.port = u.port or (443 if self.https else 80)
        self.connect_ip = args.console_ip or self.host
        self.socks = hostport(args.socks) if args.socks else None
        self.creds = None
        if args.console_state and Path(args.console_state).is_file():
            try:
                self.creds = json.loads(Path(args.console_state).read_text())
            except (OSError, ValueError):
                self.creds = None
        self.cookie: str | None = None
        self.lock = threading.Lock()
        self.ctx = ssl.create_default_context()

    @property
    def auth(self) -> str:
        return "agent" if self.creds else "browser"

    def _do(self, method: str, path: str, body: bytes | None, headers: dict) -> tuple[int, list, bytes]:
        if self.socks:
            sock = socks5(self.socks, self.connect_ip, self.port)
        else:
            sock = socket.create_connection((self.connect_ip, self.port), timeout=20)
        conn_cls = http.client.HTTPSConnection if self.https else http.client.HTTPConnection
        conn = conn_cls(self.host, self.port, timeout=20)
        conn.sock = self.ctx.wrap_socket(sock, server_hostname=self.host) if self.https else sock
        try:
            conn.request(method, path, body=body, headers=headers)
            r = conn.getresponse()
            return r.status, r.getheaders(), r.read()
        finally:
            conn.close()

    def login(self) -> None:
        body = json.dumps({"username": self.creds["admin_user"],
                           "password": hashlib.md5(self.creds["admin_pw"].encode()).hexdigest()}).encode()
        status, rh, data = self._do("POST", "/api/v1/auth/login", body, {"Content-Type": "application/json"})
        if status != 200:
            raise OSError(f"console login {status}: {data[:200]!r}")
        for k, v in rh:
            if k.lower() == "set-cookie":
                self.cookie = v.split(";", 1)[0]

    def request(self, method: str, path: str, body: bytes | None, ctype: str | None, browser_cookie: str | None):
        headers = {"Accept": "application/json"}
        if ctype:
            headers["Content-Type"] = ctype
        if self.creds:
            with self.lock:
                if not self.cookie:
                    self.login()
                headers["Cookie"] = self.cookie or ""
        elif browser_cookie:
            headers["Cookie"] = browser_cookie
        status, rh, data = self._do(method, path, body, headers)
        if status == 401 and self.creds:
            with self.lock:
                self.login()
                headers["Cookie"] = self.cookie or ""
            status, rh, data = self._do(method, path, body, headers)
        return status, rh, data


# ---------- 配对 / 设备登记 ----------

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,30}$")
IP_RE = re.compile(r"^10\.188\.(\d{1,3})\.(\d{1,3})$")


def registry_path() -> Path:
    return STATE_DIR / "mesh-devices.json"


def pair_capable(args) -> bool:
    return bool(args.allow_pair and not args.read_only and shutil.which("uv") and (MESH_DIR / "scripts/pair.py").is_file()
                and (HOME_DIR / ".env").is_file() and (STATE_DIR / "mesh-console.json").is_file())


def do_pair(args, req: dict) -> tuple[int, dict]:
    name, ip = str(req.get("name", "")), str(req.get("ip", ""))
    try:
        ttl = int(req.get("ttl_days", 365))
    except (TypeError, ValueError):
        return 400, {"message": "有效期要是整数天"}
    peers = [str(p) for p in req.get("peers") or []]
    m = IP_RE.match(ip)
    if not NAME_RE.match(name):
        return 400, {"message": "设备名只能用小写字母、数字和 -，32 个字符以内"}
    if not m or not all(0 <= int(x) <= 255 for x in m.groups()) or m.group(2) in ("0", "255"):
        return 400, {"message": "地址必须在 10.188.0.0/16 里"}
    if not 1 <= ttl <= 365:
        return 400, {"message": "有效期 1–365 天"}
    if not all(re.match(r"^[a-z]+$", p) for p in peers):
        return 400, {"message": "接入点名不合法"}
    cmd = ["uv", "run", "--quiet", str(MESH_DIR / "scripts/pair.py"), "new",
           "--name", name, "--ip", ip, "--ttl-days", str(ttl)]
    if peers:
        cmd += ["--peers", ",".join(peers)]
    if req.get("probe") is False:  # 默认装公开探针
        cmd += ["--no-probe"]
    if args.socks:
        cmd += ["--socks", args.socks]
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
    code = re.search(r"\b([A-Z0-9]{4}-[A-Z0-9]{4}-[A-Z0-9]{4})\b", p.stdout)
    if p.returncode != 0 or not code:
        tail = (p.stderr or p.stdout).strip().splitlines()[-3:]
        return 502, {"message": "pair.py 失败：" + " / ".join(tail)[:400]}
    cid = re.search(r"凭据 (\S+?)，", p.stdout)
    return 200, {
        "name": name,
        "ip": ip,
        "code": code.group(1),
        "credential_id": cid.group(1) if cid else None,
        "command": f"curl -fsSL https://{JOIN_HOST}/i | sudo sh -s {code.group(1)}",
        "expires_at": int(time.time()) + 900,
    }


def do_forget(req: dict) -> tuple[int, dict]:
    cid = str(req.get("credential_id", ""))
    reg = registry_path()
    if not reg.is_file():
        return 404, {"message": "没有设备登记文件"}
    devices = json.loads(reg.read_text())
    left = [d for d in devices if d.get("credential_id") != cid]
    reg.write_text(json.dumps(left, ensure_ascii=False, indent=2) + "\n")
    os.chmod(reg, 0o600)
    return 200, {"removed": len(devices) - len(left)}


# ---------- HTTP ----------

class Handler(http.server.BaseHTTPRequestHandler):
    server_version = f"mesh-agent/{VERSION}"
    args: argparse.Namespace
    snap: Snapshotter
    console: Console

    def log_message(self, fmt, *a):  # 只记 API 调用
        if "/api/" in (self.path or ""):
            sys.stderr.write(f"{time.strftime('%H:%M:%S')} {self.command} {self.path.split('?')[0]} {a[1] if len(a) > 1 else ''}\n")

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_DELETE(self):
        self.route("DELETE")

    def do_PATCH(self):
        self.route("PATCH")

    def do_PUT(self):
        self.route("PUT")

    def host_ok(self) -> bool:
        host = (self.headers.get("Host") or "").rsplit(":", 1)[0].strip("[]").lower()
        return host in {*LOOPBACK, *self.args.allow_host}

    def body(self) -> bytes:
        n = int(self.headers.get("Content-Length") or 0)
        if n > 64 * 1024:
            raise ValueError("body too large")
        return self.rfile.read(n) if n else b""

    def json_body(self) -> dict | None:
        try:
            v = json.loads(self.body() or b"{}")
        except ValueError:
            return None
        return v if isinstance(v, dict) else None

    def send(self, status: int, data: bytes, ctype: str, extra: list | None = None):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("X-Content-Type-Options", "nosniff")
        for k, v in extra or []:
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def send_json(self, status: int, obj, extra: list | None = None):
        self.send(status, json.dumps(obj, ensure_ascii=False).encode(), "application/json; charset=utf-8",
                  [("Cache-Control", "no-store"), *(extra or [])])

    def route(self, method: str):
        if not self.host_ok():
            return self.send_json(421, {"message": "Host not allowed"})
        path = urlsplit(self.path).path
        if method != "GET" and self.headers.get("X-Mesh-Agent") != "1":
            return self.send_json(403, {"message": "missing X-Mesh-Agent header"})
        # 只读：局域网里谁都能自己带上 X-Mesh-Agent，所以签凭据、吊销、删登记一律不放（登录只换 cookie，不改东西）
        if method != "GET" and self.args.read_only and not path.startswith("/api/v1/auth/login"):
            return self.send_json(403, {"message": "只读模式：配对和吊销请在这台机器上用 127.0.0.1 打开"})
        try:
            if path.startswith("/api/v1/"):
                return self.proxy(method)
            if path.startswith("/api/mesh/"):
                return self.mesh(method, path[len("/api/mesh/"):])
            if method == "GET":
                return self.static(path)
            return self.send_json(405, {"message": "method not allowed"})
        except Exception as e:  # noqa: BLE001
            return self.send_json(502, {"message": f"{type(e).__name__}: {e}"})

    def mesh(self, method: str, what: str):
        a = self.args
        if method == "GET" and what == "info":
            creds = self.console.creds or {}
            return self.send_json(200, {
                "agent": VERSION,
                "rpc": a.rpc,
                "socks": a.socks,
                "console": {"url": a.console, "auth": self.console.auth},
                "network_id": creds.get("network_id"),
                "pair": pair_capable(a),
                "read_only": a.read_only,
                "registry": registry_path().is_file(),
                "join": JOIN_HOST,
            })
        if method == "GET" and what == "snapshot":
            return self.send_json(200, self.snap.get())
        if method == "GET" and what == "devices":
            reg = registry_path()
            return self.send_json(200, {"devices": json.loads(reg.read_text()) if reg.is_file() else []})
        if method == "POST" and what == "pair":
            if not pair_capable(a):
                return self.send_json(501, {"message": "这个 agent 没开配对（启动时加 --allow-pair，且要能读到主仓库 .env 和 state）"})
            req = self.json_body()
            return self.send_json(*do_pair(a, req)) if req is not None else self.send_json(400, {"message": "body 不是 JSON 对象"})
        if method == "POST" and what == "forget":
            req = self.json_body()
            return self.send_json(*do_forget(req)) if req is not None else self.send_json(400, {"message": "body 不是 JSON 对象"})
        return self.send_json(404, {"message": "not found"})

    def proxy(self, method: str):
        path = self.path
        bare = urlsplit(path).path
        if method == "GET" and not READ_ALLOW.match(bare):
            return self.send_json(403, {"message": "agent 不代理这个接口，请到官方控制台查看"})
        if method != "GET" and not any(m == method and rx.match(bare) for m, rx in WRITE_ALLOW):
            return self.send_json(403, {"message": "agent 不代理这个写操作，请到官方控制台操作"})
        body = self.body() if method in ("POST", "PUT", "PATCH") else None
        status, rh, data = self.console.request(method, path, body, self.headers.get("Content-Type"),
                                                self.headers.get("Cookie"))
        ctype = next((v for k, v in rh if k.lower() == "content-type"), "application/octet-stream")
        extra = [("Cache-Control", "no-store")]
        if self.console.auth == "browser":  # 浏览器自己登录：把 cookie 改成对 127.0.0.1 生效
            for k, v in rh:
                if k.lower() == "set-cookie":
                    v = re.sub(r";\s*(Domain=[^;]*|Secure)", "", v, flags=re.I)
                    extra.append(("Set-Cookie", v))
        if "json" in ctype and data:
            try:
                data = json.dumps(scrub(json.loads(data)), ensure_ascii=False).encode()
            except ValueError:
                pass
        self.send(status, data, ctype, extra)

    def static(self, path: str):
        root = Path(self.args.static).resolve()
        rel = path.lstrip("/") or "index.html"
        f = (root / rel).resolve()
        if root not in f.parents and f != root or not f.is_file():
            f = root / "index.html"
        if not f.is_file():
            return self.send(404, "web/dist 不存在，先 npm run build\n".encode(), "text/plain; charset=utf-8")
        ctype = mimetypes.guess_type(f.name)[0] or "application/octet-stream"
        if ctype.startswith("text/") or ctype.endswith(("javascript", "json", "svg+xml")):
            ctype += "; charset=utf-8"
        cache = "public, max-age=31536000, immutable" if "/assets/" in f.as_posix() else "no-cache"
        self.send(200, f.read_bytes(), ctype, [("Cache-Control", cache)])


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--listen", default="127.0.0.1:10188")
    ap.add_argument("--rpc", default="127.0.0.1:15888", help="easytier-core rpc portal（菜单栏 App 的内核是 15897）")
    ap.add_argument("--cli", default=shutil.which("easytier-cli") or "/usr/local/libexec/easytier/easytier-cli")
    ap.add_argument("--console", default=nodes_field("console", r"https?://[^\s#]+"),
                    help="默认节点清单的 network.console；部署在控制台那台 relay 上时用 http://127.0.0.1:11211")
    ap.add_argument("--console-ip", default=issuer_ip(), help="控制台的 overlay 地址（本机解析器是 fake-IP，不走 DNS）")
    ap.add_argument("--socks", help="no-tun 内核的 SOCKS5，如 127.0.0.1:18080；TUN 版不用")
    ap.add_argument("--console-state", default=str(STATE_DIR / "mesh-console.json"),
                    help="读得到就由 agent 登录控制台；读不到就让浏览器登录")
    ap.add_argument("--allow-pair", action="store_true", help="允许页面调 pair.py 签凭据、出配对码")
    ap.add_argument("--allow-host", action="append", default=[], help="反代部署时额外允许的 Host")
    ap.add_argument("--read-only", action="store_true", help="只读：不配对、不吊销。--listen 不是回环地址时自动打开")
    ap.add_argument("--static", default=str(WEB_DIR / "dist"))
    ap.add_argument("--dump", action="store_true", help="打印一次 snapshot 后退出（生成演示数据）")
    args = ap.parse_args()
    args.allow_host = [h.lower() for h in args.allow_host]
    listen_host = hostport(args.listen)[0]
    if listen_host not in LOOPBACK:
        # 局域网可达时强制只读；监听的那个地址本身就是浏览器会带的 Host
        args.read_only = True
        if listen_host not in ("0.0.0.0", "::"):
            args.allow_host.append(listen_host)

    snap = Snapshotter(args)
    if args.dump:
        json.dump(snap.take(), sys.stdout, ensure_ascii=False, indent=1)
        sys.stdout.write("\n")
        return
    Handler.args, Handler.snap, Handler.console = args, snap, Console(args)
    host, port = hostport(args.listen)
    srv = http.server.ThreadingHTTPServer((host, port), Handler)
    srv.daemon_threads = True
    print(f"lo-li mesh agent {VERSION}  http://{host}:{port}/  rpc={args.rpc}  console={args.console}"
          f" ({Handler.console.auth} auth{', socks ' + args.socks if args.socks else ''})"
          f"{'  pair=on' if pair_capable(args) else ''}{'  READ-ONLY' if args.read_only else ''}", file=sys.stderr)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
