#!/usr/bin/env python3
"""lolidesk 配对的端到端检查（只用标准库）：回环上起一台主机，几台「设备」各用自己的 HOME。

- 主人 `lolidesk host code` 发配对码和分享链接，设备 A `lolidesk pair <链接>` 配对、能列会话；
- 设备 B `lolidesk pair <主机>` 请求同意，主人 `host access` 看到请求、`host approve` 同意，B 配对上；
- 设备 C 请求被 `host deny` 拒绝，马上再请求被挡（冷却）；
- A `lolidesk share` 发分享链接，D 用它配对；主人的 access 里 D 挂在 A 的分享链接下面，有最近登录；
- `host limit <B> cua off` 后 B 调 computer use 被拒（读鼠标位置，不截图、不注入）；
- 吊销 A：A、A 的分享链接、D 一起失效（连不上），B 还能连；
- 老的主机 token（--token）还是主人；错的凭据被拒。

不弹系统对话框（LOLIDESK_NO_DIALOG=1）。用法: python3 desk/scripts/check_pairing.py <lolidesk 二进制>
"""
from __future__ import annotations

import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

FAILS: list[str] = []


def check(ok: bool, what: str) -> None:
    print(("  ok    " if ok else "  FAIL  ") + what)
    if not ok:
        FAILS.append(what)


