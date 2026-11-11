#!/usr/bin/env python3
"""lolinet 出口节点（lolinet exit run）的端到端检查，只用标准库，不要 root。

在回环上起 `lolinet exit run --listen 127.0.0.1:<端口>`（来源默认只放回环），当 SOCKS5 客户端用：
- CONNECT 用域名连公网（example.com:80，读到 HTTP 回应）；
- 回环、云元数据（169.254.169.254）、私网（10.x）这些目标要回 0x02（不允许），--lan 也不放开回环；
- 不支持的命令（BIND）回 0x07；
- UDP ASSOCIATE：经它向 1.1.1.1:53 和 one.one.one.one:53（域名）发 DNS 查询，收到同一个 id 的回答；
- 来源不在 --allow 里：握手前就被关掉，什么都不回。
- 网里用（--mesh，默认跑）：回环上起两个 userspace（无 TUN）的 EasyTier 节点 A、B，A 开出口。经 B 本机的 SOCKS5
  连 A 的 overlay 地址上的出口端口（A 的 EasyTier 把它转给 127.0.0.1），再经出口连 example.com。
  这同时证明 userspace 模式的进（本机 SOCKS5）和出（overlay 地址转回环）都通。

用法: python3 lolinet/check_exit.py <lolinet 二进制> [--offline] [--no-mesh]
  --offline 跳过要连公网的项（CONNECT 公网、UDP DNS、网里用）。
"""
from __future__ import annotations

