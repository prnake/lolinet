#!/usr/bin/env python3
"""验证隧道偏好补丁（patches/easytier-v2.7.0-udp-tunnel-pref.patch）：
内层 UDP 每一跳都走 UDP 隧道，KCP 代理包每一跳都走 TCP / WSS 隧道，其他流量照原版按延迟选。

本机起三个 no-tun 内核，全部只绑 127.0.0.1，secure mode：
  A（设备）—— tcp + udp ——> R（relay）<—— tcp + udp —— B（设备），A 和 B 不直连。
A 用端口转发发 UDP 和 TCP 流量，B 用子网代理交给本机的接收端，看两跳每条连接的发送字节。两个场景：
  1. UDP 连接加 20 ms 延迟（默认连接是 tcp）：UDP 流量应该改走 udp；TCP 流量照旧走 tcp。
  2. TCP 连接加 20 ms 延迟（默认连接是 udp），A、B 开 KCP 代理：UDP 流量走 udp；
     TCP 流量被 KCP 代理，KCP 包应该改走 tcp（补丁前跟着默认连接走 udp，KCP 套 UDP 隧道遇到丢包会崩）。
只用标准库。用法: python3 easytier/check_udp_pref.py <easytier-core 路径> [--expect-baseline]
--expect-baseline 用来对照没打补丁的版本：期望两个场景里该被改道的流量都还跟着默认连接走。
"""
from __future__ import annotations

import asyncio
import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time

DELAY = 0.02
NET = "10.211.0"
MAPPED = "10.212.0"  # B 把 127.0.0.0/24 映射成这个网段导出


def free_port(kind=socket.SOCK_STREAM) -> int:
    s = socket.socket(socket.AF_INET, kind)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def in_thread(coro_fn) -> None:
    threading.Thread(target=lambda: asyncio.run(coro_fn()), daemon=True).start()


def start_udp_delay_proxy(listen: int, upstream: int) -> None:
    """UDP 代理：每个方向加 DELAY 秒，不丢包、不限速。"""

    async def main():
        loop = asyncio.get_running_loop()
        flows: dict = {}
        front_box: list = []

        class Up(asyncio.DatagramProtocol):
            def __init__(self, addr):
                self.addr = addr

            def datagram_received(self, data, _):
                loop.call_later(DELAY, front_box[0].sendto, data, self.addr)

        class Front(asyncio.DatagramProtocol):
            def datagram_received(self, data, addr):
                tr = flows.get(addr)
                if tr is None:
                    flows[addr] = "pending"
                    loop.create_task(open_flow(addr, data))
                elif tr != "pending":
                    loop.call_later(DELAY, tr.sendto, data)

        async def open_flow(addr, first):
            tr, _ = await loop.create_datagram_endpoint(lambda: Up(addr), remote_addr=("127.0.0.1", upstream))
            flows[addr] = tr
            loop.call_later(DELAY, tr.sendto, first)

        front, _ = await loop.create_datagram_endpoint(Front, local_addr=("127.0.0.1", listen))
        front_box.append(front)
        await asyncio.Event().wait()

    in_thread(main)


def start_tcp_delay_proxy(listen: int, upstream: int) -> None:
    """TCP 代理：每个方向的每块数据晚 DELAY 秒转发，保序。"""

    async def pump(r, w):
        loop = asyncio.get_running_loop()
        q: asyncio.Queue = asyncio.Queue()

        async def writer():
            while True:
                due, data = await q.get()
                if data is None:
                    break
                await asyncio.sleep(max(0.0, due - loop.time()))
                w.write(data)
                await w.drain()

        task = asyncio.create_task(writer())
        try:
            while data := await r.read(1 << 16):
                await q.put((loop.time() + DELAY, data))
        except OSError:
            pass
        finally:
            await q.put((0, None))
            try:
                await task
            except OSError:
                pass
            w.close()

    async def handle(r, w):
        try:
            ur, uw = await asyncio.open_connection("127.0.0.1", upstream)
        except OSError:
            w.close()
            return
        await asyncio.gather(pump(r, uw), pump(ur, w), return_exceptions=True)

    async def main():
        srv = await asyncio.start_server(handle, "127.0.0.1", listen)
        async with srv:
            await srv.serve_forever()

    in_thread(main)


