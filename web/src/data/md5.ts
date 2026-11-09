// easytier-web 的登录接口要 md5(password)；WebCrypto 没有 MD5，这里放一个最小实现（RFC 1321）。
// 只在「浏览器自己登录」模式用；agent 能读到 state/mesh-console.json 时根本不走这里。

const S = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
const K = Array.from({ length: 64 }, (_, i) => Math.floor(Math.abs(Math.sin(i + 1)) * 2 ** 32) >>> 0);

export function md5(text: string): string {
  const bytes = new TextEncoder().encode(text);
  const len = ((bytes.length + 8) >>> 6) * 16 + 16;
  const w = new Uint32Array(len);
  bytes.forEach((b, i) => (w[i >> 2] |= b << ((i % 4) * 8)));
  w[bytes.length >> 2] |= 0x80 << ((bytes.length % 4) * 8);
  w[len - 2] = (bytes.length * 8) >>> 0;
  w[len - 1] = Math.floor((bytes.length * 8) / 2 ** 32);

  let a0 = 0x67452301, b0 = 0xefcdab89, c0 = 0x98badcfe, d0 = 0x10325476;
  for (let o = 0; o < len; o += 16) {
    let a = a0, b = b0, c = c0, d = d0;
    for (let i = 0; i < 64; i++) {
      const r = i >> 4;
      let f: number, g: number;
      if (r === 0) (f = (b & c) | (~b & d)), (g = i);
      else if (r === 1) (f = (d & b) | (~d & c)), (g = (5 * i + 1) % 16);
      else if (r === 2) (f = b ^ c ^ d), (g = (3 * i + 5) % 16);
      else (f = c ^ (b | ~d)), (g = (7 * i) % 16);
      const t = d;
      d = c;
      c = b;
      const x = (a + f + K[i] + w[o + g]) >>> 0;
      const s = S[r * 4 + (i % 4)];
      b = (b + ((x << s) | (x >>> (32 - s)))) >>> 0;
      a = t;
    }
    a0 = (a0 + a) >>> 0;
    b0 = (b0 + b) >>> 0;
    c0 = (c0 + c) >>> 0;
    d0 = (d0 + d) >>> 0;
  }
  return [a0, b0, c0, d0]
    .map((v) => Array.from({ length: 4 }, (_, i) => ((v >>> (i * 8)) & 255).toString(16).padStart(2, "0")).join(""))
    .join("");
}
