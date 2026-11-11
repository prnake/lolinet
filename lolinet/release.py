#!/usr/bin/env python3
"""lolinet 的发布清单 lolinet.json，lolidesk 的发布清单 lolidesk.json（只用标准库）。

清单写「设备应该跑哪个版本」，lolinet upgrade 按它换（见 lolinet/src/upgrade.rs）：
  {"version": "<lolinet --version 的版本>",
   "lolinet": {"<macos|linux>-<aarch64|x86_64>": {"file": "lolinet-<平台>.tar.gz", "sha256": "..."}}}
版本取包里 BUILD-INFO 的第一行（lolinet <版本>），四个平台必须一致。清单和包放同一个目录：准入站 dl/，或者一个 GitHub Release。
设备上的公开探针编在 lolinet 里（lolinet probe），跟着 lolinet 一起换；以前清单里的 "probe"（python 版 reporter.py 的包）不再发，
老版本 lolinet 读到没有 "probe" 的清单就不管探针。

lolidesk.json 同样的格式，键是 lolidesk；版本取 BUILD-INFO 第一行 lolidesk <版本>（= lolidesk --version）。
lolinet upgrade 按它换设备上装了的 lolidesk，join/install.sh 按它装。

用法（公开仓库 build.yml / desk.yml 发 GitHub Release）：
  python3 lolinet/release.py <放了 lolinet-*.tar.gz 的目录>
  在同一个目录写 lolinet.json。
  python3 lolinet/release.py --lolidesk <放了 lolidesk-*.tar.gz 的目录>
  在同一个目录写 lolidesk.json。
准入站用 join/deploy.py release / lolidesk <目录>：目录是那一版 Release 的文件，原样传到 dl/（清单用同样的函数再算一遍核对）。
"""
from __future__ import annotations

import hashlib
import io
import json
import sys
import tarfile
from pathlib import Path

PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-aarch64", "macos-x86_64")


def version_of(tgz: bytes, product: str = "lolinet") -> str:
    with tarfile.open(fileobj=io.BytesIO(tgz)) as t:
        member = t.extractfile("BUILD-INFO")
        first = member.read().decode().splitlines()[0] if member else ""
    if not first.startswith(f"{product} "):
        raise SystemExit(f"BUILD-INFO 第一行不是 {product} <版本>：{first!r}")
    return first.split(" ", 1)[1].strip()


def manifest(packages: dict[str, bytes], product: str = "lolinet") -> dict:
    """packages：文件名 -> 内容，要有四个平台的 <product>-<平台>.tar.gz。product 是 lolinet 或 lolidesk"""
    files, versions = {}, {}
    for p in PLATFORMS:
        name = f"{product}-{p}.tar.gz"
        if name not in packages:
            raise SystemExit(f"缺 {name}：清单要四个平台都在")
        versions[p] = version_of(packages[name], product)
        files[p] = {"file": name, "sha256": hashlib.sha256(packages[name]).hexdigest()}
    if len(set(versions.values())) != 1:
        raise SystemExit(f"四个平台的版本不一致：{versions}")
    return {"version": next(iter(versions.values())), product: files}


def main() -> None:
    if len(sys.argv) == 3 and sys.argv[1] == "--lolidesk":
        out = Path(sys.argv[2])
        m = manifest({f.name: f.read_bytes() for f in out.glob("lolidesk-*.tar.gz")}, "lolidesk")
        (out / "lolidesk.json").write_text(json.dumps(m, indent=1) + "\n")
        print(f"lolidesk.json：lolidesk {m['version']}，{len(m['lolidesk'])} 个平台")
        return
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    out = Path(sys.argv[1])
    m = manifest({f.name: f.read_bytes() for f in out.glob("lolinet-*.tar.gz")})
    (out / "lolinet.json").write_text(json.dumps(m, indent=1) + "\n")
    print(f"lolinet.json：lolinet {m['version']}，{len(m['lolinet'])} 个平台")


if __name__ == "__main__":
    main()