def start_sinks(udp_port: int, tcp_port: int, counts: dict) -> None:
    u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    u.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4 << 20)
    u.bind(("127.0.0.1", udp_port))
    t = socket.socket()
    t.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    t.bind(("127.0.0.1", tcp_port))
    t.listen(8)

    def udp_loop():
        while True:
            counts["udp"] += len(u.recv(65536))

    def tcp_conn(c):
        while d := c.recv(1 << 16):
            counts["tcp"] += len(d)

    def tcp_loop():
        while True:
            c, _ = t.accept()
            threading.Thread(target=tcp_conn, args=(c,), daemon=True).start()

    for f in (udp_loop, tcp_loop):
        threading.Thread(target=f, daemon=True).start()


def cli(core: str, rpc: int, *args: str):
    exe = os.path.join(os.path.dirname(core), "easytier-cli")
    out = subprocess.run([exe, "-p", f"127.0.0.1:{rpc}", "-o", "json", *args], capture_output=True, text=True, timeout=15)
    return json.loads(out.stdout or "null")


def conns(core: str, rpc: int, peer_name: str) -> dict[str, tuple[int, int]]:
    """本节点到 peer_name 的每种隧道：(累计发送字节, 延迟微秒)。"""
    res: dict[str, tuple[int, int]] = {}
    for it in cli(core, rpc, "-v", "peer") or []:
        if (it.get("route") or {}).get("hostname") != peer_name:
            continue
        for c in (it.get("peer") or {}).get("conns", []):
            kind = c["tunnel"]["tunnel_type"]
            tx, lat = res.get(kind, (0, 0))
            res[kind] = (tx + int(c["stats"]["tx_bytes"]), max(lat, int(c["stats"].get("latency_us") or 0)))
    return res


def tx(core, rpc, peer):
    return {k: v[0] for k, v in conns(core, rpc, peer).items()}


def lat(core, rpc, peer):
    return {k: v[1] for k, v in conns(core, rpc, peer).items()}


def delta(a: dict, b: dict) -> dict:
    return {k: b.get(k, 0) - a.get(k, 0) for k in set(a) | set(b)}


def share(d: dict, kind: str) -> float:
    total = sum(max(0, v) for v in d.values())
    return max(0, d.get(kind, 0)) / total if total else 0.0


