// lolinet 准入站 Worker。绑定自定义域（join/deploy.py 按节点清单挂）：
//   BASE_URL 的主机        首次准入：安装脚本、配对码兑换（一次性密文）、客户端二进制（R2）；
//   （如 join.edge.example.com）首页是公开的只读拓扑探针（浏览器看页面，curl 看安装命令）
//   <name>.<EDGE_ZONE>     其他名字：探测内网可达后跳到 <name>-local.<EDGE_ZONE>（内网服务本身不暴露公网）
// 绑定：PAIRING = KV；DIST = R2（install.sh、dl/、site/ 探针页面）；
//       PROBE = Durable Object ProbeHub（各 relay / 设备上报的状态和历史）；PROBE_MASTER = 上报签名主密钥；
//       RELAYS = 节点清单里的 relay 名单（JSON：name / region / spec）；
//       BASE_URL = 准入站地址（LOLINET_BASE_URL，不设就是节点清单的 network.join），发 install.sh 时填进去；
//       EDGE_ZONE = 节点清单的 network.edge_zone；
//       TITLE = 网络的显示名（节点清单的 network.title，不设就是 lolinet）。
// 公开接口不出现任何名字：relay 用区域代码当 id（页面上写城市），设备用 d-<10 位十六进制>（HMAC(devid_key, 主机名)，
// relay 和设备各自算好再报，Worker 根本拿不到设备名）。设备只公开硬件、位置，以及它直连了哪些接入点。
// relay 名字只用来验签和在 DO 里关联历史，不出 Worker。
import { DurableObject } from "cloudflare:workers";

const joinHost = (env) => new URL(env.BASE_URL).hostname;
const baseUrl = (env) => env.BASE_URL.replace(/\/+$/, "");
const title = (env) => env.TITLE || "lolinet";
const RATE_LIMIT = 20; // 每 IP 每 10 分钟兑换次数
const REPORT_MAX = 16 * 1024;
const SKEW = 300; // 上报时间戳和 Worker 时钟允许差多少秒
const ONLINE_WITHIN = 90; // 多久没上报算离线（上报间隔 30 秒）
const DEVICE_ONLINE_WITHIN = 180; // 设备 60 秒一次心跳
const DEVICE_KEEP = 7 * 86400; // 设备 7 天没上报就不再公开（卸载了或换了 id）
const DEVICE_ID = /^d-[0-9a-f]{10}$/;
const BUCKET = 300; // 历史按 5 分钟一格
const TZ = 8 * 3600; // 「天」按北京时间切

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    if (url.hostname === joinHost(env) && url.pathname === "/v1/probe/report") {
      return req.method === "POST" ? report(req, env) : text("method not allowed", 405);
    }
    if (req.method !== "GET" && req.method !== "HEAD") return text("method not allowed", 405);
    if (url.hostname === joinHost(env)) return join(req, env, url);
    return probePage(url, env);
  },
};

async function join(req, env, url) {
  const path = url.pathname;
  if (path === "/") {
    if ((req.headers.get("accept") || "").includes("text/html")) {
      const page = await site(env, "index.html");
      if (page.status === 200) return page; // 页面还没传上去时退回文字说明
    }
    return text(`${title(env)} 准入\n\n  curl -fsSL ${baseUrl(env)}/i | sudo sh -s <配对码>\n\n状态：${baseUrl(env)}/v1/probe\n`);
  }
  if (path === "/favicon.svg" || /^\/assets\/[A-Za-z0-9._-]+$/.test(path)) return site(env, path.slice(1));
  if (path === "/v1/probe") return publicJson(await hub(env).fetch("https://hub/snapshot"));
  if (path === "/v1/probe/link") return publicJson(await hub(env).fetch("https://hub/link" + url.search));
  if (path === "/i") return installScript(env);
  if (path.startsWith("/dl/")) {
    const name = path.slice(4);
    if (!/^[A-Za-z0-9._-]+$/.test(name)) return text("not found", 404);
    return fromR2(env, `dl/${name}`, "application/octet-stream");
  }
  const m = path.match(/^\/v1\/pair\/([0-9a-f]{32})$/);
  if (m) {
    if (req.method !== "GET") return text("method not allowed", 405); // HEAD 也会走到删除，不能消耗配对码
    const ip = req.headers.get("cf-connecting-ip") || "unknown";
    const bucket = `rl:${ip}:${Math.floor(Date.now() / 600000)}`;
    const used = parseInt((await env.PAIRING.get(bucket)) || "0", 10);
    if (used >= RATE_LIMIT) return text("too many attempts", 429);
    await env.PAIRING.put(bucket, String(used + 1), { expirationTtl: 900 });
    const key = `pair:${m[1]}`;
    const blob = await env.PAIRING.get(key);
    if (!blob) return text("invalid or expired code", 404);
    await env.PAIRING.delete(key); // 一次性
    return new Response(blob, { headers: { "content-type": "text/plain", "cache-control": "no-store" } });
  }
  return text("not found", 404);
}

