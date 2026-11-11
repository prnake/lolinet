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
- 成员邀请。网里任何一台成员都能要配对码（lolinet invite、Lolidesk.app 的「邀请」），不用控制台账号：
  请求经 nginx 从 overlay 上来，来源地址就是成员的 overlay 地址，这里在路由表里找到它是哪台。地址按种类从地址池里分
  （server 10.188.1.0/24、desktop 10.188.16.0/20、userspace 10.188.32.0/20），凭据由这台 relay 自己经本机 EasyTier RPC 签
  （它的配置要有 credential_file，不然重启就丢）。谁能邀请什么、一天几次看 state 里的 policy.json。
  overlay 地址目前没有和凭据密码学绑定，所以这是方便用的检查，不是安全边界；改成用节点凭据签请求在计划里（docs/design.md）。

  GET  /api/mesh/info                 不用登录：能力、是否已登录
  POST /api/v1/auth/login             控制台账号登录，成功后发会话 cookie
  GET  /api/v1/auth/check_login_status
  POST /api/mesh/logout
  GET  /api/mesh/snapshot | devices | settings
  POST /api/mesh/settings             保存设置
  POST /api/mesh/settings/preview     按草稿设置渲染一份示例配置（凭据和探针密钥是占位）
  POST /api/mesh/pair | forget
  POST /api/mesh/invite               不用登录，只认 overlay 来源：{"kind", "name"?} -> 配对码（policy.json 管谁能要）
  GET  /api/mesh/policy               登录后看 / POST 改
  GET  /api/mesh/mihomo               登录后看 / POST 改集中下发的 mihomo 规则（mihomo.json）
  不用登录、只认 overlay 来源（成员的 mihomo 经内网来取）：
  GET  /api/mesh/mihomo/config.yaml?mode=tun|userspace   电脑的 mihomo 配置（lolinet mihomo）
  GET  /api/mesh/mihomo/proxies.yaml[?via=lolinet]       出口节点列表（proxy-provider），最后一个是直连
  GET  /api/mesh/mihomo/rules/<reject|direct|exit>.yaml  集中下发的规则（rule-provider）
  POST /api/mesh/exit                 {"port"}：把发请求的这台登记成出口（lolinet exit on / off，port 0 是撤掉）

手机（kind=phone）：不装 EasyTier，用 mihomo 的 WireGuard 出站连签发 relay 的 VPN 入口（EasyTier 的 vpn_portal，
具名客户端，每个手机一把派生出来的密钥和一个 10.188.48.0/20 的地址）。配对包里是一份完整的 mihomo 配置：
网内地址走 WireGuard，集中下发的规则决定哪些拒绝、哪些直连、哪些经出口节点（出口 = 网里某台的 lolinet exit，
mihomo 里是一个 socks5，经 WireGuard 拨过去）。手机在准入站的 /m 页面用配对码在浏览器里解开，再导入 mihomo 客户端。
  *    /api/v1/...                    反代控制台（白名单和 mesh_agent 一样），带会话里的控制台 cookie

文件:
  --config  /etc/mesh-manager/config.json      部署时生成、不含密钥：network、relays（EIP、EasyTier 端口）、base_url、network_id
  --secrets $CREDENTIALS_DIRECTORY/manager.json  {"manager_key", "devid_key"}（systemd LoadCredential，源文件 600）
  --state   /var/lib/mesh-manager              settings.json、devices.json（设备登记，和 pair.py 的同一种格式）、forgotten.json、
                                               policy.json、invites.json（成员邀请的记录，算每天的次数）、
                                               mihomo.json（集中下发的规则）、exits.json（登记的出口节点）

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
import ipaddress
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
# 地址池：和 nodes.example.yaml 的地址规划一致；节点清单的 network.pools 可以改
POOLS = {"server": "10.188.1.0/24", "desktop": "10.188.16.0/20", "userspace": "10.188.32.0/20", "phone": "10.188.48.0/20"}
KINDS = ("server", "desktop", "userspace", "phone")  # phone：签发 relay 的 WireGuard 入口 + mihomo 配置
DEFAULT_POLICY = {
    "invite": "members",                        # members：成员都能邀请；admins：只有登录了控制台账号的
    "member_kinds": ["desktop", "userspace", "phone"],  # 成员能邀请的种类；server 默认只给管理员
    "per_day": 20,                              # 每个成员每天最多几个
    "deny": [],                                 # 不让邀请的成员（主机名）
}


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