import ipaddress
import os
import random
import socket
import struct
import shutil
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
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def start(binary: str, *args: str) -> tuple[subprocess.Popen, int]:
    port = free_port()
    p = subprocess.Popen([binary, "exit", "run", "--listen", f"127.0.0.1:{port}", *args],
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    for _ in range(50):
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            return p, port
        except OSError:
            time.sleep(0.1)
    p.kill()
    raise SystemExit(f"lolinet exit run 没起来：{p.stdout.read().decode(errors='replace')}")


def addr_bytes(host: str, port: int) -> bytes:
    try:
        ip = ipaddress.ip_address(host)
        return (b"\x01" if ip.version == 4 else b"\x04") + ip.packed + struct.pack(">H", port)
    except ValueError:
        h = host.encode()
        return b"\x03" + bytes([len(h)]) + h + struct.pack(">H", port)


def recv_exact(s: socket.socket, n: int) -> bytes:
    b = b""
    while len(b) < n:
        chunk = s.recv(n - len(b))
        if not chunk:
            raise ConnectionError("连接关了")
        b += chunk
    return b


def read_reply(s: socket.socket) -> tuple[int, tuple[str, int]]:
    head = recv_exact(s, 4)
    if head[3] == 1:
        ip = socket.inet_ntop(socket.AF_INET, recv_exact(s, 4))
    elif head[3] == 4:
        ip = socket.inet_ntop(socket.AF_INET6, recv_exact(s, 16))
    else:
        ip = recv_exact(s, recv_exact(s, 1)[0]).decode()
    port = struct.unpack(">H", recv_exact(s, 2))[0]
    return head[1], (ip, port)


def socks(port: int, cmd: int, host: str, dport: int, timeout: float = 15) -> tuple[socket.socket, int, tuple[str, int]]:
    s = socket.create_connection(("127.0.0.1", port), timeout=timeout)
    s.sendall(b"\x05\x01\x00")
    if recv_exact(s, 2) != b"\x05\x00":
        raise ConnectionError("方法协商失败")
    s.sendall(b"\x05" + bytes([cmd]) + b"\x00" + addr_bytes(host, dport))
    code, bound = read_reply(s)
    return s, code, bound


def dns_query(name: str) -> tuple[int, bytes]:
    qid = random.randint(1, 0xFFFF)
    q = struct.pack(">HHHHHH", qid, 0x0100, 1, 0, 0, 0)
    q += b"".join(bytes([len(p)]) + p.encode() for p in name.split(".")) + b"\x00" + struct.pack(">HH", 1, 1)
    return qid, q


def udp_dns(port: int, server: str) -> bool:
    ctl, code, bound = socks(port, 3, "0.0.0.0", 0)
    if code != 0:
        return False
    host = "127.0.0.1" if bound[0] in ("0.0.0.0", "::") else bound[0]
    u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    u.settimeout(5)
    try:
        for _ in range(3):
            qid, q = dns_query("example.com")
            u.sendto(b"\x00\x00\x00" + addr_bytes(server, 53) + q, (host, bound[1]))
            try:
                data, _ = u.recvfrom(65536)
            except socket.timeout:
                continue
            if data[2] != 0:
                return False
            off = {1: 10, 4: 22}.get(data[3]) or 7 + data[4]
            reply = data[off:]
            return len(reply) > 12 and struct.unpack(">H", reply[:2])[0] == qid and reply[2] & 0x80 != 0
        return False
    finally:
        u.close()
        ctl.close()


def socks_over(s: socket.socket, host: str, dport: int) -> int:
    """在已经连上 SOCKS5 服务的 s 上做一次 CONNECT，返回回应码"""
    s.sendall(b"\x05\x01\x00")
    if recv_exact(s, 2) != b"\x05\x00":
        raise ConnectionError("方法协商失败")
    s.sendall(b"\x05\x01\x00" + addr_bytes(host, dport))
    return read_reply(s)[0]


def mesh(binary: str) -> None:
    """两个 userspace 节点：B 的本机 SOCKS5 -> A 的 overlay 地址:出口端口 -> A 的出口 -> example.com"""
    tmp = tempfile.mkdtemp(prefix="lolinet-exit-")
    core = os.path.join(tmp, "easytier-core")
    os.symlink(os.path.abspath(binary), core)
    pa, ra, rb, sb, ep = (free_port() for _ in range(5))
    a_ip = "10.188.200.1"
    conf = {
        "a": f'''hostname = "exit-a"
instance_name = "chk-a"
ipv4 = "{a_ip}/24"
dhcp = false
listeners = ["tcp://127.0.0.1:{pa}"]

[network_identity]
network_name = "chk-exit"
network_secret = "chk-exit-secret"

[flags]
no_tun = true
''',
        "b": f'''hostname = "user-b"
instance_name = "chk-b"
ipv4 = "10.188.200.2/24"
dhcp = false
listeners = []
socks5_proxy = "socks5://127.0.0.1:{sb}"

[network_identity]
network_name = "chk-exit"
network_secret = "chk-exit-secret"

[[peer]]
uri = "tcp://127.0.0.1:{pa}"

[flags]
no_tun = true
''',
    }
    procs = []
    try:
        for name, rpc in (("a", ra), ("b", rb)):
            path = os.path.join(tmp, f"{name}.toml")
            with open(path, "w") as f:
                f.write(conf[name])
            log = open(os.path.join(tmp, f"{name}.log"), "w")
            procs.append(subprocess.Popen([core, "-c", path, "--rpc-portal", f"127.0.0.1:{rpc}", "--console-log-level", "warn"],
                                          stdout=log, stderr=subprocess.STDOUT))
        procs.append(subprocess.Popen([binary, "exit", "run", "--listen", f"127.0.0.1:{ep}"],
                                      stdout=subprocess.PIPE, stderr=subprocess.STDOUT))
        got, code, err = b"", None, ""
        deadline = time.time() + 40
        while time.time() < deadline:
            try:
                s = socket.create_connection(("127.0.0.1", sb), timeout=5)
                s.settimeout(15)
                if socks_over(s, a_ip, ep) == 0:
                    code = socks_over(s, "example.com", 80)
                    if code == 0:
                        s.sendall(b"HEAD / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
                        got = s.recv(64)
                s.close()
                if got:
                    break
            except OSError as e:
                err = str(e)
            time.sleep(1)
        check(got.startswith(b"HTTP/1."),
              f"网里用：B 的 SOCKS5 -> {a_ip}:{ep}（A 转回环）-> A 的出口 -> example.com（出口回 {code}，读到 {got[:12]!r}{', ' + err if err and not got else ''}）")
        if not got:
            for name in ("a", "b"):
                with open(os.path.join(tmp, f"{name}.log")) as f:
                    print(f"--- {name}.log\n" + f.read()[-2000:])
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    binary, offline = sys.argv[1], "--offline" in sys.argv[2:]
    p, port = start(binary)
    try:
        print(f"lolinet exit run on 127.0.0.1:{port}")
        if not offline:
            s, code, _ = socks(port, 1, "example.com", 80)
            if code == 0:
                s.sendall(b"HEAD / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
                got = s.recv(64)
            else:
                got = b""
            s.close()
            check(code == 0 and got.startswith(b"HTTP/1."), f"CONNECT example.com:80（回 {code}，读到 {got[:12]!r}）")
        lsn = socket.socket()
        lsn.bind(("127.0.0.1", 0))
        lsn.listen(1)
        for host, dport, what in [("127.0.0.1", lsn.getsockname()[1], "回环"), ("169.254.169.254", 80, "云元数据"),
                                  ("10.255.255.1", 80, "私网"), ("::1", 22, "IPv6 回环"), ("localhost", 22, "解析到回环的域名")]:
            s, code, _ = socks(port, 1, host, dport)
            s.close()
            check(code == 2, f"{what} {host} 不允许（回 {code}）")
        lsn.close()
        s, code, _ = socks(port, 2, "1.1.1.1", 80)
        s.close()
        check(code == 7, f"BIND 不支持（回 {code}）")
        if not offline:
            check(udp_dns(port, "1.1.1.1"), "UDP ASSOCIATE：经出口向 1.1.1.1:53 查 DNS")
            check(udp_dns(port, "one.one.one.one"), "UDP ASSOCIATE：目标是域名 one.one.one.one:53")
    finally:
        p.terminate()
        p.wait(5)

    p, port = start(binary, "--lan")
    try:
        s, code, _ = socks(port, 1, "127.0.0.1", 22)
        s.close()
        check(code == 2, f"--lan 也不放开回环（回 {code}）")
    finally:
        p.terminate()
        p.wait(5)

    p, port = start(binary, "--allow", "10.0.0.0/8")
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=5)
        try:
            s.sendall(b"\x05\x01\x00")
            got = s.recv(2)
        except OSError:
            got = b""
        s.close()
        check(got == b"", f"来源不在 --allow 里：握手前关掉（读到 {got!r}）")
    finally:
        p.terminate()
        p.wait(5)

    if not offline and "--no-mesh" not in sys.argv[2:]:
        mesh(binary)

    if FAILS:
        raise SystemExit(f"{len(FAILS)} 项没过")
    print("全部通过")


if __name__ == "__main__":
    os.environ.setdefault("PYTHONUNBUFFERED", "1")
    main()
