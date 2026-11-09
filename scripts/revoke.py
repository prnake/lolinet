# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx[socks]", "pyyaml", "cryptography"]
# ///
"""吊销某设备最新的控制台凭据（经 overlay；本机 TUN 在网时 SOCKS 写 -），并从 $LOLINET_HOME/state/mesh-devices.json 移除登记。
同名重新配对后吊销旧的那条：第三个参数写凭据 id。

用法: uv run scripts/revoke.py <设备名> <127.0.0.1:18080 | -> [凭据 id]
"""
import hashlib
import json
import os
import sys
from pathlib import Path

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parent))
from pair import CONSOLE_HOST, CONSOLE_IP, STATE_DIR  # noqa: E402

name, socks = sys.argv[1], sys.argv[2] if len(sys.argv) > 2 and sys.argv[2] != "-" else None
reg = STATE_DIR / "mesh-devices.json"
devices = json.loads(reg.read_text())
entry = [d for d in devices if d["name"] == name and (len(sys.argv) < 4 or d["credential_id"] == sys.argv[3])][-1]
st = json.loads((STATE_DIR / "mesh-console.json").read_text())
ext = {"sni_hostname": CONSOLE_HOST}
with httpx.Client(base_url=f"https://{CONSOLE_IP}", headers={"Host": CONSOLE_HOST}, timeout=30,
                  proxy=f"socks5://{socks}" if socks else None) as c:
    c.post("/api/v1/auth/login", extensions=ext, json={"username": st["admin_user"],
           "password": hashlib.md5(st["admin_pw"].encode()).hexdigest()}).raise_for_status()
    r = c.delete(f"/api/v1/networks/{st['network_id']}/credentials/{entry['credential_id']}", extensions=ext)
    print("revoke", entry["credential_id"], r.status_code)
    left = [x["credential_id"] for x in c.get(f"/api/v1/networks/{st['network_id']}/credentials", extensions=ext).json().get("credentials", [])] \
        if r.status_code < 300 else "?"
    print("credentials left:", left)
devices.remove(entry)
reg.write_text(json.dumps(devices, ensure_ascii=False, indent=2) + "\n")
os.chmod(reg, 0o600)
