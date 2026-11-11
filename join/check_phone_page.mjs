// 手机兑换页（worker.js 的 /m）在真浏览器里的检查：配对码在浏览器里解开配对包，和 openssl 加密的原文逐字节一致。
// 从 worker.js 原样取出 phonePage()，本机起一个替身准入站（/m 和一次性的 /v1/pair/<id>），无头 Chrome 经 DevTools 协议打开页面：
//   1. /m#<配对码>：解出原文（含中文），地址栏里的配对码已去掉，下载链接是 blob:
//   2. 同一个码再来一次：配对包已经取走，提示「已经用过」
//   3. 码的位数不对：提示要 12 位
// 配对包用 openssl 加密，参数和 web/agent/mesh_manager.py、join/install.sh 一样。
// 用法: node join/check_phone_page.mjs [chrome 路径]（不给就找 $CHROME、常见安装位置；找不到就跳过）
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

const here = path.dirname(new URL(import.meta.url).pathname);
const chrome = [process.argv[2], process.env.CHROME, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium", "/usr/bin/chromium-browser"]
  .find((p) => p && fs.existsSync(p));
if (!chrome) {
  console.log("没找到 Chrome，跳过手机兑换页的检查");
  process.exit(0);
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "lolinet-phone-"));
const code = "K7QM4TRA2WXZ";
const plain = "mixed-port: 7890\n# 中文注释：lolinet 手机配置\nproxies:\n  - name: lolinet\n    type: wireguard\n";
fs.writeFileSync(path.join(tmp, "plain.yaml"), plain);
fs.writeFileSync(path.join(tmp, "pass"), code + "\n");
const blobText = execFileSync("openssl", ["enc", "-aes-256-cbc", "-pbkdf2", "-iter", "200000", "-md", "sha256", "-a", "-A", "-salt",
  "-pass", `file:${path.join(tmp, "pass")}`, "-in", path.join(tmp, "plain.yaml")], { encoding: "utf8" });
const id = createHash("sha256").update(code).digest("hex").slice(0, 32);

const src = fs.readFileSync(path.join(here, "worker.js"), "utf8");
const fnSrc = src.slice(src.indexOf("function phonePage(env)"), src.indexOf("const esc = (s) =>"));
const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const phonePage = new Function("esc", "title", "baseUrl", `${fnSrc}; return phonePage;`)(esc, () => "lolinet", () => "");

let blob = blobText;
const server = http.createServer(async (req, res) => {
  if (req.url === "/m") {
    const r = phonePage({});
    res.writeHead(200, Object.fromEntries(r.headers));
    return res.end(await r.text());
  }
  if (req.url === `/v1/pair/${id}` && blob) {
    const b = blob;
    blob = null; // 一次性，和 Worker 一样
    res.writeHead(200, { "content-type": "text/plain" });
    return res.end(b);
  }
  res.writeHead(404);
  res.end("invalid or expired code");
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

const port = 9000 + Math.floor(Math.random() * 1000);
const browser = spawn(chrome, ["--headless=new", "--disable-gpu", "--no-first-run", "--no-sandbox", `--user-data-dir=${path.join(tmp, "profile")}`,
  `--remote-debugging-port=${port}`, "about:blank"], { stdio: "ignore" });
let failed = 0;
try {
  let ver;
  for (let i = 0; i < 50 && !ver; i++) {
    ver = await fetch(`http://127.0.0.1:${port}/json/version`).then((r) => r.json()).catch(() => null);
    if (!ver) await new Promise((r) => setTimeout(r, 200));
  }
  if (!ver) throw new Error("Chrome 的调试端口没起来");

  async function open(url) {
    const t = await (await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, { method: "PUT" })).json();
    const ws = new WebSocket(t.webSocketDebuggerUrl);
    await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
    let n = 0;
    const pending = new Map();
    ws.onmessage = (m) => { const d = JSON.parse(m.data); pending.get(d.id)?.(d); pending.delete(d.id); };
    const call = (method, params) => new Promise((r) => { pending.set(++n, r); ws.send(JSON.stringify({ id: n, method, params })); });
    const expr = `JSON.stringify({s: document.getElementById("s")?.textContent || "", shown: !document.getElementById("r")?.hidden,
      y: document.getElementById("y")?.value || "", url: location.href, dl: (document.getElementById("dl")?.href || "").slice(0, 5)})`;
    let st = {};
    for (let i = 0; i < 60; i++) {
      st = JSON.parse((await call("Runtime.evaluate", { expression: expr, returnByValue: true })).result.result.value || "{}");
      if (st.s && !st.s.endsWith("中…")) break;
      await new Promise((r) => setTimeout(r, 250));
    }
    ws.close();
    return st;
  }

  const check = (ok, what, st) => {
    console.log(`  ${ok ? "ok  " : "FAIL"}  ${what}${ok ? "" : `：${JSON.stringify(st)}`}`);
    if (!ok) failed++;
  };
  let st = await open(`${base}/m#K7QM-4TRA-2WXZ`);
  check(st.shown && st.y === plain && !st.url.includes("#") && st.dl === "blob:", "配对码在浏览器里解开配对包，原文一致，地址栏里的码已去掉", st);
  st = await open(`${base}/m#K7QM-4TRA-2WXZ`);
  check(!st.shown && st.s.includes("已经用过"), "同一个码第二次：已经用过", st);
  st = await open(`${base}/m#ABCD`);
  check(!st.shown && st.s.includes("12 位"), "位数不对：提示 12 位", st);
} finally {
  const exited = new Promise((r) => browser.once("exit", r));
  browser.kill();
  await Promise.race([exited, new Promise((r) => setTimeout(r, 5000))]);
  server.close();
  // Chrome 退出时还会往 profile 里写几下
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}
if (failed) {
  console.error(`${failed} 项没过`);
  process.exit(1);
}
console.log("手机兑换页：全部通过");