def check_policy(d) -> tuple[dict | None, str | None]:
    if not isinstance(d, dict):
        return None, "策略要是 JSON 对象"
    p = {**DEFAULT_POLICY, **{k: v for k, v in d.items() if k in DEFAULT_POLICY}}
    if p["invite"] not in ("members", "admins"):
        return None, "invite 只能是 members 或 admins"
    if not isinstance(p["member_kinds"], list) or not all(k in KINDS for k in p["member_kinds"]):
        return None, f"member_kinds 只能从 {', '.join(KINDS)} 里选"
    if not isinstance(p["per_day"], int) or not 0 <= p["per_day"] <= 1000:
        return None, "per_day 是 0–1000 的整数"
    if not isinstance(p["deny"], list) or not all(isinstance(x, str) and NAME_RE.match(x) for x in p["deny"]):
        return None, "deny 是主机名列表"
    return p, None


def pools(cfg: dict) -> dict:
    return {k: ipaddress.ip_network(v) for k, v in {**POOLS, **(cfg["network"].get("pools") or {})}.items()}


def allocate(pool, used: set) -> str | None:
    """池里第一个没人用的地址；末位 0 和 255 不用（看着像网段和广播，容易填错）"""
    for ip in pool.hosts():
        if ip.packed[3] in (0, 255):
            continue
        if str(ip) not in used:
            return str(ip)
    return None


def used_addresses(store: "Store", snap: dict) -> set:
    used = {d.get("overlay_ip") for d in store.read("devices.json", [])}
    for p in (snap.get("peers") or []) + (snap.get("routes") or []):
        for k in ("ipv4", "ipv4_addr"):
            v = p.get(k) if isinstance(p, dict) else None
            if isinstance(v, str) and v:
                used.add(v.split("/")[0])
    return {u for u in used if u}


# ---------- 设备配置 ----------

def q(s: str) -> str:
    return json.dumps(s, ensure_ascii=False)  # ASCII 名字和 device_config.py 输出一样


