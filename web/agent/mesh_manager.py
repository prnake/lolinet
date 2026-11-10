#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = []
# ///
"""lo-li mesh 管理面：和控制台跑在同一台签发 relay 上，nginx 只在 overlay 地址上把它发成 manager.<edge_zone>，只有内网能开。

在 mesh_agent 的基础上多了三样：
- 登录。整站要登录，账号就是 easytier-web 控制台的账号：浏览器把 md5(密码) 交过来，这里转给控制台，
  成功后发一个本服务自己的会话 cookie，控制台的会话 cookie 只留在服务端。manager 不存控制台密码。
- 配对在这里做完，不调 pair.py：用登录者的控制台会话签凭据，按「设置」渲染设备配置，用 openssl 以配对码加密，
  再经准入站 Worker 的 /v1/manager/pair 放进 KV。设备的探针上报密钥找 Worker 要（/v1/manager/probe-key）。
  请求用 manager_key = HMAC(PROBE_MASTER, "manager") 签名，所以这台机器既没有 Cloudflare token，也没有主密钥。
- 设置。配对默认值（凭据有效期、配对码有效期、默认接入点、传输、公开探针）和设备配置开关，存在 state 目录。

  GET  /api/mesh/info                 不用登录：能力、是否已登录
  POST /api/v1/auth/login             控制台账号登录，成功后发会话 cookie
  GET  /api/v1/auth/check_login_status
  POST /api/mesh/logout
  GET  /api/mesh/snapshot | devices | settings
  POST /api/mesh/settings             保存设置
  POST /api/mesh/settings/preview     按草稿设置渲染一份示例配置（凭据和探针密钥是占位）
  POST /api/mesh/pair | forget
  *    /api/v1/...                    反代控制台（白名单和 mesh_agent 一样），带会话里的控制台 cookie

文件:
  --config  /etc/mesh-manager/config.json      部署时生成、不含密钥：network、relays（EIP、EasyTier 端口）、base_url、network_id
  --secrets $CREDENTIALS_DIRECTORY/manager.json  {"manager_key", "devid_key"}（systemd LoadCredential，源文件 600）
  --state   /var/lib/mesh-manager              settings.json、devices.json（设备登记，和 pair.py 的同一种格式）、forgotten.json

用法（本机试）:
  python3 web/agent/mesh_manager.py --config /tmp/m/config.json --secrets /tmp/m/manager.json --state /tmp/m/state \\
      --console http://127.0.0.1:11211 --rpc 127.0.0.1:15888
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import hmac
import http.server
import json
import os
import re
import secrets
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python3 -I 不把脚本目录放进 sys.path
import mesh_agent as agent  # noqa: E402

VERSION = "0.1.0"
COOKIE = "mesh_manager"
SESSION_TTL = 12 * 3600
LOGIN_FAILS = 10  # 每个来源地址 10 分钟内最多错 10 次
CODE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"  # 和 pair.py 一致
CODE_LEN = 12
PBKDF2_ITER = 200000  # 和 join/install.sh 的 openssl -iter 一致
NAME_RE = agent.NAME_RE
IP_RE = agent.IP_RE
LOCATION_RE = re.compile(r"^[^\x00-\x1f<>\"\\]{1,32}$")


# ---------- 状态文件 ----------

class Store:
    """state 目录里的 JSON 文件：写临时文件再 rename，600。"""

    def __init__(self, root: Path):
        self.root = root
        self.lock = threading.Lock()
        root.mkdir(parents=True, exist_ok=True)

    def read(self, name: str, default):
        p = self.root / name
        try:
            return json.loads(p.read_text())
        except FileNotFoundError:
            return default

    def write(self, name: str, data) -> None:
        p = self.root / name
        tmp = p.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
        os.chmod(tmp, 0o600)
        tmp.replace(p)


# ---------- 设置 ----------

def default_settings(cfg: dict) -> dict:
    relays = [r["name"] for r in cfg["relays"]]
    return {
        "pair": {
            "ttl_days": 365,
            "code_minutes": 15,
            "peers": cfg["network"].get("device_peers") or relays[:3],
            "transport": "wss",
            "probe": True,
        },
        # 和 scripts/device_config.py 的默认一致：latency_first、enable_kcp_proxy 开，额外的 udp peer 关
        "device": {"latency_first": True, "enable_kcp_proxy": True, "udp_peers": False},
    }


def check_settings(cfg: dict, d) -> tuple[dict | None, str | None]:
    """校验整份设置，返回规范化后的副本；不认识的字段丢掉。"""
    if not isinstance(d, dict) or not isinstance(d.get("pair"), dict) or not isinstance(d.get("device"), dict):
        return None, "设置要有 pair 和 device 两部分"
    p, v = d["pair"], d["device"]
    names = [r["name"] for r in cfg["relays"]]
    try:
        ttl, code = int(p.get("ttl_days")), int(p.get("code_minutes"))
    except (TypeError, ValueError):
        return None, "有效期要是整数"
    if not 1 <= ttl <= 365:
        return None, "凭据有效期 1–365 天"
    if not 1 <= code <= 1440:
        return None, "配对码有效期 1–1440 分钟"
    peers = p.get("peers")
    if not isinstance(peers, list) or not peers or not all(x in names for x in peers) or len(set(peers)) != len(peers):
        return None, "默认接入点至少选一台，只能是节点清单里的 relay"
    if p.get("transport") not in ("wss", "tcp"):
        return None, "传输只能是 wss 或 tcp"
    flags = {}
    for k in ("latency_first", "enable_kcp_proxy", "udp_peers"):
        if not isinstance(v.get(k), bool):
            return None, f"{k} 要是 true / false"
        flags[k] = v[k]
    if not isinstance(p.get("probe"), bool):
        return None, "公开探针要是 true / false"
    return {"pair": {"ttl_days": ttl, "code_minutes": code, "peers": list(peers),  # 顺序有意义：第一台是探针「附近」的兜底
                     "transport": p["transport"], "probe": p["probe"]}, "device": flags}, None


# ---------- 设备配置 ----------

def q(s: str) -> str:
    return json.dumps(s, ensure_ascii=False)  # ASCII 名字和 device_config.py 输出一样


def render(cfg: dict, device: dict, name: str, ip: str, secret: str, peers: list[str], transport: str) -> str:
    """和 scripts/device_config.py 的 render() 出同样的配置；开关来自设置。改一边要改另一边。"""
    net = cfg["network"]
    rs = {r["name"]: r for r in cfg["relays"]}
    lines = [
        f"hostname = {q(name)}",
        f"instance_name = {q(net['name'])}",
        f"ipv4 = {q(ip + '/16')}",
        "dhcp = false",
        "listeners = []",
        "",
        "[network_identity]",
        f"network_name = {q(net['name'])}",
        "",
        "[secure_mode]",
        "enabled = true",
        f"local_private_key = {q(secret)}",
    ]
    for p in peers:
        r = rs[p]
        host = r["eip"]
        uri = f"tcp://{host}:{r['et_port']}" if transport == "tcp" else f"wss://{host}:{r.get('wss_port', 443)}/?sni={net['sni']}"
        lines += ["", "[[peer]]", f"# {p}.{net['edge_zone']}", f"uri = {q(uri)}"]
        # 额外一条 udp：配合 lolinet 的隧道偏好补丁让内层 UDP 走 UDP 隧道；relay 没换成打了补丁的 lolinet 之前别开。
        # tcp 传输是给官方二进制用的，它们没有这个偏好，会把 KCP 也放到 udp 上，所以只跟 wss 一起加
        if transport == "wss" and device.get("udp_peers"):
            udp_uri = f"udp://{host}:{r['et_port']}"
            lines += ["", "[[peer]]", f"# {p}.{net['edge_zone']} udp", f"uri = {q(udp_uri)}"]
    flags = ["private_mode = true"] + [f"{k} = true" for k in ("latency_first", "enable_kcp_proxy") if device.get(k)]
    lines += ["", "[flags]", *flags, ""]
    return "\n".join(lines)


def encrypt(code: str, plain: bytes) -> str:
    """openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -md sha256 -a -A，和 install.sh 解密的参数一致；配对码经管道给，不上命令行。"""
    base = ["openssl", "enc", "-aes-256-cbc", "-pbkdf2", "-iter", str(PBKDF2_ITER), "-md", "sha256", "-a", "-A"]

    def run(extra: list[str], data: bytes) -> bytes:
        r, w = os.pipe()
        os.write(w, code.encode() + b"\n")
        os.close(w)
        try:
            p = subprocess.run([*base, *extra, "-pass", f"fd:{r}"], input=data, capture_output=True, pass_fds=(r,), timeout=60)
        finally:
            os.close(r)
        if p.returncode != 0:
            raise RuntimeError("openssl: " + p.stderr.decode(errors="replace").strip()[-200:])
        return p.stdout

    blob = run(["-salt"], plain)
    if run(["-d"], blob) != plain:  # 设备上就是这样解的；对不上宁可不发
        raise RuntimeError("openssl 加解密自检没通过")
    return blob.decode().strip()


# ---------- Worker ----------

class Worker:
    def __init__(self, base_url: str, manager_key: str):
        self.base = base_url.rstrip("/")
        self.key = bytes.fromhex(manager_key)

    def call(self, path: str, obj: dict) -> dict:
        body = json.dumps(obj, separators=(",", ":")).encode()
        ts = str(int(time.time()))
        sig = hmac.new(self.key, f"{ts}.{path}.".encode() + body, hashlib.sha256).hexdigest()
        req = urllib.request.Request(self.base + path, data=body, method="POST", headers={
            "Content-Type": "application/json", "User-Agent": f"mesh-manager/{VERSION}",
            "X-Manager-Ts": ts, "X-Manager-Sig": sig})
        try:
            with urllib.request.urlopen(req, timeout=20) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"准入站 {path} {e.code}: {e.read()[:200].decode(errors='replace')}") from None


# ---------- 会话 ----------

class Sessions:
    def __init__(self):
        self.lock = threading.Lock()
        self.items: dict[str, dict] = {}
        self.fails: dict[str, list[float]] = {}

    def new(self, user: str, console_cookie: str) -> str:
        tok = secrets.token_urlsafe(32)
        with self.lock:
            now = time.time()
            self.items = {k: v for k, v in self.items.items() if v["exp"] > now}
            self.items[tok] = {"user": user, "console": console_cookie, "exp": now + SESSION_TTL}
        return tok

    def get(self, tok: str | None) -> dict | None:
        if not tok:
            return None
        with self.lock:
            s = self.items.get(tok)
            if s and s["exp"] > time.time():
                return s
            self.items.pop(tok, None)
        return None

    def drop(self, tok: str | None) -> None:
        with self.lock:
            self.items.pop(tok or "", None)

    def blocked(self, ip: str) -> bool:
        with self.lock:
            now = time.time()
            self.fails[ip] = [t for t in self.fails.get(ip, []) if now - t < 600]
            return len(self.fails[ip]) >= LOGIN_FAILS

    def failed(self, ip: str) -> None:
        with self.lock:
            self.fails.setdefault(ip, []).append(time.time())


# ---------- HTTP ----------

class Manager(agent.Handler):
    server_version = f"mesh-manager/{VERSION}"
    cfg: dict
    keys: dict
    store: Store
    sessions: Sessions
    worker: Worker | None

    def client_ip(self) -> str:
        # 只听回环，前面是 nginx：来源地址看它给的 X-Forwarded-For（nginx 用 $remote_addr 覆盖，不是追加）
        xff = self.headers.get("X-Forwarded-For")
        return xff.split(",")[0].strip() if xff and self.client_address[0] in agent.LOOPBACK else self.client_address[0]

    def token(self) -> str | None:
        for part in (self.headers.get("Cookie") or "").split(";"):
            k, _, v = part.strip().partition("=")
            if k == COOKIE:
                return v
        return None

    def session(self) -> dict | None:
        return self.sessions.get(self.token())

    def cookie(self, tok: str, max_age: int) -> tuple[str, str]:
        host = (self.headers.get("Host") or "").rsplit(":", 1)[0].strip("[]").lower()
        secure = "" if host in agent.LOOPBACK else "; Secure"  # 本机 http 调试时浏览器不收 Secure cookie
        return ("Set-Cookie", f"{COOKIE}={tok}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}")

    def audit(self, msg: str) -> None:
        s = self.session()
        sys.stderr.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} AUDIT {msg} by={s['user'] if s else '-'} from={self.client_ip()}\n")

    def route(self, method: str):
        if not self.host_ok():
            return self.send_json(421, {"message": "Host not allowed"})
        path = urlsplit(self.path).path
        if method != "GET" and self.headers.get("X-Mesh-Agent") != "1":
            return self.send_json(403, {"message": "missing X-Mesh-Agent header"})
        try:
            if path == "/api/mesh/info" and method == "GET":
                return self.info()
            if path == "/api/v1/auth/login" and method == "POST":
                return self.login()
            if path.startswith("/api/"):
                sess = self.session()
                if not sess:
                    return self.send_json(401, {"message": "没有登录或登录已过期"})
                if path.startswith("/api/v1/"):
                    return self.proxy_as(method, sess)
                return self.manage(method, path[len("/api/mesh/"):] if path.startswith("/api/mesh/") else "", sess)
            if method == "GET":
                return self.static(path)
            return self.send_json(405, {"message": "method not allowed"})
        except Exception as e:  # noqa: BLE001
            sys.stderr.write(f"{time.strftime('%H:%M:%S')} ERROR {method} {path}: {type(e).__name__}: {e}\n")
            return self.send_json(502, {"message": f"{type(e).__name__}: {e}"})

    def info(self):
        s = self.session()
        net = self.cfg["network"]
        return self.send_json(200, {
            "agent": VERSION,
            "rpc": self.args.rpc,
            "socks": None,
            "console": {"url": net.get("console", ""), "auth": "session"},
            "network_id": self.cfg.get("network_id") if s else None,
            "pair": bool(self.worker) and bool(s),
            "registry": True,
            "read_only": False,
            "join": urlsplit(self.cfg["base_url"]).hostname,
            "manager": True,
            "login_required": True,
            "logged_in": bool(s),
            "user": s["user"] if s else None,
        })

    def login(self):
        ip = self.client_ip()
        if self.sessions.blocked(ip):
            return self.send_json(429, {"message": "错太多次了，10 分钟后再试"})
        req = self.json_body() or {}
        user, pw = str(req.get("username", ""))[:64], str(req.get("password", ""))
        if not user or not re.fullmatch(r"[0-9a-f]{32}", pw):
            return self.send_json(400, {"message": "要用户名和 md5(密码)"})
        body = json.dumps({"username": user, "password": pw}).encode()
        status, rh, data = self.console._do("POST", "/api/v1/auth/login", body, {"Content-Type": "application/json"})
        if status != 200:
            self.sessions.failed(ip)
            sys.stderr.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} AUDIT login-failed user={user} from={ip} status={status}\n")
            return self.send_json(401 if status in (400, 401, 403) else 502, {"message": "用户名或密码不对" if status < 500 else "控制台出错"})
        cookie = next((v.split(";", 1)[0] for k, v in rh if k.lower() == "set-cookie"), None)
        if not cookie:
            return self.send_json(502, {"message": "控制台登录成功但没有给会话 cookie"})
        tok = self.sessions.new(user, cookie)
        sys.stderr.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} AUDIT login user={user} from={ip}\n")
        return self.send_json(200, {"ok": True, "user": user}, [self.cookie(tok, SESSION_TTL)])

    def proxy_as(self, method: str, sess: dict):
        bare = urlsplit(self.path).path
        if method == "GET" and not agent.READ_ALLOW.match(bare):
            return self.send_json(403, {"message": "manager 不代理这个接口，请到官方控制台查看"})
        if method != "GET" and not any(m == method and rx.match(bare) for m, rx in agent.WRITE_ALLOW):
            return self.send_json(403, {"message": "manager 不代理这个写操作，请到官方控制台操作"})
        body = self.body() if method in ("POST", "PUT", "PATCH") else None
        status, rh, data = self.console.request(method, self.path, body, self.headers.get("Content-Type"), sess["console"])
        if status == 401:  # 控制台那边的会话过期了：这边也作废，页面回到登录
            self.sessions.drop(self.token())
            return self.send_json(401, {"message": "控制台会话过期了，重新登录"}, [self.cookie("", 0)])
        if method == "DELETE" and status < 300:
            self.audit(f"revoke {bare.rsplit('/', 1)[-1]}")
        ctype = next((v for k, v in rh if k.lower() == "content-type"), "application/octet-stream")
        if "json" in ctype and data:
            try:
                data = json.dumps(agent.scrub(json.loads(data)), ensure_ascii=False).encode()
            except ValueError:
                pass
        self.send(status, data, ctype, [("Cache-Control", "no-store")])

    def manage(self, method: str, what: str, sess: dict):
        if method == "POST" and what == "logout":
            self.sessions.drop(self.token())
            return self.send_json(200, {"ok": True}, [self.cookie("", 0)])
        if method == "GET" and what == "snapshot":
            return self.send_json(200, self.snap.get())
        if method == "GET" and what == "devices":
            return self.send_json(200, {"devices": self.store.read("devices.json", [])})
        if method == "GET" and what == "settings":
            return self.send_json(200, self.settings_view())
        if method == "POST" and what == "settings":
            s, err = check_settings(self.cfg, self.json_body())
            if err:
                return self.send_json(400, {"message": err})
            with self.store.lock:
                self.store.write("settings.json", s)
            self.audit("settings " + json.dumps(s, separators=(",", ":")))
            return self.send_json(200, self.settings_view())
        if method == "POST" and what == "settings/preview":
            req = self.json_body() or {}
            s, err = check_settings(self.cfg, req.get("settings"))
            if err:
                return self.send_json(400, {"message": err})
            return self.send_json(200, {"config": self.preview(s)})
        if method == "POST" and what == "pair":
            req = self.json_body()
            return self.send_json(*self.pair(req, sess)) if req is not None else self.send_json(400, {"message": "body 不是 JSON 对象"})
        if method == "POST" and what == "forget":
            req = self.json_body() or {}
            return self.send_json(*self.forget(str(req.get("credential_id", ""))))
        return self.send_json(404, {"message": "not found"})

    # --- 设置 ---

    def settings(self) -> dict:
        s, err = check_settings(self.cfg, self.store.read("settings.json", None))
        return s if not err else default_settings(self.cfg)

    def settings_view(self) -> dict:
        return {
            "settings": self.settings(),
            "defaults": default_settings(self.cfg),
            "relays": [{"name": r["name"], "region": r.get("region"), "overlay_ip": r.get("overlay_ip")} for r in self.cfg["relays"]],
            "network": {k: self.cfg["network"].get(k) for k in ("name", "cidr", "edge_zone", "issuer", "console")},
            "base_url": self.cfg["base_url"],
            "preview": self.preview(self.settings()),
        }

    def preview(self, s: dict) -> str:
        p = s["pair"]
        conf = render(self.cfg, s["device"], "<设备名>", "10.188.16.x", "<控制台签发的凭据私钥>", p["peers"], p["transport"])
        conf += "\n# credential_id = <设备名>-<时间>\n# credential_expiry_unix = <签发时间 + %d 天>\n" % p["ttl_days"]
        if p["probe"]:
            conf += '# probe = {"name":"d-<HMAC(devid_key, 设备名)>","mode":"device","url":"%s/v1/probe/report","key":"<Worker 派生>","near":"%s"}\n' % (
                self.cfg["base_url"].rstrip("/"), p["peers"][0])
        return conf

    # --- 配对 ---

    def pair(self, req: dict, sess: dict) -> tuple[int, dict]:
        if not self.worker:
            return 501, {"message": "没有 manager_key：部署时没下发 secrets，不能写配对包"}
        s = self.settings()["pair"]
        name, ip = str(req.get("name", "")), str(req.get("ip", ""))
        try:
            ttl = int(req.get("ttl_days", s["ttl_days"]))
            minutes = int(req.get("code_minutes", s["code_minutes"]))
        except (TypeError, ValueError):
            return 400, {"message": "有效期要是整数"}
        peers = req.get("peers") or s["peers"]
        transport = req.get("transport") or s["transport"]
        probe = req.get("probe", s["probe"])
        location = str(req.get("location") or "").strip()
        m = IP_RE.match(ip)
        names = [r["name"] for r in self.cfg["relays"]]
        if not NAME_RE.match(name):
            return 400, {"message": "设备名只能用小写字母、数字和 -，32 个字符以内"}
        if not m or not all(0 <= int(x) <= 255 for x in m.groups()) or m.group(2) in ("0", "255"):
            return 400, {"message": "地址必须在 10.188.0.0/16 里"}
        if not 1 <= ttl <= 365:
            return 400, {"message": "凭据有效期 1–365 天"}
        if not 1 <= minutes <= 1440:
            return 400, {"message": "配对码有效期 1–1440 分钟"}
        if not isinstance(peers, list) or not peers or not all(isinstance(p, str) and p in names for p in peers):
            return 400, {"message": "接入点只能是节点清单里的 relay"}
        if transport not in ("wss", "tcp"):
            return 400, {"message": "传输只能是 wss 或 tcp"}
        if location and not LOCATION_RE.match(location):
            return 400, {"message": "位置是 32 个字符以内的城市名"}

        # 先做没有副作用的：探针密钥。再签凭据；之后哪步失败就把刚签的凭据吊销掉，不留没人用的凭据
        probe_cfg = None
        if probe is not False:
            dev_id = "d-" + hmac.new(bytes.fromhex(self.keys["devid_key"]), name.encode(), hashlib.sha256).hexdigest()[:10]
            key = self.worker.call("/v1/manager/probe-key", {"id": dev_id})["key"]
            probe_cfg = {"name": dev_id, "mode": "device", "url": self.cfg["base_url"].rstrip("/") + "/v1/probe/report",
                         "key": key, "near": peers[0]}
            if location:
                probe_cfg["location"] = location
        nid = self.cfg["network_id"]
        cid = f"{name}-{dt.datetime.now().strftime('%Y%m%d%H%M%S')}"
        body = json.dumps({"ttl_seconds": ttl * 86400, "credential_id": cid, "reusable": False}).encode()
        status, _, data = self.console.request("POST", f"/api/v1/networks/{nid}/credentials", body, "application/json", sess["console"])
        if status == 401:
            self.sessions.drop(self.token())
            return 401, {"message": "控制台会话过期了，重新登录"}
        if status != 200:
            return 502, {"message": f"控制台签凭据失败 {status}: {data[:200].decode(errors='replace')}"}
        cred = json.loads(data)
        try:
            config = render(self.cfg, self.settings()["device"], name, ip, cred["credential_secret"], peers, transport)
            # 菜单栏 App 读这两行注释显示「账号」和「凭证剩余」；install.sh 把 probe 那行取出来装探针
            config += f"\n# credential_id = {cred['credential_id']}\n# credential_expiry_unix = {cred['expiry_unix']}\n"
            if probe_cfg:
                config += f"# probe = {json.dumps(probe_cfg, separators=(',', ':'))}\n"
            code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(CODE_LEN))
            blob = encrypt(code, config.encode())
            put = self.worker.call("/v1/manager/pair", {"id": hashlib.sha256(code.encode()).hexdigest()[:32], "blob": blob,
                                                         "ttl": minutes * 60})
        except Exception:
            self.console.request("DELETE", f"/api/v1/networks/{nid}/credentials/{cred['credential_id']}", None, None, sess["console"])
            raise
        with self.store.lock:
            devices = self.store.read("devices.json", [])
            devices.append({"name": name, "overlay_ip": ip, "credential_id": cred["credential_id"], "expiry_unix": cred["expiry_unix"],
                            "paired_at": dt.datetime.now().isoformat(timespec="seconds"), "paired_by": sess["user"]})
            self.store.write("devices.json", devices)
        shown = "-".join(code[i:i + 4] for i in range(0, CODE_LEN, 4))
        self.audit(f"pair {name} {ip} {cred['credential_id']} peers={','.join(peers)} {transport} probe={bool(probe_cfg)}")
        return 200, {
            "name": name,
            "ip": ip,
            "code": shown,
            "credential_id": cred["credential_id"],
            "command": f"curl -fsSL {self.cfg['base_url'].rstrip('/')}/i | sudo sh -s {shown}",
            "expires_at": int(put.get("expires_at") or time.time() + minutes * 60),
        }

    def forget(self, cid: str) -> tuple[int, dict]:
        with self.store.lock:
            devices = self.store.read("devices.json", [])
            left = [d for d in devices if d.get("credential_id") != cid]
            self.store.write("devices.json", left)
            gone = self.store.read("forgotten.json", [])
            if cid and cid not in gone:  # 墓碑：部署时合并本机的设备登记，别把删掉的又带回来
                self.store.write("forgotten.json", [*gone, cid])
        self.audit(f"forget {cid}")
        return 200, {"removed": len(devices) - len(left)}


def seed_registry(store: Store, seed: list) -> int:
    """部署时带上的本机设备登记：没有的补进来，已有的和删过的（墓碑）不动。之后以这里的登记为准。"""
    with store.lock:
        have = store.read("devices.json", [])
        skip = {d.get("credential_id") for d in have} | set(store.read("forgotten.json", []))
        add = [d for d in seed if isinstance(d, dict) and d.get("credential_id") and d["credential_id"] not in skip]
        if add:
            store.write("devices.json", have + add)
    return len(add)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--listen", default="127.0.0.1:10188")
    ap.add_argument("--rpc", default="127.0.0.1:15888")
    ap.add_argument("--cli", default="/opt/easytier/easytier-cli")
    ap.add_argument("--console", default="http://127.0.0.1:11211", help="控制台 API（同机）")
    ap.add_argument("--console-ip", help="控制台是 https 域名时直接连这个地址（本机试：控制台的 overlay IP）")
    ap.add_argument("--config", default="/etc/mesh-manager/config.json")
    ap.add_argument("--secrets", default=str(Path(os.environ.get("CREDENTIALS_DIRECTORY", "/etc/mesh-manager")) / "manager.json"))
    ap.add_argument("--state", default=os.environ.get("STATE_DIRECTORY", "/var/lib/mesh-manager"))
    ap.add_argument("--allow-host", action="append", default=[], help="nginx 转过来的 Host，如 manager.edge.lo-li.co")
    ap.add_argument("--static", default=str(agent.WEB_DIR / "dist"))
    args = ap.parse_args()
    args.allow_host = [h.lower() for h in args.allow_host]
    if agent.hostport(args.listen)[0] not in agent.LOOPBACK:
        raise SystemExit("manager 只听回环地址，内网访问经 nginx（只放 overlay 网段）")
    # mesh_agent 的 Console / Snapshotter 要的字段
    args.socks, args.console_state, args.read_only, args.allow_pair = None, None, False, False

    cfg = json.loads(Path(args.config).read_text())
    keys: dict = {}
    if Path(args.secrets).is_file():
        keys = json.loads(Path(args.secrets).read_text())
    Manager.args, Manager.cfg, Manager.keys = args, cfg, keys
    Manager.snap, Manager.console = agent.Snapshotter(args), agent.Console(args)
    Manager.store, Manager.sessions = Store(Path(args.state)), Sessions()
    seeded = seed_registry(Manager.store, cfg.get("seed_devices") or [])
    Manager.worker = Worker(cfg["base_url"], keys["manager_key"]) if keys.get("manager_key") and keys.get("devid_key") else None
    host, port = agent.hostport(args.listen)
    srv = http.server.ThreadingHTTPServer((host, port), Manager)
    srv.daemon_threads = True
    print(f"lo-li mesh manager {VERSION}  http://{host}:{port}/  rpc={args.rpc}  console={args.console}  state={args.state}"
          f"  pair={'on' if Manager.worker else 'off (no secrets)'}  hosts={','.join(args.allow_host) or '-'}"
          f"{f'  seeded {seeded} devices' if seeded else ''}", file=sys.stderr)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