def free_port() -> int:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    binary = os.path.abspath(sys.argv[1])
    root = tempfile.mkdtemp(prefix="lolidesk-pair-")
    homes = {}
    for n in ("owner", "a", "b", "c", "d"):
        homes[n] = os.path.join(root, n)
        os.makedirs(homes[n])

    def run(who: str, *args: str, timeout: float = 60) -> subprocess.CompletedProcess:
        env = {**os.environ, "HOME": homes[who], "XDG_CONFIG_HOME": os.path.join(homes[who], ".config"),
               "LOLIDESK_NO_DIALOG": "1", "SHELL": "/bin/sh"}
        return subprocess.run([binary, *args], capture_output=True, text=True, timeout=timeout, env=env)

    def bg(who: str, *args: str) -> subprocess.Popen:
        env = {**os.environ, "HOME": homes[who], "XDG_CONFIG_HOME": os.path.join(homes[who], ".config"),
               "LOLIDESK_NO_DIALOG": "1", "SHELL": "/bin/sh"}
        return subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)

    port = free_port()
    host = f"127.0.0.1:{port}"
    token = re.search(r"token\s+(\S+)", run("owner", "host", "token").stdout).group(1)
    log = open(os.path.join(root, "host.log"), "w")
    env = {**os.environ, "HOME": homes["owner"], "XDG_CONFIG_HOME": os.path.join(homes["owner"], ".config"),
           "LOLIDESK_NO_DIALOG": "1", "SHELL": "/bin/sh"}
    hostp = subprocess.Popen([binary, "host", "run", "--listen", host], stdout=log, stderr=subprocess.STDOUT, env=env)
    procs = [hostp]
    try:
        for _ in range(50):
            if "配对给了" in run("owner", "host", "access").stdout and hostp.poll() is None:
                break
            time.sleep(0.2)

        # ---- 配对码 / 分享链接 ----
        r = run("owner", "host", "code", "--label", "ci")
        link = re.search(r"lolidesk://pair\S+", r.stdout)
        code_id = re.search(r"（(c-[0-9a-f]+)）", r.stdout)
        check(r.returncode == 0 and link and code_id, f"主人发配对码和分享链接（{r.stderr.strip()[-200:]}）")
        link, code_id = link.group(0), code_id.group(1)
        r = run("a", "pair", link)
        check(r.returncode == 0 and "已和" in r.stdout, f"A 用分享链接配对（{(r.stderr or r.stdout).strip()[-200:]}）")
        r = run("a", host, "--sessions")
        check(r.returncode == 0, f"A 配对后不用 token 就能连（{r.stderr.strip()[-200:]}）")

        # ---- 请求同意 ----
        pb = bg("b", "pair", host, "-y")
        procs.append(pb)
        pending = None
        for _ in range(50):
            m = re.search(r"#(\d+)\s+\S+", run("owner", "host", "access").stdout)
            if m:
                pending = m.group(1)
                break
            time.sleep(0.2)
        check(pending is not None, "主人在 host access 里看到 B 的配对请求")
        r = run("owner", "host", "approve", pending or "0")
        check(r.returncode == 0, f"主人同意（{r.stderr.strip()[-200:]}）")
        out, err = pb.communicate(timeout=30)
        check(pb.returncode == 0 and "已和" in out, f"B 等到同意、配对上（{(err or out).strip()[-200:]}）")
        r = run("b", host, "--sessions")
        check(r.returncode == 0, f"B 之后直接连（{r.stderr.strip()[-200:]}）")

        # ---- 拒绝、冷却 ----
        pc = bg("c", "pair", host, "-y")
        procs.append(pc)
        pending = None
        for _ in range(50):
            m = re.search(r"#(\d+)\s+\S+", run("owner", "host", "access").stdout)
            if m:
                pending = m.group(1)
                break
            time.sleep(0.2)
        run("owner", "host", "deny", pending or "0")
        out, err = pc.communicate(timeout=30)
        check(pc.returncode != 0 and "拒绝" in err, f"C 被拒绝（{err.strip()[-120:]}）")
        r = run("c", "pair", host, "-y")
        check(r.returncode != 0 and "刚被拒绝过" in r.stderr, f"C 马上再请求被挡（{r.stderr.strip()[-120:]}）")
        r = run("c", host, "--sessions", "-y")
        check(r.returncode != 0, "C 没配对上，连不进来")

        # ---- A 的分享链接给 D ----
        r = run("a", "share", host, "--label", "给 D")
        share = re.search(r"lolidesk://pair\S+", r.stdout)
        check(r.returncode == 0 and share, f"A 发分享链接（{(r.stderr or r.stdout).strip()[-200:]}）")
        r = run("d", "pair", share.group(0) if share else "x")
        check(r.returncode == 0, f"D 用 A 的分享链接配对（{(r.stderr or r.stdout).strip()[-200:]}）")
        run("d", host, "--sessions")
        acc = run("owner", "host", "access").stdout
        check("配对给了 3 台设备" in acc and "的分享链接" in acc and "「给 D」" in acc and "最近 刚才" in acc,
              "主人的 access：3 台设备（A、B、D），D 挂在 A 的分享链接下面，有最近登录")
        if FAILS:
            print(acc)
        r = run("a", "access", host)
        check(r.returncode == 0 and "（我）" in r.stdout and "「给 D」" in r.stdout and "「ci」" not in r.stdout,
              "A 的 access 只看到自己和自己的分享链接")

        # ---- 限制 ----
        b_line = [l for l in acc.splitlines() if "经主人同意" in l]
        b_id = re.findall(r"(g-[0-9a-f]+)", acc.split("经主人同意", 1)[1].split("配对码", 1)[0]) if b_line else []
        check(len(b_id) == 1, "access 里分得出 B（经主人同意那组）")
        if b_id:
            r = run("owner", "host", "limit", b_id[0], "cua", "off")
            check(r.returncode == 0 and "agent" not in r.stdout.split("能：", 1)[-1], f"限制 B 不能用 computer use（{r.stdout.strip()}）")
            r = run("b", "cua", host, "cursor")
            check(r.returncode != 0 and "没给这台设备开放" in r.stderr, f"B 调 computer use 被拒（{r.stderr.strip()[-120:]}）")
            run("owner", "host", "limit", b_id[0], "--reset")

        # ---- 吊销 A：A、A 的分享链接、D 一起失效 ----
        acc = run("owner", "host", "access").stdout
        a_ids = re.findall(r"(g-[0-9a-f]+)", acc.split("「ci」", 1)[1].split("\n\n", 1)[0]) if "「ci」" in acc else []
        check(len(a_ids) == 1, "access 里分得出 A（「ci」那个码下面）")
        r = run("owner", "host", "revoke", a_ids[0] if a_ids else "g-0")
        check(r.returncode == 0, f"吊销 A（{r.stderr.strip()[-120:]}）")
        ra, rd, rb = run("a", host, "--sessions"), run("d", host, "--sessions"), run("b", host, "--sessions")
        check(ra.returncode != 0 and "吊销" in ra.stderr, f"A 连不上了（{ra.stderr.strip()[-100:]}）")
        check(rd.returncode != 0, "D（A 分享的）也连不上了")
        check(rb.returncode == 0, "B 不受影响")
        acc = run("owner", "host", "access").stdout
        check("配对给了 1 台设备" in acc and "给 D" not in acc, "access 里只剩 B，A 的分享链接也没了")

        # ---- 吊销配对码 ----
        r = run("owner", "host", "revoke", code_id)
        check(r.returncode == 0, "吊销主人发的码（它下面的已经没了）")

        # ---- 老 token、错凭据 ----
        r = run("c", host, "--token", token, "--sessions", "-y")
        check(r.returncode == 0, "主机 token 还是主人的钥匙")
        r = run("c", host, "--token", "g1.g-00000000.00", "--sessions", "-y")
        check(r.returncode != 0, "错的凭据被拒")
    finally:
        for p in procs:
            if p.poll() is None:
                p.terminate()
        for p in procs:
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()
        log.close()
        if FAILS:
            print("--- host.log\n" + open(os.path.join(root, "host.log")).read()[-3000:])
        shutil.rmtree(root, ignore_errors=True)
    if FAILS:
        raise SystemExit(f"{len(FAILS)} 项没过")
    print("全部通过")


if __name__ == "__main__":
    main()