def render(cfg: dict, device: dict, name: str, ip: str, secret: str, peers: list[str], transport: str,
           userspace: bool = False) -> str:
    """和 scripts/device_config.py 的 render() 出同样的配置；开关来自设置。改一边要改另一边。
    userspace：无 TUN，和 lolinet up --userspace 写的两行一样（lolinet/src/netcfg.rs）"""
    net = cfg["network"]
    rs = {r["name"]: r for r in cfg["relays"]}
    lines = [
        f"hostname = {q(name)}",
        f"instance_name = {q(net['name'])}",
        f"ipv4 = {q(ip + '/16')}",
        "dhcp = false",
        "listeners = []",
        *(['socks5_proxy = "socks5://127.0.0.1:18080"'] if userspace else []),
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
    lines += ["", "[flags]", *(["no_tun = true"] if userspace else []), *flags, ""]
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


# ---------- mihomo ----------

RULE_SETS = ("reject", "direct", "exit")
DEFAULT_MIHOMO = {"reject": [], "direct": [], "exit": [], "exit_order": []}
RULE_RE = re.compile(r"^[A-Z][A-Z0-9-]*,[^\n\r]{1,256}$")
EXIT_GROUP = "出口"


def check_mihomo(d) -> tuple[dict | None, str | None]:
    if not isinstance(d, dict):
        return None, "要是 JSON 对象"
    out = dict(DEFAULT_MIHOMO)
    for k in RULE_SETS:
        v = d.get(k, [])
        if not isinstance(v, list) or not all(isinstance(r, str) and RULE_RE.match(r.strip()) for r in v):
            return None, f"{k} 是 mihomo 的 classical 规则列表，如 DOMAIN-SUFFIX,example.com、IP-CIDR,1.2.3.0/24、GEOSITE,openai"
        bad = [r for r in v if r.split(",", 1)[0] in ("RULE-SET", "SUB-RULE", "MATCH", "AND", "OR", "NOT")]
        if bad:
            return None, f"{k} 里不能用 {bad[0].split(',', 1)[0]}"
        out[k] = [r.strip() for r in v]
    order = d.get("exit_order", [])
    if not isinstance(order, list) or not all(isinstance(x, str) and NAME_RE.match(x) for x in order):
        return None, "exit_order 是出口节点的主机名列表"
    out["exit_order"] = order
    return out, None


def y(s: str) -> str:
    """YAML 标量：JSON 字符串就是合法的 YAML 双引号字符串"""
    return json.dumps(s, ensure_ascii=False)


def rule_provider(rules: list) -> str:
    return "payload:\n" + "".join(f"  - {y(r)}\n" for r in rules) if rules else "payload: []\n"


def exit_proxies(exits: list, order: list, via: str | None) -> str:
    """出口节点 -> socks5 代理，按管理员定的顺序（没列的按名字），最后一个是直连（不走出口）"""
    rank = {n: i for i, n in enumerate(order)}
    lines = ["proxies:"]
    for e in sorted(exits, key=lambda e: (rank.get(e["name"], len(rank)), e["name"])):
        lines.append(f"  - {{name: {y(e['name'] + ' 出口')}, type: socks5, server: {y(e['ip'])}, port: {int(e['port'])}, udp: true"
                     + (f", dialer-proxy: {y(via)}" if via else "") + "}")
    lines.append(f"  - {{name: {y('直连（不走出口）')}, type: direct}}")
    return "\n".join(lines) + "\n"


def render_mihomo(cfg: dict, mode: str, manager: str, title: str, wg: dict | None = None) -> str:
    """mode：phone（mihomo 经 WireGuard 进网）、userspace（电脑，lolinet 无 TUN，mihomo 是唯一的 TUN）、
    tun（电脑，两个 TUN：mihomo 的让开 overlay 网段，普通程序访问网内直接走 lolinet 的 TUN）。

    电脑上 mihomo 自己拨出去的连接（出口节点、从管理面拉规则、走 7890 端口的程序访问网内）一律经 lolinet 在回环上的
    SOCKS5（127.0.0.1:18080）：mihomo 开了 auto-detect-interface 后，每条往外拨的连接都绑在物理网卡上
    （macOS IP_BOUND_IF / Linux SO_BINDTODEVICE），只有回环这类地址例外，10.x 也会被绑，就到不了 lolinet 的 TUN。
    TUN 模式下 lolinet mihomo on 会给 lolinet 打开这个口。
    dns-hijack + sniffer：进 TUN 的流量也认得出域名，集中下发的 DOMAIN 规则才对它生效。
    lolinet 自己连 relay 的底层流量（relay 公网地址、lolinet / easytier-core 进程）放在最前面直连，
    不会被规则送进出口或拒绝（EasyTier 本来也把 socket 绑在物理网卡上，这是第二道保险）。"""
    net = cfg["network"]
    cidr = net.get("cidr", "10.188.0.0/16")
    host = urlsplit(manager).hostname or ""
    desktop = mode in ("tun", "userspace")
    label = {"phone": "手机，经 WireGuard 进网", "userspace": "电脑，lolinet 无 TUN，mihomo 是唯一的 TUN",
             "tun": "电脑，lolinet 和 mihomo 各一个 TUN"}[mode]
    out = [f"# {title} · mihomo 配置（{label}）",
           "# 网内地址走 lolinet；规则和出口节点由管理面集中下发（rule-providers / proxy-providers，经内网取）。",
           "# 想和自己的订阅合用：把下面的 proxies、proxy-providers、rule-providers 和 rules 的前几条并进去，网内那几条放最前。",
           "mixed-port: 7890", "allow-lan: false", "mode: rule", "log-level: warning", "ipv6: true", "unified-delay: true",
           "find-process-mode: strict", ""]
    if desktop:
        out += ["tun:", "  enable: true", "  stack: mixed", "  auto-route: true", "  auto-detect-interface: true",
                "  dns-hijack: [any:53, tcp://any:53]"]
        if mode == "tun":
            # 网内网段交给 lolinet 的 TUN（系统路由），mihomo 的 TUN 不抢
            out.append(f"  route-exclude-address: [{y(cidr)}]")
        out.append("")
    out += ["sniffer:", "  enable: true", "  parse-pure-ip: true", "  sniff:", "    TLS: {ports: [443, 8443]}",
            "    HTTP: {ports: [80, 8080-8880], override-destination: true}", "    QUIC: {ports: [443, 8443]}", ""]
    out += ["dns:", "  enable: true", "  enhanced-mode: fake-ip", "  fake-ip-range: 198.18.0.1/16",
            f"  fake-ip-filter: [{y('+.lan')}, {y('+.local')}{', ' + y('+.' + net['edge_zone']) if net.get('edge_zone') else ''}]",
            "  nameserver: [https://223.5.5.5/dns-query, https://1.1.1.1/dns-query]", ""]
    out.append("proxies:")
    if mode == "phone":
        w = wg or {}
        out += ["  - name: lolinet", "    type: wireguard", f"    server: {y(w['server'])}", f"    port: {int(w['port'])}",
                f"    ip: {y(w['ip'])}", f"    private-key: {y(w['private_key'])}", f"    public-key: {y(w['public_key'])}",
                f"    allowed-ips: [{y(cidr)}]", "    udp: true", "    mtu: 1280", "    persistent-keepalive: 25"]
    else:
        out += ["  - {name: lolinet, type: socks5, server: 127.0.0.1, port: 18080, udp: true}"]
    out += ["", "proxy-providers:", "  lolinet-exits:", "    type: http", f"    url: {y(manager + '/api/mesh/mihomo/proxies.yaml?via=lolinet')}",
            "    path: ./lolinet/exits.yaml", "    interval: 600", "    proxy: lolinet",
            "    health-check: {enable: true, url: https://cp.cloudflare.com/generate_204, interval: 600, lazy: true}",
            "", "proxy-groups:", f"  - name: {y(EXIT_GROUP)}", "    type: select", "    use: [lolinet-exits]", "", "rule-providers:"]
    for k in RULE_SETS:
        out += [f"  lolinet-{k}:", "    type: http", "    behavior: classical", "    format: yaml",
                f"    url: {y(manager + '/api/mesh/mihomo/rules/' + k + '.yaml')}", f"    path: ./lolinet/{k}.yaml",
                "    interval: 600", "    proxy: lolinet"]
    out += ["", "rules:"]
    if desktop:
        out += ["  - PROCESS-NAME,lolinet,DIRECT", "  - PROCESS-NAME,easytier-core,DIRECT"]
        for r in cfg["relays"]:
            try:
                ip = ipaddress.ip_address(str(r.get("eip") or ""))
            except ValueError:
                continue
            out.append(f"  - IP-CIDR,{ip}/32,DIRECT,no-resolve" if ip.version == 4 else f"  - IP-CIDR6,{ip}/128,DIRECT,no-resolve")
    out.append(f"  - IP-CIDR,{cidr},lolinet,no-resolve")
    if host:
        out.append(f"  - DOMAIN,{host},lolinet")
    if net.get("local_suffix"):
        out.append(f"  - DOMAIN-REGEX,^[a-z0-9-]+{re.escape(net['local_suffix'])}$,lolinet")
    out += ["  - RULE-SET,lolinet-reject,REJECT", "  - RULE-SET,lolinet-direct,DIRECT", f"  - RULE-SET,lolinet-exit,{EXIT_GROUP}",
            "  - MATCH,DIRECT", ""]
    return "\n".join(out)


def parse_wg(text: str) -> dict:
    """EasyTier vpn_portal 给的 WireGuard 客户端配置（ini）-> 字段"""
    kv = {}
    for line in text.splitlines():
        k, sep, v = line.partition("=")
        if sep:
            kv[k.strip()] = v.split("#", 1)[0].strip()
    return {"private_key": kv["PrivateKey"], "public_key": kv["PublicKey"], "ip": kv["Address"].split("/")[0],
            "port": int(kv["Endpoint"].rsplit(":", 1)[1])}


def cli_run(args, sub: list, check: bool = True) -> str:
    """easytier-cli 不带 -o json（vpn-portal add-client 这类只打一行字的）"""
    p = subprocess.run([args.cli, "-p", args.rpc, *sub], capture_output=True, text=True, timeout=20)
    if check and p.returncode != 0:
        raise Refused(502, f"easytier-cli {' '.join(sub[:2])}：{(p.stderr or p.stdout).strip()[-200:]}")
    return p.stdout


class Refused(Exception):
    """发不了配对码：HTTP 状态和给人看的原因"""

    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status, self.message = status, message


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
            if path == "/api/mesh/invite" and method == "POST":
                return self.invite()
            if path.startswith("/api/mesh/mihomo/") and method == "GET":
                return self.mihomo_get(path[len("/api/mesh/mihomo/"):], urlsplit(self.path).query)
            if path == "/api/mesh/exit" and method == "POST":
                return self.exit_post()
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
        if method == "GET" and what == "mihomo":
            return self.send_json(200, {"mihomo": self.mihomo_rules(), "exits": self.store.read("exits.json", [])})
        if method == "POST" and what == "mihomo":
            m, err = check_mihomo(self.json_body())
            if err:
                return self.send_json(400, {"message": err})
            with self.store.lock:
                self.store.write("mihomo.json", m)
            self.audit("mihomo " + json.dumps(m, ensure_ascii=False, separators=(",", ":")))
            return self.send_json(200, {"mihomo": m})
        if method == "GET" and what == "policy":
            return self.send_json(200, {"policy": self.policy(), "defaults": DEFAULT_POLICY, "kinds": list(KINDS)})
        if method == "POST" and what == "policy":
            p, err = check_policy(self.json_body())
            if err:
                return self.send_json(400, {"message": err})
            with self.store.lock:
                self.store.write("policy.json", p)
            self.audit("policy " + json.dumps(p, separators=(",", ":")))
            return self.send_json(200, {"policy": p})
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
        """管理员配对：登录者的控制台会话签凭据；地址不给就按种类从地址池里分"""
        if not self.worker:
            return 501, {"message": "没有 manager_key：部署时没下发 secrets，不能写配对包"}
        s = self.settings()["pair"]
        nid = self.cfg["network_id"]

        def sign(cid: str, ttl_days: int) -> dict:
            body = json.dumps({"ttl_seconds": ttl_days * 86400, "credential_id": cid, "reusable": False}).encode()
            status, _, data = self.console.request("POST", f"/api/v1/networks/{nid}/credentials", body, "application/json", sess["console"])
            if status == 401:
                self.sessions.drop(self.token())
                raise Refused(401, "控制台会话过期了，重新登录")
            if status != 200:
                raise Refused(502, f"控制台签凭据失败 {status}: {data[:200].decode(errors='replace')}")
            return json.loads(data)

        def revoke(cid: str) -> None:
            self.console.request("DELETE", f"/api/v1/networks/{nid}/credentials/{cid}", None, None, sess["console"])

        try:
            return 200, self.issue(req, s, sign, revoke, by=sess["user"])
        except Refused as e:
            return e.status, {"message": e.message}

    def issue(self, req: dict, s: dict, sign, revoke, by: str, kinds=KINDS) -> dict:
        """校验参数、分地址、签凭据、渲染配置、加密进 KV、登记。sign(cid, ttl_days) -> {credential_id, credential_secret,
        expiry_unix}；之后哪步失败就用 revoke(cid) 把刚签的凭据吊销，不留没人用的凭据。出错抛 Refused"""
        kind = str(req.get("kind") or "desktop")
        if kind not in kinds:
            raise Refused(400, f"种类只能是 {'、'.join(kinds)}")
        name, ip = str(req.get("name") or ""), str(req.get("ip") or "")
        try:
            ttl = int(req.get("ttl_days", s["ttl_days"]))
            minutes = int(req.get("code_minutes", s["code_minutes"]))
        except (TypeError, ValueError):
            raise Refused(400, "有效期要是整数") from None
        peers = req.get("peers") or s["peers"]
        transport = req.get("transport") or s["transport"]
        probe = req.get("probe", s["probe"])
        location = str(req.get("location") or "").strip()
        names = [r["name"] for r in self.cfg["relays"]]
        with self.store.lock:
            devices = self.store.read("devices.json", [])
            taken = {d.get("name") for d in devices}
            while not name or (name in taken and not req.get("name")):  # 没给名字：<种类>-<4 位十六进制>
                name = f"{kind}-{secrets.token_hex(2)}"
            if not ip:
                ip = allocate(pools(self.cfg)[kind], used_addresses(self.store, self.snap.get())) or ""
                if not ip:
                    raise Refused(409, f"{kind} 的地址池 {pools(self.cfg)[kind]} 用完了")
            # 先占上地址，签凭据和写 KV 要几秒，别让并发的邀请分到同一个
            reserved = {"name": name, "overlay_ip": ip, "pending": True}
            devices.append(reserved)
            self.store.write("devices.json", devices)
        try:
            return self._issue(name, ip, kind, ttl, minutes, peers, transport, probe, location, names, sign, revoke, by)
        finally:
            with self.store.lock:
                self.store.write("devices.json", [d for d in self.store.read("devices.json", []) if d != reserved])

    def _issue(self, name, ip, kind, ttl, minutes, peers, transport, probe, location, names, sign, revoke, by) -> dict:
        m = IP_RE.match(ip)
        if not NAME_RE.match(name):
            raise Refused(400, "设备名只能用小写字母、数字和 -，32 个字符以内")
        if any(d.get("name") == name and not d.get("pending") for d in self.store.read("devices.json", [])):
            raise Refused(409, f"已经有叫 {name} 的设备了")
        if not m or not all(0 <= int(x) <= 255 for x in m.groups()) or m.group(2) in ("0", "255"):
            raise Refused(400, "地址必须在 10.188.0.0/16 里")
        if not 1 <= ttl <= 365:
            raise Refused(400, "凭据有效期 1–365 天")
        if not 1 <= minutes <= 1440:
            raise Refused(400, "配对码有效期 1–1440 分钟")
        if not isinstance(peers, list) or not peers or not all(isinstance(p, str) and p in names for p in peers):
            raise Refused(400, "接入点只能是节点清单里的 relay")
        if transport not in ("wss", "tcp"):
            raise Refused(400, "传输只能是 wss 或 tcp")
        if location and not LOCATION_RE.match(location):
            raise Refused(400, "位置是 32 个字符以内的城市名")

        if kind == "phone":
            return self._issue_phone(name, ip, minutes, by)
        # 先做没有副作用的：探针密钥。再签凭据
        probe_cfg = None
        if probe is not False:
            dev_id = "d-" + hmac.new(bytes.fromhex(self.keys["devid_key"]), name.encode(), hashlib.sha256).hexdigest()[:10]
            key = self.worker.call("/v1/manager/probe-key", {"id": dev_id})["key"]
            probe_cfg = {"name": dev_id, "mode": "device", "url": self.cfg["base_url"].rstrip("/") + "/v1/probe/report",
                         "key": key, "near": peers[0]}
            if location:
                probe_cfg["location"] = location
        cid = f"{name}-{dt.datetime.now().strftime('%Y%m%d%H%M%S')}"
        cred = sign(cid, ttl)
        try:
            config = render(self.cfg, self.settings()["device"], name, ip, cred["credential_secret"], peers, transport,
                            userspace=kind == "userspace")
            # 菜单栏 App 读这两行注释显示「账号」和「凭证剩余」；lolinet up 把 probe 那行存成探针配置、lolinet 那行存进
            # /etc/lolinet/lolinet.json（lolinet invite 找这个管理面用）
            config += f"\n# credential_id = {cred['credential_id']}\n# credential_expiry_unix = {cred['expiry_unix']}\n"
            if probe_cfg:
                config += f"# probe = {json.dumps(probe_cfg, separators=(',', ':'))}\n"
            if self.manager_url():
                config += f"# lolinet = {json.dumps({'manager': self.manager_url(), 'manager_ip': self.manager_ip()}, separators=(',', ':'))}\n"
            code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(CODE_LEN))
            blob = encrypt(code, config.encode())
            put = self.worker.call("/v1/manager/pair", {"id": hashlib.sha256(code.encode()).hexdigest()[:32], "blob": blob,
                                                         "ttl": minutes * 60})
        except Exception:
            revoke(cred["credential_id"])
            raise
        with self.store.lock:
            devices = self.store.read("devices.json", [])
            devices.append({"name": name, "overlay_ip": ip, "credential_id": cred["credential_id"], "expiry_unix": cred["expiry_unix"],
                            "kind": kind, "paired_at": dt.datetime.now().isoformat(timespec="seconds"), "paired_by": by})
            self.store.write("devices.json", devices)
        shown = "-".join(code[i:i + 4] for i in range(0, CODE_LEN, 4))
        self.audit(f"pair {name} {ip} {kind} {cred['credential_id']} peers={','.join(peers)} {transport} probe={bool(probe_cfg)} by={by}")
        sudo = "sudo LOLINET_USERSPACE=1 sh" if kind == "userspace" else "sudo sh"
        return {
            "name": name,
            "ip": ip,
            "kind": kind,
            "code": shown,
            "credential_id": cred["credential_id"],
            "command": f"curl -fsSL {self.cfg['base_url'].rstrip('/')}/i | {sudo} -s {shown}",
            "expires_at": int(put.get("expires_at") or time.time() + minutes * 60),
        }

    def _issue_phone(self, name: str, ip: str, minutes: int, by: str) -> dict:
        """手机：在签发 relay 的 WireGuard 入口加一个具名客户端（密钥由入口的主密钥按名字派生），
        配对包是一份 mihomo 配置。手机在准入站的 /m 页面用配对码在浏览器里解开"""
        info = agent.cli(self.args, ["vpn-portal"]) or {}
        if not info.get("listener") and not info.get("vpn_type"):
            raise Refused(503, "签发 relay 没开 WireGuard 入口（它的 loli.toml 要有 [vpn_portal_config]），手机进不来")
        issuer = next((r for r in self.cfg["relays"] if r["name"] == self.cfg["network"].get("issuer")), None)
        if not issuer or not issuer.get("eip"):
            raise Refused(503, "节点清单里找不到签发 relay 的公网地址")
        cli_run(self.args, ["vpn-portal", "add-client", name, "--virtual-ip", f"{ip}/16"])
        try:
            info = agent.cli(self.args, ["vpn-portal"]) or {}
            client = next((c for c in info.get("clients") or [] if c.get("name") == name), None)
            if not client or not client.get("client_config"):
                raise Refused(502, "WireGuard 入口加了客户端但没给配置")
            wg = parse_wg(client["client_config"])
            wg["server"] = self.cfg.get("wireguard_endpoint") or issuer["eip"]
            if self.cfg.get("wireguard_port"):
                wg["port"] = int(self.cfg["wireguard_port"])
            title = self.cfg["network"].get("title") or "lolinet"
            config = render_mihomo(self.cfg, "phone", self.manager_url() or "https://manager.invalid", f"{title} · {name}（{ip}）", wg)
            code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(CODE_LEN))
            blob = encrypt(code, config.encode())
            put = self.worker.call("/v1/manager/pair", {"id": hashlib.sha256(code.encode()).hexdigest()[:32], "blob": blob,
                                                         "ttl": minutes * 60})
        except Exception:
            cli_run(self.args, ["vpn-portal", "remove-client", name], check=False)
            raise
        with self.store.lock:
            devices = self.store.read("devices.json", [])
            devices.append({"name": name, "overlay_ip": ip, "credential_id": f"wg:{name}", "kind": "phone",
                            "paired_at": dt.datetime.now().isoformat(timespec="seconds"), "paired_by": by})
            self.store.write("devices.json", devices)
        shown = "-".join(code[i:i + 4] for i in range(0, CODE_LEN, 4))
        self.audit(f"pair {name} {ip} phone wireguard by={by}")
        return {
            "name": name,
            "ip": ip,
            "kind": "phone",
            "code": shown,
            "credential_id": f"wg:{name}",
            "command": f"{self.cfg['base_url'].rstrip('/')}/m#{shown}",
            "expires_at": int(put.get("expires_at") or time.time() + minutes * 60),
        }

    # --- mihomo：集中下发的规则、出口节点 ---

    def mihomo_rules(self) -> dict:
        m, err = check_mihomo(self.store.read("mihomo.json", {}))
        return m if not err else dict(DEFAULT_MIHOMO)

    def mihomo_get(self, path: str, query: str):
        """成员的 mihomo（或 lolinet mihomo）经内网来取；只认 overlay 来源"""
        if not self.member():
            return self.send_json(403, {"message": f"{self.client_ip()} 不是网里的节点"})
        q = dict(p.partition("=")[::2] for p in query.split("&") if p)
        ctype = "text/yaml; charset=utf-8"
        if path == "config.yaml":
            mode = q.get("mode", "tun")
            if mode not in ("tun", "userspace"):
                return self.send_json(400, {"message": "mode 是 tun 或 userspace"})
            who = self.member() or {}
            title = self.cfg["network"].get("title") or "lolinet"
            body = render_mihomo(self.cfg, mode, self.manager_url() or "https://manager.invalid", f"{title} · {who.get('name') or who.get('ip')}")
            return self.send(200, body.encode(), ctype, [("Cache-Control", "no-store")])
        if path == "proxies.yaml":
            via = q.get("via") if q.get("via") == "lolinet" else None
            exits = self.store.read("exits.json", [])
            return self.send(200, exit_proxies(exits, self.mihomo_rules()["exit_order"], via).encode(), ctype, [("Cache-Control", "no-store")])
        m = re.fullmatch(r"rules/(reject|direct|exit)\.yaml", path)
        if m:
            return self.send(200, rule_provider(self.mihomo_rules()[m.group(1)]).encode(), ctype, [("Cache-Control", "no-store")])
        return self.send_json(404, {"message": "not found"})

    def exit_post(self):
        """lolinet exit on / off 来登记：发请求的这台（overlay 来源）在 port 上开着出口；port 0 是撤掉"""
        who = self.member()
        if not who:
            return self.send_json(403, {"message": f"{self.client_ip()} 不是网里的节点"})
        req = self.json_body() or {}
        try:
            port = int(req.get("port", 0))
        except (TypeError, ValueError):
            return self.send_json(400, {"message": "port 是整数"})
        if not 0 <= port <= 65535:
            return self.send_json(400, {"message": "port 是 0–65535"})
        name = who["name"] or who["ip"]
        with self.store.lock:
            exits = [e for e in self.store.read("exits.json", []) if e.get("ip") != who["ip"]]
            if port:
                exits.append({"name": name, "ip": who["ip"], "port": port, "since": int(time.time())})
            self.store.write("exits.json", exits)
        self.audit(f"exit {'on' if port else 'off'} {name} {who['ip']}:{port}")
        return self.send_json(200, {"ok": True, "exits": len(exits)})

    # --- 成员邀请 ---

    def manager_url(self) -> str | None:
        if self.cfg.get("manager_url"):
            return self.cfg["manager_url"].rstrip("/")
        return f"https://{self.args.allow_host[0]}" if self.args.allow_host else None

    def manager_ip(self) -> str | None:
        if self.cfg.get("manager_ip"):
            return self.cfg["manager_ip"]
        issuer = self.cfg["network"].get("issuer")
        return next((r.get("overlay_ip") for r in self.cfg["relays"] if r["name"] == issuer), None)

    def policy(self) -> dict:
        p, err = check_policy(self.store.read("policy.json", {}))
        return p if not err else dict(DEFAULT_POLICY)

    def member(self) -> dict | None:
        """请求是哪台成员发的：来源地址（nginx 给的，就是成员的 overlay 地址）在路由表里对得上的那台"""
        ip = self.client_ip()
        snap = self.snap.get()
        for p in (snap.get("peers") or []) + (snap.get("routes") or []):
            if not isinstance(p, dict):
                continue
            addr = str(p.get("ipv4") or p.get("ipv4_addr") or "").split("/")[0]
            if addr and addr == ip:
                return {"name": str(p.get("hostname") or ""), "ip": ip}
        # 手机经 WireGuard 入口进来，路由表里不一定看得到：登记过的手机地址也算
        for d in self.store.read("devices.json", []):
            if d.get("kind") == "phone" and d.get("overlay_ip") == ip and not d.get("pending"):
                return {"name": d.get("name", ""), "ip": ip}
        return None

    def local_signer(self):
        """这台 relay 自己经本机 EasyTier RPC 签凭据。没配 credential_file 的话 EasyTier 只把凭据放内存，重启就丢，不签"""
        node = agent.cli(self.args, ["node"]) or {}
        if not re.search(r"^\s*credential_file\s*=", node.get("config") or "", re.M):
            raise Refused(503, "签发 relay 的配置没有 credential_file：它签的凭据重启就丢，先在它的 loli.toml 里配上")

        def sign(cid: str, ttl_days: int) -> dict:
            out = agent.cli(self.args, ["credential", "generate", "--ttl", str(ttl_days * 86400), "--credential-id", cid,
                                        "--reusable", "false"])
            out = out[0] if isinstance(out, list) else out
            return {"credential_id": out.get("credential_id") or cid, "credential_secret": out["credential_secret"],
                    "expiry_unix": int(out.get("expiry_unix") or time.time() + ttl_days * 86400)}

        def revoke(cid: str) -> None:
            try:
                agent.cli(self.args, ["credential", "revoke", cid])
            except Exception:  # noqa: BLE001
                pass

        return sign, revoke

    def invite(self):
        """成员要配对码：不用登录，只认 overlay 来源（见文件头）"""
        if not self.worker:
            return self.send_json(501, {"message": "没有 manager_key：部署时没下发 secrets，不能写配对包"})
        who = self.member()
        if not who:
            return self.send_json(403, {"message": f"{self.client_ip()} 不是网里的节点：邀请只能从内网要"})
        req = self.json_body() or {}
        kind = str(req.get("kind") or "desktop")
        pol = self.policy()
        if pol["invite"] != "members":
            return self.send_json(403, {"message": "这个网只有管理员能发邀请（manager 的策略）"})
        if who["name"] in pol["deny"]:
            return self.send_json(403, {"message": f"{who['name']} 不能发邀请（manager 的策略）"})
        if kind not in pol["member_kinds"]:
            return self.send_json(403, {"message": f"成员只能邀请 {'、'.join(pol['member_kinds']) or '（没有）'}，{kind} 要找管理员"})
        day = dt.date.today().isoformat()
        with self.store.lock:
            log = [x for x in self.store.read("invites.json", []) if x.get("day") == day]
            if sum(1 for x in log if x.get("by") == who["ip"]) >= pol["per_day"]:
                return self.send_json(429, {"message": f"今天的邀请用完了（每天 {pol['per_day']} 个）"})
        # 成员只能决定名字和种类；有效期、接入点、传输、探针都按管理员的设置
        clean = {"kind": kind, "name": str(req.get("name") or "")}
        try:
            sign, revoke = self.local_signer()
            out = self.issue(clean, self.settings()["pair"], sign, revoke, by=f"{who['name'] or '?'}@{who['ip']}",
                             kinds=tuple(pol["member_kinds"]))
        except Refused as e:
            return self.send_json(e.status, {"message": e.message})
        with self.store.lock:
            log = [x for x in self.store.read("invites.json", []) if x.get("day") == day]
            log.append({"day": day, "by": who["ip"], "by_name": who["name"], "name": out["name"], "ip": out["ip"], "kind": kind})
            self.store.write("invites.json", log)
        return self.send_json(200, out)

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
