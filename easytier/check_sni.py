#!/usr/bin/env python3
"""验证 easytier-core 的 WSS SNI 补丁真的生效：在本机开一个 TCP 口当「relay」，让 easytier-core 以 wss 连过来，
解析它发出的 TLS ClientHello，核对 SNI。

两种情况都测：URL 里的 ?sni=，以及 URL 不带参数时的 ET_WSS_SNI 环境变量（打洞 / relay 互连靠它）。
没打补丁的版本对 IP 地址会发 localhost，两种情况都会失败。只用标准库，CI（build.yml）和本机都能跑。

用法: python3 easytier/check_sni.py <easytier-core 路径>
"""
from __future__ import annotations

import os
import socket
import subprocess
import sys
import tempfile


def client_hello_sni(data: bytes) -> str | None:
    """TLS 记录 -> ClientHello -> server_name 扩展。只认第一条记录里完整的 ClientHello（几百字节，够了）。"""
    if len(data) < 9 or data[0] != 0x16 or data[5] != 0x01:
        return None
    p = 9 + 2 + 32  # 记录头 5 + 握手头 4，然后 client_version 2 + random 32
    p += 1 + data[p]  # session_id
    p += 2 + int.from_bytes(data[p:p + 2], "big")  # cipher_suites
    p += 1 + data[p]  # compression_methods
    end = p + 2 + int.from_bytes(data[p:p + 2], "big")
    p += 2
    while p + 4 <= min(end, len(data)):
        typ, n = int.from_bytes(data[p:p + 2], "big"), int.from_bytes(data[p + 2:p + 4], "big")
        if typ == 0:  # server_name: list_len 2, name_type 1, name_len 2, name
            ln = int.from_bytes(data[p + 7:p + 9], "big")
            return data[p + 9:p + 9 + ln].decode()
        p += 4 + n
    return None


def run(core: str, url: str, env_sni: str | None, port: int, timeout: float = 40) -> str | None:
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(4)
    srv.settimeout(timeout)
    env = {k: v for k, v in os.environ.items() if k != "ET_WSS_SNI"}
    if env_sni:
        env["ET_WSS_SNI"] = env_sni
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(
            # Linux 上 bind_device 默认开，出站 socket 绑在物理网卡上，连不到 127.0.0.1
            [core, "--no-tun", "--no-listener", "--bind-device", "false", "--network-name", "sni-check",
             "--network-secret", "sni-check", "--rpc-portal", "127.0.0.1:15991", "-p", url],
            env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            conn, _ = srv.accept()
            conn.settimeout(10)
            data = b""
            while len(data) < 5 or len(data) < 5 + int.from_bytes(data[3:5], "big"):
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data += chunk
            conn.close()
            return client_hello_sni(data)
        except socket.timeout:
            log.seek(0)
            sys.stderr.write(log.read()[-2000:].decode(errors="replace"))
            return None
        finally:
            proc.kill()
            proc.wait()
            srv.close()


def main() -> None:
    core = sys.argv[1]
    cases = [
        ("?sni= 参数", "wss://127.0.0.1:18443/?sni=param-check.example", None, "param-check.example", 18443),
        ("ET_WSS_SNI 环境变量", "wss://127.0.0.1:18444/", "env-check.example", "env-check.example", 18444),
    ]
    ok = True
    for label, url, env_sni, want, port in cases:
        got = run(core, url, env_sni, port)
        good = got == want
        ok &= good
        print(f"{'ok  ' if good else 'FAIL'} {label}: SNI = {got!r}（应为 {want!r}）")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
