// 公开探针不能带内部信息：relay 和设备的名字、overlay 地址、伪装 SNI、控制台域名、密钥字段名。
// 名单从 nodes.yaml 读（本机还会读 $LOLINET_HOME/state/mesh-devices.json 里配对过的设备），加了设备会自动纳入检查。
//   node scripts/check-probe-bundle.mjs                 检查 dist-probe/（npm run build 之后，CI 的 web.yml 会跑）
//   node scripts/check-probe-bundle.mjs --api <URL>     检查线上 /v1/probe 的响应（部署后的冒烟测试）
import { existsSync } from "node:fs";
import { readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

// 节点清单：LOLINET_NODES，否则仓库根的 nodes.yaml，没有就用 nodes.example.yaml
const repo = fileURLToPath(new URL("../../", import.meta.url));
const nodesFile = process.env.LOLINET_NODES ?? [`${repo}nodes.yaml`, `${repo}nodes.example.yaml`].find((p) => existsSync(p));
const doc = parse(readFileSync(nodesFile, "utf8"), { uniqueKeys: false });
const host = (u) => {
  try {
    return new URL(u).hostname;
  } catch {
    return null;
  }
};
const home = process.env.LOLINET_HOME ?? process.env.FLEET_REPO ?? `${homedir()}/.config/lolinet`;
const registry = `${home}/state/mesh-devices.json`;
const paired = existsSync(registry) ? JSON.parse(readFileSync(registry, "utf8")) : [];
const names = [...doc.relays.map((r) => r.name), ...(doc.devices ?? []).map((d) => d.name), ...paired.map((d) => d.name)];
const bad = [
  doc.network.sni,
  host(doc.network.console),
  host(doc.network.manager),
  ...(doc.devices ?? []).map((d) => d.overlay_ip),
  ...paired.map((d) => d.overlay_ip),
  ...doc.relays.map((r) => r.overlay_ip),
  "credential_secret",
  "network_secret",
  "local_private_key",
].filter(Boolean);

// 名字按整词匹配（不在字母数字下划线中间），免得误伤压缩后的代码
const word = (s) => new RegExp(`(?<![A-Za-z0-9_])${s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![A-Za-z0-9_])`);
const scan = (text) => [...new Set([...bad.filter((s) => text.includes(s)), ...names.filter((n) => word(n).test(text))])];

const api = process.argv.indexOf("--api");
if (api > 0) {
  const r = await fetch(process.argv[api + 1], { cache: "no-store" });
  const text = await r.text();
  const hits = scan(text);
  if (!r.ok || hits.length) {
    console.error(`公开接口 ${r.status}，带了内部信息：${hits.join(", ") || "（无）"}`);
    process.exit(1);
  }
  console.log(`${process.argv[api + 1]} clean: ${bad.length + names.length} strings`);
  process.exit(0);
}

const root = new URL("../dist-probe/", import.meta.url).pathname;
const files = readdirSync(root, { recursive: true, withFileTypes: true })
  .filter((e) => e.isFile() && !e.name.endsWith(".woff2"))
  .map((e) => join(e.parentPath, e.name));
const hits = files.flatMap((f) => {
  const text = readFileSync(f, "latin1");
  return scan(text).map((s) => `${f.slice(root.length)}: ${s}`);
});
if (hits.length) {
  console.error("公开探针包里有内部信息：\n  " + hits.join("\n  "));
  process.exit(1);
}
console.log(`dist-probe clean: ${bad.length + names.length} strings × ${files.length} files`);
