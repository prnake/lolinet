# /// script
# requires-python = ">=3.11"
# dependencies = ["pyyaml"]
# ///
"""部署准入站（LOLINET_BASE_URL，不设就是节点清单的 network.join）：Worker（KV + R2 + Durable Object ProbeHub）、
客户端分发文件和公开探针页面。

用法:
  uv run join/deploy.py worker              上传 worker.js、绑定 KV/R2/DO/探针密钥、挂自定义域（HOSTNAMES）、关掉 workers.dev
  uv run join/deploy.py install             上传 install.sh；准入站上还没有四个平台的 lolinet 包（先跑下面的 release）就不传
  uv run join/deploy.py dist <tarball>...   上传 dl/<文件名> 和 dl/<文件名>.sha256
  uv run join/deploy.py release <目录>       目录是一版 GitHub Release 的文件或 build 的产物：传 easytier-* / lolinet-* 包，最后是清单
                                            dl/lolinet.json（lolinet upgrade 按它自动升级，见 lolinet/release.py）
  uv run join/deploy.py lolidesk <目录>      目录是一版 lolidesk Release 的文件：传 lolidesk-* 包，最后是清单 dl/lolidesk.json
                                            （install.sh 按它装，lolinet upgrade 按它换）
  uv run join/deploy.py site                上传探针页面（先 cd web && npm run build:probe）到 R2 site/

Cloudflare 凭据：CI（.github/workflows/dist.yml）用仓库 secret CLOUDFLARE_API_TOKEN / CLOUDFLARE_ACCOUNT_ID（最小权限 token），
本机用$LOLINET_HOME/.env。
探针上报主密钥：CI 用仓库 secret PROBE_MASTER，本机用$LOLINET_HOME/state/mesh-probe.json；各 relay 的派生密钥要另外下发到 relay。
设备的公开探针编在 lolinet 里（lolinet probe），不再单独发 python 版的探针包；dl/ 里以前传的 mesh-probe.tar.gz 留着给旧的 install.sh。
"""
from __future__ import annotations

import datetime as dt
import hashlib
import hmac
import json
import os
import secrets
import sys
import uuid
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit
from urllib.request import Request, urlopen

import yaml

HERE = Path(__file__).resolve().parent
MESH_DIR = HERE.parent
# 运行时数据（.env、state/）：LOLINET_HOME，兼容旧的 FLEET_REPO，默认 ~/.config/lolinet
HOME_DIR = Path(os.environ.get("LOLINET_HOME") or os.environ.get("FLEET_REPO") or Path.home() / ".config/lolinet")
PROBE_STATE = HOME_DIR / "state" / "mesh-probe.json"
SITE_DIR = MESH_DIR / "web" / "dist-probe"
DO_CLASS = "ProbeHub"
DO_TAG = "v1"
sys.path.insert(0, str(MESH_DIR / "lolinet"))
from release import manifest as lolinet_manifest  # noqa: E402
COMPAT_DATE = "2026-10-01"


def nodes_path() -> Path:
    """节点清单：LOLINET_NODES，否则仓库根的 nodes.yaml，没有就用 nodes.example.yaml（公开仓库只有这份）。"""
    if os.environ.get("LOLINET_NODES"):
        return Path(os.environ["LOLINET_NODES"])
    p = MESH_DIR / "nodes.yaml"
    return p if p.is_file() else MESH_DIR / "nodes.example.yaml"


NODES = yaml.safe_load(nodes_path().read_text())
NET = NODES["network"]
# 准入站地址：LOLINET_BASE_URL，不设就是 https://<network.join>。Worker 拿它认准入域名、填进发出去的 install.sh
BASE_URL = (os.environ.get("LOLINET_BASE_URL") or f"https://{NET['join']}").rstrip("/")
JOIN_HOST = urlsplit(BASE_URL).hostname
CF = NODES.get("cloudflare") or {}
WORKER = CF.get("worker", "lolinet-join")
KV_TITLE = CF.get("kv", "lolinet-pairing")
BUCKET = CF.get("bucket", "lolinet-dist")
ZONE = CF.get("zone") or ".".join(JOIN_HOST.split(".")[-2:])
# 准入站之外的 <名字>.<edge_zone> 公网入口：探测内网可达后跳 <名字>-local.<edge_zone>
HOSTNAMES = [JOIN_HOST] + [f"{n}.{NET['edge_zone']}" for n in CF.get("redirects", [])]


