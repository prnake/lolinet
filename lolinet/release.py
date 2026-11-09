#!/usr/bin/env python3
"""lolinet 的发布清单 lolinet.json 和探针包 mesh-probe.tar.gz（只用标准库）。

清单写「设备应该跑哪个版本」，lolinet upgrade 按它换（见 lolinet/src/upgrade.rs）：
  {"version": "<lolinet --version 的版本>",
   "lolinet": {"<macos|linux>-<aarch64|x86_64>": {"file": "lolinet-<平台>.tar.gz", "sha256": "..."}},
   "probe": {"file": "mesh-probe.tar.gz"}}
版本取包里 BUILD-INFO 的第一行（lolinet <版本>），四个平台必须一致。探针包的 sha256 放在同目录的 <file>.sha256，
所以只更新探针（join/deploy.py install）不用改清单。清单和包放同一个目录：准入站 dl/，或者一个 GitHub Release。

用法（公开仓库 build.yml 发 GitHub Release）：
  python3 lolinet/release.py <放了 lolinet-*.tar.gz 的目录>
  在同一个目录写 lolinet.json、mesh-probe.tar.gz、mesh-probe.tar.gz.sha256。
准入站用 join/deploy.py release（同样的函数，传到 dl/）。
"""
from __future__ import annotations

import gzip
import hashlib
import io
import json
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-aarch64", "macos-x86_64")
PROBE_FILES = ("reporter.py", "mesh-probe.service", "co.lo-li.mesh-probe.plist")
PROBE_NAME = "mesh-probe.tar.gz"


def probe_bundle() -> bytes:
    """join/install.sh 装探针、lolinet upgrade 换探针用的包：reporter.py + systemd 单元（Linux）+ LaunchDaemon（macOS）。
    tar 头和 gzip 的时间戳都是 0，同一个 Python 下内容不变 sha256 就不变（不同版本的 tar 头格式不一样）。"""
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as t:
        for name in PROBE_FILES:
            data = (ROOT / "probe" / name).read_bytes()
            info = tarfile.TarInfo(name)
            info.size, info.mode = len(data), 0o644
            t.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0)


def sha_line(name: str, data: bytes) -> bytes:
    return f"{hashlib.sha256(data).hexdigest()}  {name}\n".encode()


def version_of(tgz: bytes) -> str:
    with tarfile.open(fileobj=io.BytesIO(tgz)) as t:
        member = t.extractfile("BUILD-INFO")
        first = member.read().decode().splitlines()[0] if member else ""
    if not first.startswith("lolinet "):
        raise SystemExit(f"BUILD-INFO 第一行不是 lolinet <版本>：{first!r}")
    return first.split(" ", 1)[1].strip()


def manifest(packages: dict[str, bytes]) -> dict:
    """packages：文件名 -> 内容，要有四个平台的 lolinet-<平台>.tar.gz"""
    files, versions = {}, {}
    for p in PLATFORMS:
        name = f"lolinet-{p}.tar.gz"
        if name not in packages:
            raise SystemExit(f"缺 {name}：清单要四个平台都在")
        versions[p] = version_of(packages[name])
        files[p] = {"file": name, "sha256": hashlib.sha256(packages[name]).hexdigest()}
    if len(set(versions.values())) != 1:
        raise SystemExit(f"四个平台的版本不一致：{versions}")
    return {"version": next(iter(versions.values())), "lolinet": files, "probe": {"file": PROBE_NAME}}


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    out = Path(sys.argv[1])
    m = manifest({f.name: f.read_bytes() for f in out.glob("lolinet-*.tar.gz")})
    probe = probe_bundle()
    (out / PROBE_NAME).write_bytes(probe)
    (out / f"{PROBE_NAME}.sha256").write_bytes(sha_line(PROBE_NAME, probe))
    (out / "lolinet.json").write_text(json.dumps(m, indent=1) + "\n")
    print(f"lolinet.json：lolinet {m['version']}，{len(m['lolinet'])} 个平台；{PROBE_NAME} {hashlib.sha256(probe).hexdigest()[:12]}")


if __name__ == "__main__":
    main()