async function fromR2(env, key, type) {
  const obj = await env.DIST.get(key);
  if (!obj) return text("not found", 404);
  return new Response(obj.body, { headers: { "content-type": type, "cache-control": "no-store", etag: obj.httpEtag } });
}

/** install.sh 里 BASE 的默认值是占位符，发出去时换成本站地址：同一份脚本，谁部署就指向谁 */
async function installScript(env) {
  const obj = await env.DIST.get("install.sh");
  if (!obj) return text("not found", 404);
  const body = (await obj.text()).replaceAll("@LOLINET_BASE_URL@", baseUrl(env));
  return new Response(body, { headers: { "content-type": "text/x-shellscript; charset=utf-8", "cache-control": "no-store" } });
}

const TYPES = { html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", css: "text/css; charset=utf-8",
  svg: "image/svg+xml", woff2: "font/woff2", json: "application/json" };

/** 探针页面：web 的 probe 构建产物放在 R2 site/ 下。带 hash 的 assets 长缓存，index.html 每次验证。 */
async function site(env, key) {
  const obj = await env.DIST.get(`site/${key}`);
  if (!obj) return text("not found", 404);
  const ext = key.split(".").pop();
  return new Response(obj.body, { headers: {
    "content-type": TYPES[ext] || "application/octet-stream",
    "cache-control": key.startsWith("assets/") ? "public, max-age=31536000, immutable" : "no-cache",
    "x-content-type-options": "nosniff",
    etag: obj.httpEtag,
  } });
}

function publicJson(r) {
  const h = new Headers(r.headers);
  h.set("cache-control", "public, max-age=10");
  h.set("access-control-allow-origin", "*");
  return new Response(r.body, { status: r.status, headers: h });
}

const hub = (env) => env.PROBE.get(env.PROBE.idFromName("global"));
const enc = (s) => new TextEncoder().encode(s);

function hex(s) {
  if (!/^[0-9a-f]*$/.test(s) || s.length % 2) return null;
  return Uint8Array.from(s.match(/../g) || [], (b) => parseInt(b, 16));
}