def load_env() -> None:
    """CI 里用仓库 secret（CLOUDFLARE_API_TOKEN / CLOUDFLARE_ACCOUNT_ID / PROBE_MASTER）；本机读$LOLINET_HOME/.env。"""
    for short, std in (("CLOUDFLARE_API", "CLOUDFLARE_API_TOKEN"), ("CLOUDFLARE_ID", "CLOUDFLARE_ACCOUNT_ID")):
        if os.environ.get(std):
            os.environ.setdefault(short, os.environ[std])
    env = HOME_DIR / ".env"
    if not env.is_file():
        return
    for line in env.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            os.environ.setdefault(k.strip(), v.strip().strip('"').strip("'"))


class CloudflareError(SystemExit):
    def __init__(self, msg: str, codes: set[int]):
        super().__init__(msg)
        self.codes = codes


def api(method: str, path: str, data: bytes | None = None, ctype: str = "application/json") -> dict:
    req = Request(f"https://api.cloudflare.com/client/v4{path}", method=method, data=data,
                  headers={"Authorization": f"Bearer {os.environ['CLOUDFLARE_API']}", "Content-Type": ctype})
    try:
        with urlopen(req, timeout=120) as r:
            d = json.load(r)
    except HTTPError as e:
        d = json.loads(e.read() or b"{}")
    if not d.get("success"):
        errors = d.get("errors") or []
        raise CloudflareError(f"Cloudflare {method} {path}: {errors}", {e.get("code") for e in errors if isinstance(e, dict)})
    return d.get("result")


def acct() -> str:
    return f"/accounts/{os.environ['CLOUDFLARE_ID']}"


def probe_master() -> str:
    """上报签名主密钥：CI 用仓库 secret PROBE_MASTER，本机读$LOLINET_HOME/state/mesh-probe.json，都没有才生成（只在本机）。
    换主密钥会让所有 relay 和设备的上报全部验签失败（relay 要用 probe/device.py relay 重新生成配置下发，设备要重新配对），所以 CI 里缺 secret 直接失败。"""
    master = os.environ.get("PROBE_MASTER", "").strip()
    if master:
        if len(master) != 64 or not all(c in "0123456789abcdef" for c in master):
            raise SystemExit("PROBE_MASTER 必须是 64 位小写十六进制")
        return master
    if PROBE_STATE.is_file():
        return json.loads(PROBE_STATE.read_text())["master"]
    if os.environ.get("CI"):
        raise SystemExit("CI 里没有 PROBE_MASTER secret：不能生成新的主密钥")
    master = secrets.token_hex(32)
    PROBE_STATE.write_text(json.dumps({"master": master, "note": "join/worker.js PROBE_MASTER；relay 密钥 = HMAC(master, 'probe:<名字>')"}) + "\n")
    os.chmod(PROBE_STATE, 0o600)
    print(f"generated {PROBE_STATE}")
    return master


def relays() -> list[dict]:
    """给 Worker 的 relay 名单：名字（只用来验签，不公开）、区域（公开 id）、规格。不给 EIP、VPC、overlay 地址。
    规格 = 「provider instance_type」（如「阿里云 ecs.c9i.xlarge」），各自先看 relay 自己的，再看 relay_defaults；没写的那段省掉。"""
    defaults = NODES.get("relay_defaults") or {}

    def spec(r: dict) -> str | None:  # 两个都没写就是 None，页面上写「接入点」
        parts = (r.get(k) or defaults.get(k) for k in ("provider", "instance_type"))
        return " ".join(str(p).strip() for p in parts if p and str(p).strip()) or None

    return [{"name": r["name"], "region": r["region"], "spec": spec(r)} for r in NODES["relays"]]