def run_scenario(core: str, tmp: str, slow: str, kcp: bool, baseline: bool) -> bool:
    """slow = 哪种隧道加 20 ms 延迟；kcp = A、B 是否开 KCP 代理。返回是否符合预期。"""
    r_tcp, r_udp = free_port(), free_port(socket.SOCK_DGRAM)
    tcp_entry, udp_entry = r_tcp, r_udp
    if slow == "udp":
        udp_entry = free_port(socket.SOCK_DGRAM)
        start_udp_delay_proxy(udp_entry, r_udp)
    else:
        tcp_entry = free_port()
        start_tcp_delay_proxy(tcp_entry, r_tcp)
    sink_udp, sink_tcp = free_port(socket.SOCK_DGRAM), free_port()
    fwd_udp, fwd_tcp = free_port(socket.SOCK_DGRAM), free_port()
    rpc = {"r": free_port(), "a": free_port(), "b": free_port()}
    counts = {"udp": 0, "tcp": 0}
    start_sinks(sink_udp, sink_tcp, counts)

    common = ["--network-name", "udppref-" + secrets.token_hex(4), "--network-secret", secrets.token_hex(16),
              "--secure-mode", "true", "--no-tun", "true", "--disable-p2p", "true",
              "--disable-udp-hole-punching", "true", "--disable-tcp-hole-punching", "true", "--stun-servers"]
    peers = ["-p", f"tcp://127.0.0.1:{tcp_entry}", "-p", f"udp://127.0.0.1:{udp_entry}"]
    dev_flags = ["--enable-kcp-proxy", "true"] if kcp else []
    nodes = {
        "r": ["--hostname", "r", "--ipv4", f"{NET}.1", "-l", f"tcp://127.0.0.1:{r_tcp}", f"udp://127.0.0.1:{r_udp}"],
        "a": ["--hostname", "a", "--ipv4", f"{NET}.2", "--no-listener", *peers, *dev_flags,
              "--port-forward", f"udp://127.0.0.1:{fwd_udp}/{MAPPED}.1:{sink_udp}",
              "--port-forward", f"tcp://127.0.0.1:{fwd_tcp}/{MAPPED}.1:{sink_tcp}"],
        "b": ["--hostname", "b", "--ipv4", f"{NET}.3", "--no-listener", *peers, *dev_flags,
              "-n", f"127.0.0.0/24->{MAPPED}.0/24"],
    }
    print(f"== 场景：{slow} 连接加 {DELAY * 1000:.0f} ms{'，A、B 开 KCP 代理' if kcp else ''}")
    procs = []
    try:
        for name, args in nodes.items():
            log = open(os.path.join(tmp, f"{slow}-{name}.log"), "w")
            procs.append(subprocess.Popen([core, *common, "--rpc-portal", f"127.0.0.1:{rpc[name]}", *args],
                                          stdout=log, stderr=subprocess.STDOUT))

        # 等两跳的 tcp、udp 连接都建好、都量到延迟，并且 A 能路由到 B
        deadline, ka, kb = time.time() + 90, {}, {}
        while time.time() < deadline:
            ka, kb = lat(core, rpc["a"], "r"), lat(core, rpc["r"], "b")
            routes = cli(core, rpc["a"], "route") or []
            if all(k.get("tcp", 0) > 0 and k.get("udp", 0) > 0 for k in (ka, kb)) and any(
                    r.get("hostname") == "b" for r in routes):
                break
            time.sleep(1)
        else:
            print("FAIL: 连接没有在 90 s 内就绪", ka, kb, f"logs in {tmp}")
            return False
        time.sleep(6)  # 默认连接每 5 s 按延迟重选一次
        print("   延迟 us  A→R:", lat(core, rpc["a"], "r"), " R→B:", lat(core, rpc["r"], "b"))

        res = {}
        for kind in ("udp", "tcp"):
            a0, r0, c0 = tx(core, rpc["a"], "r"), tx(core, rpc["r"], "b"), counts[kind]
            payload, total, n = os.urandom(1200), 16 << 20, 0
            if kind == "udp":
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                t0 = time.time()
                while n < total:
                    s.sendto(payload, ("127.0.0.1", fwd_udp))
                    n += len(payload)
                    if n % (120 * 1200) == 0:  # 约 40 Mbps，免得本机回环把包挤掉
                        time.sleep(max(0, t0 + n * 8 / 40e6 - time.time()))
            else:
                s = socket.create_connection(("127.0.0.1", fwd_tcp), timeout=10)
                while n < total:
                    s.sendall(payload * 50)
                    n += len(payload) * 50
                s.close()
            end = time.time() + 40
            while counts[kind] - c0 < total * 0.99 and time.time() < end:
                time.sleep(0.5)
            time.sleep(2)
            da, dr = delta(a0, tx(core, rpc["a"], "r")), delta(r0, tx(core, rpc["r"], "b"))
            got = (counts[kind] - c0) / total
            res[kind] = (share(da, "udp"), share(dr, "udp"), got)
            print(f"   {kind} 流量 {total >> 20} MB，收到 {got:.0%}：A→R 走 udp {share(da, 'udp'):.0%}，"
                  f"R→B 走 udp {share(dr, 'udp'):.0%}  A→R {da}  R→B {dr}")

        ua, ur, ugot = res["udp"]
        ta, tr, tgot = res["tcp"]
        if ugot < 0.5 or tgot < 0.99:
            print(f"FAIL: 流量没送到（udp {ugot:.0%}，tcp {tgot:.0%}），logs in {tmp}")
            return False
        if baseline:
            # 没打补丁都跟着默认连接走：场景 1 默认 tcp，UDP 流量也走 tcp；场景 2 默认 udp，KCP 包也走 udp
            ok = (ua < 0.5 and ur < 0.5) if slow == "udp" else (ta > 0.5 and tr > 0.5)
            print("ok   对照组：该改道的流量还跟着默认连接走" if ok else "FAIL: 对照组里流量被改道了")
            return ok
        # 补丁后：UDP 两跳都走 udp；TCP 两跳都走 tcp（场景 1 靠默认连接，场景 2 靠 KCP 偏好）
        ok = ua >= 0.9 and ur >= 0.9 and ta < 0.1 and tr < 0.1
        print("ok" if ok else "FAIL: UDP 流量没有两跳都走 udp，或 TCP / KCP 流量没有两跳都走 tcp")
        return ok
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()


def main() -> int:
    core = os.path.abspath(sys.argv[1])
    baseline = "--expect-baseline" in sys.argv
    tmp = tempfile.mkdtemp(prefix="udppref-")
    # lolinet 是多入口二进制，按 argv[0] 分发；原版 easytier-core 旁边就有 easytier-cli
    if not os.path.exists(os.path.join(os.path.dirname(core), "easytier-cli")):
        os.symlink(core, os.path.join(tmp, "easytier-core"))
        os.symlink(core, os.path.join(tmp, "easytier-cli"))
        core = os.path.join(tmp, "easytier-core")
    ok = run_scenario(core, tmp, "udp", False, baseline)
    ok = run_scenario(core, tmp, "tcp", True, baseline) and ok
    print("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