/** 每台 relay 的上报密钥 = HMAC-SHA256(主密钥, "probe:<名字>")；和 probe/device.py 一致。 */
async function nodeKey(env, node) {
  const master = await crypto.subtle.importKey("raw", hex(env.PROBE_MASTER), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const raw = await crypto.subtle.sign("HMAC", master, enc(`probe:${node}`));
  return crypto.subtle.importKey("raw", raw, { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
}

async function report(req, env) {
  const node = req.headers.get("x-probe-node") || "";
  const ts = Number(req.headers.get("x-probe-ts"));
  const sig = hex(req.headers.get("x-probe-sig") || "");
  // 设备不用名单：任何 d-<id> 只要用主密钥派生的那把 key 签了名就收，所以加设备不用改 Worker 配置（CI 也不需要知道设备）
  if (!DEVICE_ID.test(node) && !JSON.parse(env.RELAYS).some((r) => r.name === node)) return text("unknown node", 403);
  if (!Number.isInteger(ts) || Math.abs(Date.now() / 1000 - ts) > SKEW) return text("bad timestamp", 403);
  if (!sig || sig.length !== 32) return text("bad signature", 403);
  if (Number(req.headers.get("content-length") || 0) > REPORT_MAX) return text("too large", 413);
  const body = new Uint8Array(await req.arrayBuffer());
  if (body.byteLength > REPORT_MAX) return text("too large", 413);
  const prefix = enc(`${ts}.`);
  const msg = new Uint8Array(prefix.length + body.length);
  msg.set(prefix);
  msg.set(body, prefix.length);
  if (!(await crypto.subtle.verify("HMAC", await nodeKey(env, node), sig, msg))) return text("bad signature", 403);
  return hub(env).fetch("https://hub/report", { method: "POST", headers: { "x-probe-node": node, "x-probe-ts": String(ts) }, body });
}

// ---------- 状态与历史 ----------

const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v, n) => (typeof v === "string" ? v.slice(0, n) : null);
const pair = (v) => (Array.isArray(v) && v.length === 2 ? v.map(num) : null);

/** 上报虽然验过签，也只留认识的字段；链路只留到其他 relay 的。 */
function clean(d, node, names) {
  const h = d.host || {};
  const net = h.net || {};
  const et = d.et || {};
  return {
    host: {
      os: str(h.os, 64), cpu_model: str(h.cpu_model, 64), cores: num(h.cores), cpu: num(h.cpu), uptime: num(h.uptime),
      load: Array.isArray(h.load) ? h.load.slice(0, 3).map(num) : null,
      mem: pair(h.mem), swap: pair(h.swap), disk: pair(h.disk),
      net: { rx: num(net.rx), tx: num(net.tx), rx_total: num(net.rx_total), tx_total: num(net.tx_total) },
    },
    et: {
      ok: et.ok === true, version: str(et.version, 32), devices: num(et.devices),
      // relay 看到的设备：只有匿名 id、是否直连、延迟
      peers: (Array.isArray(et.peers) ? et.peers : [])
        .filter((x) => x && DEVICE_ID.test(x.id))
        .slice(0, 256)
        .map((x) => ({ id: x.id, direct: x.direct === true, lat: num(x.lat) })),
      links: (Array.isArray(et.links) ? et.links : [])
        .filter((l) => l && names.has(l.peer) && l.peer !== node)
        .slice(0, 32)
        .map((l) => ({ peer: l.peer, direct: l.direct === true, lat: num(l.lat), loss: num(l.loss), proto: str(l.proto, 8) })),
    },
  };
}

/** 设备只留硬件和位置。near 是它最近的 relay（名字，只在这里用），公开时换成区域代码。 */
function cleanDevice(d, names) {
  const v = d.device || {};
  const near = v.near && names.has(v.near.peer) ? v.near.peer : null;
  return {
    kind: str(v.kind, 24), model: str(v.model, 48), chip: str(v.chip, 64), cores: num(v.cores), mem: num(v.mem),
    os: str(v.os, 48), arch: str(v.arch, 16), location: str(v.location, 32), near,
    disks: (Array.isArray(v.disks) ? v.disks : []).slice(0, 8).map((x) => ({ label: str(x?.label, 24), total: num(x?.total) })),
  };
}

export class ProbeHub extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS latest (node TEXT PRIMARY KEY, ts INTEGER, data TEXT)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS beats (node TEXT, bucket INTEGER, n INTEGER, cpu REAL, rx REAL, tx REAL, PRIMARY KEY (node, bucket))");
    this.sql.exec("CREATE TABLE IF NOT EXISTS links (a TEXT, b TEXT, bucket INTEGER, n INTEGER, lat REAL, PRIMARY KEY (a, b, bucket))");
    this.sql.exec("CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, first INTEGER, ts INTEGER, data TEXT)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v INTEGER)");
    const ver = this.sql.exec("SELECT v FROM meta WHERE k = 'schema'").toArray()[0]?.v ?? 1;
    if (ver < 2) {
      // v2：设备 id 改成按 EasyTier 主机名派生（和 relay 报的对得上），旧 id 作废；设备 60 秒内会用新 id 重新上报
      this.sql.exec("DELETE FROM devices");
      this.sql.exec("INSERT INTO meta (k, v) VALUES ('schema', 2) ON CONFLICT(k) DO UPDATE SET v = 2");
    }
  }

  relays() {
    return JSON.parse(this.env.RELAYS);
  }

  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/report") {
      let data;
      try {
        data = JSON.parse(await req.text());
      } catch {
        return text("bad json", 400);
      }
      return this.ingest(req.headers.get("x-probe-node"), Number(req.headers.get("x-probe-ts")), data);
    }
    if (url.pathname === "/snapshot") return json(this.snapshot());
    if (url.pathname === "/link") return json(this.link(url.searchParams.get("a"), url.searchParams.get("b")));
    return text("not found", 404);
  }

  ingestDevice(id, ts, data) {
    const prev = this.sql.exec("SELECT ts FROM devices WHERE id = ?", id).toArray()[0];
    if (prev && ts <= prev.ts) return text("replayed", 409);
    if (data && data.bye === true) {
      // 设备卸载 / 改名时用自己的 key 签一条 bye：立刻从公开页面撤掉，不用等 7 天
      this.sql.exec("DELETE FROM devices WHERE id = ?", id);
      return text("bye");
    }
    const d = cleanDevice(data, new Set(this.relays().map((r) => r.name)));
    this.sql.exec(
      "INSERT INTO devices (id, first, ts, data) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET ts = excluded.ts, data = excluded.data",
      id, ts, ts, JSON.stringify(d));
    return text("ok");
  }

  ingest(node, ts, data) {
    if (DEVICE_ID.test(node)) return this.ingestDevice(node, ts, data);
    const prev = this.sql.exec("SELECT ts FROM latest WHERE node = ?", node).toArray()[0];
    if (prev && ts <= prev.ts) return text("replayed", 409);
    const d = clean(data, node, new Set(this.relays().map((r) => r.name)));
    this.sql.exec(
      "INSERT INTO latest (node, ts, data) VALUES (?, ?, ?) ON CONFLICT(node) DO UPDATE SET ts = excluded.ts, data = excluded.data",
      node, ts, JSON.stringify(d));
    const b = Math.floor(ts / BUCKET);
    this.sql.exec(
      "INSERT INTO beats VALUES (?, ?, 1, ?, ?, ?) ON CONFLICT(node, bucket) DO UPDATE SET n = n + 1, cpu = cpu + excluded.cpu, rx = rx + excluded.rx, tx = tx + excluded.tx",
      node, b, d.host.cpu ?? 0, d.host.net.rx ?? 0, d.host.net.tx ?? 0);
    for (const l of d.et.links) {
      if (!l.direct || l.lat === null) continue;
      this.sql.exec(
        "INSERT INTO links VALUES (?, ?, ?, 1, ?) ON CONFLICT(a, b, bucket) DO UPDATE SET n = n + 1, lat = lat + excluded.lat",
        node, l.peer, b, l.lat);
    }
    if (Math.random() < 0.02) {
      this.sql.exec("DELETE FROM beats WHERE bucket < ?", b - 31 * 288);
      this.sql.exec("DELETE FROM links WHERE bucket < ?", b - 2 * 288);
    }
    return text("ok");
  }

  snapshot() {
    const now = Math.floor(Date.now() / 1000);
    const nb = Math.floor(now / BUCKET);
    const today = Math.floor((now + TZ) / 86400);
    const latest = new Map(this.sql.exec("SELECT node, ts, data FROM latest").toArray().map((r) => [r.node, r]));
    const first = new Map(this.sql.exec("SELECT node, MIN(bucket) AS b FROM beats GROUP BY node").toArray().map((r) => [r.node, r.b]));
    const perDay = new Map();
    for (const r of this.sql.exec(
      `SELECT node, (bucket * ${BUCKET} + ${TZ}) / 86400 AS day, COUNT(*) AS c FROM beats WHERE bucket >= ? GROUP BY node, day`,
      nb - 31 * 288).toArray()) perDay.set(`${r.node}|${r.day}`, r.c);
    const series = new Map();
    for (const r of this.sql.exec(
      "SELECT node, bucket / 6 AS slot, SUM(cpu) / SUM(n) AS cpu, SUM(rx) / SUM(n) AS rx, SUM(tx) / SUM(n) AS tx FROM beats WHERE bucket > ? GROUP BY node, slot ORDER BY slot",
      nb - 288).toArray()) {
      if (!series.has(r.node)) series.set(r.node, []);
      series.get(r.node).push({ t: r.slot * 6 * BUCKET, cpu: r.cpu, rx: r.rx, tx: r.tx });
    }

    const regionOf = new Map(this.relays().map((r) => [r.name, r.region]));
    let peers = 0;
    const relays = this.relays().map((r) => {
      const l = latest.get(r.name);
      const data = l ? JSON.parse(l.data) : null;
      const online = !!l && now - l.ts <= ONLINE_WITHIN;
      if (online) peers = Math.max(peers, data.et.devices ?? 0);
      if (data) data.et.links = data.et.links.filter((x) => regionOf.has(x.peer)).map((x) => ({ ...x, peer: regionOf.get(x.peer) }));
      // 每天的可用率 = 有上报的 5 分钟格数 / 应有格数；开始上报之前的日子记 null（没数据，不算宕机）
      const fb = first.get(r.name);
      let seen = 0, expected = 0;
      const days = [];
      for (let day = today - 29; day <= today; day++) {
        const start = Math.max(Math.ceil((day * 86400 - TZ) / BUCKET), fb ?? Infinity);
        const end = Math.min(Math.floor(((day + 1) * 86400 - TZ) / BUCKET), nb + 1);
        const want = end - start;
        if (fb === undefined || want <= 0) {
          days.push({ day: day * 86400 - TZ, up: null });
          continue;
        }
        const c = Math.min(perDay.get(`${r.name}|${day}`) || 0, want);
        seen += c;
        expected += want;
        days.push({ day: day * 86400 - TZ, up: c / want });
      }
      return {
        id: r.region, region: r.region, spec: r.spec ?? null, online, last_seen: l ? l.ts : null,
        host: data?.host ?? null, et: data?.et ?? null,
        uptime30: expected ? seen / expected : null, days, series: series.get(r.name) || [],
      };
    });
    // 在线 relay 看到的设备：匿名 id -> 它直连的接入点（区域）和延迟
    const seen = new Map();
    for (const r of relays) {
      if (!r.et) continue;
      for (const x of r.online ? r.et.peers ?? [] : []) {
        const links = seen.get(x.id) ?? [];
        if (x.direct && x.lat !== null) links.push({ region: r.region, lat: Math.round(x.lat) });
        seen.set(x.id, links);
      }
      delete r.et.peers; // 并进 devices 了，不在 relay 里重复
    }
    const blank = { kind: null, model: null, chip: null, cores: null, mem: null, os: null, arch: null, disks: [], location: null };
    const probed = new Map(this.sql.exec("SELECT id, first, ts, data FROM devices WHERE ts > ?", now - DEVICE_KEEP).toArray()
      .map((x) => [x.id, x]));
    const devices = [...new Set([...probed.keys(), ...seen.keys()])].map((id) => {
      const row = probed.get(id);
      const { near, ...hw } = row ? JSON.parse(row.data) : { near: null, ...blank };
      const links = (seen.get(id) ?? []).sort((a, b) => a.lat - b.lat);
      const heartbeat = !!row && now - row.ts <= DEVICE_ONLINE_WITHIN;
      return {
        id, ...hw,
        // 位置：在网里就按实测最近的接入点，否则用设备自己配的
        near_region: links[0]?.region ?? (near ? regionOf.get(near) ?? null : null),
        links,
        // online = relay 在网里看得到它；standby = 探针还在报，但没连进网
        status: seen.has(id) ? "online" : heartbeat ? "standby" : "offline",
        online: seen.has(id),
        hardware: !!row,
        last_seen: seen.has(id) ? now : row?.ts ?? null,
      };
    }).sort((a, b) => Number(b.online) - Number(a.online) || Number(b.hardware) - Number(a.hardware) || a.id.localeCompare(b.id));
    // peers：在网设备数（relay 报了匿名 id 的按 id 去重；还没升级的 relay 只有数量，取最大）
    return { now, interval: 30, relays, peers: Math.max(seen.size, peers), devices };
  }

  /** a、b 是区域代码（公开 id）；历史表里存的是名字，这里换回去查。 */
  link(a, b) {
    const nameOf = new Map(this.relays().map((r) => [r.region, r.name]));
    if (!nameOf.has(a) || !nameOf.has(b)) return { a, b, ab: [], ba: [] };
    const nb = Math.floor(Date.now() / 1000 / BUCKET);
    const q = (x, y) => this.sql.exec(
      "SELECT bucket * ? AS t, lat / n AS lat FROM links WHERE a = ? AND b = ? AND bucket > ? ORDER BY bucket", BUCKET, x, y, nb - 288).toArray();
    return { a, b, ab: q(nameOf.get(a), nameOf.get(b)), ba: q(nameOf.get(b), nameOf.get(a)) };
  }
}