def worker() -> None:
    kv = next(n for n in api("GET", f"{acct()}/storage/kv/namespaces?per_page=100") if n["title"] == KV_TITLE)
    meta = {"main_module": "worker.js", "compatibility_date": COMPAT_DATE, "bindings": [
        {"type": "kv_namespace", "name": "PAIRING", "namespace_id": kv["id"]},
        {"type": "r2_bucket", "name": "DIST", "bucket_name": BUCKET},
        {"type": "durable_object_namespace", "name": "PROBE", "class_name": DO_CLASS},
        {"type": "secret_text", "name": "PROBE_MASTER", "text": probe_master()},
        {"type": "plain_text", "name": "RELAYS", "text": json.dumps(relays())},
        {"type": "plain_text", "name": "BASE_URL", "text": BASE_URL},
        {"type": "plain_text", "name": "EDGE_ZONE", "text": NET["edge_zone"]},
        {"type": "plain_text", "name": "TITLE", "text": str(NET.get("title") or "").strip() or "lolinet"}]}
    # DO 迁移只能做一次：脚本还没有 migration_tag 时带上建类的迁移，之后只核对 tag
    current = next((s for s in api("GET", f"{acct()}/workers/scripts") if s["id"] == WORKER), {})
    tag = current.get("migration_tag")
    if tag is None:
        meta["migrations"] = {"new_tag": DO_TAG, "new_sqlite_classes": [DO_CLASS]}
    elif tag != DO_TAG:
        raise SystemExit(f"unexpected migration tag {tag!r}; update DO_TAG / migrations first")
    b = uuid.uuid4().hex
    parts = [
        (f'Content-Disposition: form-data; name="metadata"\r\nContent-Type: application/json\r\n\r\n', json.dumps(meta).encode()),
        (f'Content-Disposition: form-data; name="worker.js"; filename="worker.js"\r\n'
         f'Content-Type: application/javascript+module\r\n\r\n', (HERE / "worker.js").read_bytes()),
    ]
    body = b"".join(f"--{b}\r\n".encode() + h.encode() + c + b"\r\n" for h, c in parts) + f"--{b}--\r\n".encode()
    r = api("PUT", f"{acct()}/workers/scripts/{WORKER}", body, f"multipart/form-data; boundary={b}")
    print(f"worker {WORKER}: uploaded etag={r.get('etag', '')[:12]}")
    api("POST", f"{acct()}/workers/scripts/{WORKER}/subdomain", json.dumps({"enabled": False}).encode())
    zone = api("GET", f"/zones?name={ZONE}")[0]["id"]
    for host in HOSTNAMES:
        d = api("PUT", f"{acct()}/workers/domains", json.dumps(
            {"environment": "production", "hostname": host, "service": WORKER, "zone_id": zone}).encode())
        print(f"custom domain: {d['hostname']} -> {d['service']}")


_use_s3 = False


def put_object(key: str, data: bytes, ctype: str) -> None:
    """REST 对象接口要账号级 R2 写权限；CI 的 token 只授权了这一个桶，REST 报认证错误时改走 S3 兼容接口。"""
    global _use_s3
    if not _use_s3:
        try:
            api("PUT", f"{acct()}/r2/buckets/{BUCKET}/objects/{key}", data, ctype)
            print(f"r2 {key}: {len(data)} B")
            return
        except CloudflareError as e:
            if 10000 not in e.codes:
                raise
            _use_s3 = True
    s3_put(key, data, ctype)
    print(f"r2 {key}: {len(data)} B (s3)")


_s3_creds: tuple[str, str] | None = None


def s3_put(key: str, data: bytes, ctype: str) -> None:
    """R2 S3 接口，SigV4 签名。API token 本身就是 S3 凭据：access key = token id，secret = sha256(token)。"""
    global _s3_creds
    if _s3_creds is None:
        token = os.environ["CLOUDFLARE_API"]
        kid = api("GET", f"{acct()}/tokens/verify")["id"]
        _s3_creds = (kid, hashlib.sha256(token.encode()).hexdigest())
    kid, secret = _s3_creds
    host = f"{os.environ['CLOUDFLARE_ID']}.r2.cloudflarestorage.com"
    path = f"/{BUCKET}/" + quote(key, safe="/-_.~")
    amz = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    payload = hashlib.sha256(data).hexdigest()
    headers = {"content-type": ctype, "host": host, "x-amz-content-sha256": payload, "x-amz-date": amz}
    signed = ";".join(sorted(headers))
    canonical = "\n".join(["PUT", path, "", *(f"{k}:{headers[k]}" for k in sorted(headers)), "", signed, payload])
    scope = f"{amz[:8]}/auto/s3/aws4_request"
    to_sign = "\n".join(["AWS4-HMAC-SHA256", amz, scope, hashlib.sha256(canonical.encode()).hexdigest()])
    k = ("AWS4" + secret).encode()
    for part in (amz[:8], "auto", "s3", "aws4_request"):
        k = hmac.new(k, part.encode(), hashlib.sha256).digest()
    sig = hmac.new(k, to_sign.encode(), hashlib.sha256).hexdigest()
    req = Request(f"https://{host}{path}", data=data, method="PUT", headers={
        **{h: v for h, v in headers.items() if h != "host"},
        "Authorization": f"AWS4-HMAC-SHA256 Credential={kid}/{scope}, SignedHeaders={signed}, Signature={sig}",
    })
    try:
        with urlopen(req, timeout=120):
            pass
    except HTTPError as e:
        raise SystemExit(f"R2 S3 PUT {key}: {e.code} {e.read()[:300]!r}")


def put_dist(name: str, data: bytes) -> None:
    put_object(f"dl/{name}", data, "application/gzip")
    put_object(f"dl/{name}.sha256", f"{hashlib.sha256(data).hexdigest()}  {name}\n".encode(), "text/plain")


PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-aarch64", "macos-x86_64")


def require_lolinet() -> None:
    """install.sh 先从 GitHub 最新的 Release 下载，连不上就退到准入站 dl/（lolinet.json + lolinet-<平台>.tar.gz）。
    dl/ 是国内网络的退路：上线新脚本之前先查准入站上这些都在（dist.yml 从 Release 同步）。"""
    missing = []
    for p in PLATFORMS + ("manifest",):
        url = f"{BASE_URL}/dl/lolinet.json" if p == "manifest" else f"{BASE_URL}/dl/lolinet-{p}.tar.gz.sha256"
        try:
            with urlopen(Request(url, headers={"User-Agent": "lolinet-deploy"}), timeout=30) as r:
                body = r.read()
                ok = r.status == 200 and (b'"version"' in body if p == "manifest" else len(body.split()[0]) == 64)
        except (URLError, OSError, IndexError):
            ok = False
        if not ok:
            missing.append(url)
    if missing:
        raise SystemExit("install.sh 要下载的 lolinet 包还不在准入站上，先把一版 Release 的包传上来（join/deploy.py release <目录>）：\n  "
                         + "\n  ".join(missing))


def site() -> None:
    """探针页面：dist-probe/probe.html 作为 site/index.html，其余原样放 site/。"""
    if not (SITE_DIR / "probe.html").is_file():
        raise SystemExit(f"{SITE_DIR}/probe.html 不存在，先 cd web && npm run build:probe")
    types = {".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".html": "text/html"}
    # 先传带 hash 的 assets，最后换 index.html，换页的瞬间不会引用到还没上传的文件
    files = sorted((f for f in SITE_DIR.rglob("*") if f.is_file()), key=lambda f: f.name == "probe.html")
    for f in files:
        rel = f.relative_to(SITE_DIR).as_posix()
        key = "site/index.html" if rel == "probe.html" else f"site/{rel}"
        put_object(key, f.read_bytes(), types.get(f.suffix, "application/octet-stream"))


def main() -> None:
    load_env()
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "worker":
        worker()
    elif cmd == "site":
        site()
    elif cmd == "install":
        require_lolinet()
        put_object("install.sh", (HERE / "install.sh").read_bytes(), "text/x-shellscript")
    elif cmd == "release":
        out = Path(args[0])
        packages = {f.name: f.read_bytes() for f in sorted(out.glob("*.tar.gz")) if f.name.startswith(("easytier-", "lolinet-"))}
        m = lolinet_manifest({k: v for k, v in packages.items() if k.startswith("lolinet-")})
        # 目录是一版 GitHub Release 的文件时，清单原样传（设备从哪个来源升级拿到的都是同一份）；清单要和包对得上。
        # 以前的 Release 清单多一个 "probe"（python 版探针包），只比版本和包
        given = out / "lolinet.json"
        if given.is_file() and {k: json.loads(given.read_text()).get(k) for k in m} != m:
            raise SystemExit(f"{given} 和目录里的 lolinet 包对不上")
        for name, data in packages.items():
            put_dist(name, data)
        # 清单最后传：设备看到新清单时，它指的包都已经在了
        put_object("dl/lolinet.json", given.read_bytes() if given.is_file() else (json.dumps(m, indent=1) + "\n").encode(), "application/json")
        print(f"dl/lolinet.json：lolinet {m['version']}")
    elif cmd == "lolidesk":
        out = Path(args[0])
        packages = {f.name: f.read_bytes() for f in sorted(out.glob("lolidesk-*.tar.gz"))}
        m = lolinet_manifest(packages, "lolidesk")
        given = out / "lolidesk.json"
        if given.is_file() and json.loads(given.read_text()) != m:
            raise SystemExit(f"{given} 和目录里的 lolidesk 包对不上")
        for name, data in packages.items():
            put_dist(name, data)
        # 清单最后传，理由同上
        put_object("dl/lolidesk.json", given.read_bytes() if given.is_file() else (json.dumps(m, indent=1) + "\n").encode(), "application/json")
        print(f"dl/lolidesk.json：lolidesk {m['version']}")
    elif cmd == "dist":
        for f in map(Path, args):
            put_dist(f.name, f.read_bytes())
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