// ---------- <name>.<EDGE_ZONE> ----------

function probePage(url, env) {
  const suffix = "." + env.EDGE_ZONE;
  const label = url.hostname.endsWith(suffix) ? url.hostname.slice(0, -suffix.length) : "";
  if (!/^[a-z0-9-]+$/.test(label)) return text("not found", 404);
  const local = `https://${label}-local${suffix}`;
  const target = local + url.pathname + url.search;
  // 猫眼（web/DESIGN.md）的最小版：纯黑 / 纸白、星号标志、等宽大写标注
  const html = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${label} · ${esc(title(env))}</title><link rel="icon" href="${baseUrl(env)}/favicon.svg" type="image/svg+xml">
<style>
:root{color-scheme:dark;--bg:#000;--fg:#f2f2f3;--fg3:rgb(255 255 255/.42);--line:rgb(255 255 255/.12);--eye:#ceff1c;--eyefg:#ceff1c;--arm:rgb(255 255 255/.36)}
@media (prefers-color-scheme:light){:root{color-scheme:light;--bg:#f5f5f2;--fg:#111113;--fg3:rgb(0 0 0/.55);--line:rgb(0 0 0/.12);--eye:#c6f432;--eyefg:#4f6b00;--arm:rgb(0 0 0/.36)}}
*{box-sizing:border-box;margin:0}body{min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:14px/1.6 ui-sans-serif,system-ui,-apple-system,"PingFang SC",sans-serif}
main{display:flex;flex-direction:column;align-items:center;gap:14px;padding:24px;text-align:center}
.arm{fill:var(--arm)}.eye{fill:var(--eye);animation:b 1.6s ease-in-out infinite}.ok .eye{animation:none}.off .eye{fill:var(--fg3);animation:none}
@keyframes b{50%{opacity:.3}}h1{font-size:28px;font-weight:500;letter-spacing:-.02em}
.cap{font:500 11px ui-monospace,"SF Mono",monospace;letter-spacing:.24em;text-transform:uppercase;color:var(--fg3)}
p{color:var(--fg3);max-width:32em}nav{display:flex;gap:10px;margin-top:8px}
a{display:inline-flex;align-items:center;height:34px;padding:0 16px;border-radius:999px;border:1px solid var(--line);color:var(--fg);text-decoration:none}
a.primary{background:var(--fg);color:var(--bg);border-color:var(--fg)}
@media (prefers-reduced-motion:reduce){.eye{animation:none}}
</style>
<main id="m"><svg width="56" height="56" viewBox="0 0 32 32" aria-hidden="true"><g class="arm"><rect x="14" y="3" width="4" height="26" rx="1.4"/><rect x="14" y="3" width="4" height="26" rx="1.4" transform="rotate(60 16 16)"/><rect x="14" y="3" width="4" height="26" rx="1.4" transform="rotate(120 16 16)"/></g><circle class="eye" cx="16" cy="16" r="3.4"/></svg>
<div class="cap">${esc(title(env))}</div><h1>${label}</h1><p id="s">正在检测内网连接…</p>
<nav><a class="primary" href="${target}">直接打开内网地址</a><a href="${baseUrl(env)}/">网络状态</a></nav></main>
<script>
const target = ${JSON.stringify(target)};
const ping = ${JSON.stringify(local + "/.well-known/mesh-ping")};
const s = document.getElementById("s"), m = document.getElementById("m");
fetch(ping, { mode: "no-cors", cache: "no-store", targetAddressSpace: "local", signal: AbortSignal.timeout(3000) })
  .then(() => { m.className = "ok"; s.textContent = "内网可达，正在跳转…"; location.replace(target); })
  .catch(() => { m.className = "off"; s.textContent = ${JSON.stringify(`当前设备不在 ${title(env)} 内网，或浏览器没允许访问本地网络。`).replaceAll("<", "\\u003c")}; });
</script></html>`;
  return new Response(html, { headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" } });
}

const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function json(obj, status = 200) {
  return new Response(JSON.stringify(obj), { status, headers: { "content-type": "application/json; charset=utf-8" } });
}

function text(body, status = 200) {
  return new Response(body, { status, headers: { "content-type": "text/plain; charset=utf-8" } });
}
