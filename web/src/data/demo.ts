// 演示后端：agent 连不上或地址栏带 ?demo 时用。拓扑是从真实内核抓的快照（agent --dump，已去密钥；公开仓库里的这份把设备名、公网地址和 SNI 换成了示例），
// 凭据和设备是编的。配对会在约 9 秒后让新设备「上线」，好把整条流程走完。
import nodesDoc from "virtual:mesh-nodes";
import type { Backend, PairRequest } from "./api";
import fixture from "./demo-snapshot.json";
import type { AgentInfo, CenterNode, Credential, CliPeer, CliRoute, MihomoRules, RegisteredDevice, Settings, SettingsView, Snapshot } from "./types";

const base = fixture as unknown as Snapshot;
const now = () => Math.floor(Date.now() / 1000);
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
const CODE = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

type Joined = { peer: CliPeer; route: CliRoute; center: CenterNode; at: number };

const state = {
  credentials: [
    { credential_id: "mbp-studio-20261102093011", expiry_unix: now() + 86400 * 358, reusable: false, online_peers: [] },
    { credential_id: "pad-20261105211804", expiry_unix: now() + 86400 * 5, reusable: false, online_peers: [] },
  ] as Credential[],
  devices: [
    { name: "mbp-studio", overlay_ip: "10.188.16.11", credential_id: "mbp-studio-20261102093011", expiry_unix: now() + 86400 * 358, paired_at: "2026-11-02T09:30:11" },
    { name: "pad", overlay_ip: "10.188.32.1", credential_id: "pad-20261105211804", expiry_unix: now() + 86400 * 5, paired_at: "2026-11-05T21:18:04" },
  ] as RegisteredDevice[],
  joined: [] as Joined[],
  settings: {
    pair: { ttl_days: 365, code_minutes: 15, peers: nodesDoc.network.device_peers ?? nodesDoc.relays.slice(0, 3).map((r) => r.name), transport: "wss", probe: true },
    device: { latency_first: true, enable_kcp_proxy: true, udp_peers: false },
  } as Settings,
  mihomo: {
    reject: ["DOMAIN-SUFFIX,ads.example.com"],
    direct: ["GEOIP,CN"],
    exit: ["GEOSITE,openai"],
    exit_order: ["mbp-studio"],
    routes: [{ name: "办公室", exit: "mbp-studio", rules: ["DOMAIN-SUFFIX,web.com"] }],
  } as MihomoRules,
  exits: [{ name: "mbp-studio", ip: "10.188.16.11", port: 10180, since: now() - 3600 * 5 }],
};

/** 演示用的配置预览：结构和 manager 渲染的一样，relay 地址用占位 */
function demoPreview(s: Settings) {
  const n = nodesDoc.network;
  const lines = ['hostname = "<设备名>"', `instance_name = "${n.name}"`, 'ipv4 = "10.188.16.x/16"', "dhcp = false", "listeners = []", "",
    "[network_identity]", `network_name = "${n.name}"`, "", "[secure_mode]", "enabled = true", 'local_private_key = "<控制台签发的凭据私钥>"'];
  for (const p of s.pair.peers) {
    const uri = s.pair.transport === "tcp" ? `tcp://<${p} EIP>:<端口>` : `wss://<${p} EIP>:443/?sni=<伪装 SNI>`;
    lines.push("", "[[peer]]", `# ${p}.${n.edge_zone}`, `uri = "${uri}"`);
    if (s.pair.transport === "wss" && s.device.udp_peers) lines.push("", "[[peer]]", `# ${p}.${n.edge_zone} udp`, `uri = "udp://<${p} EIP>:<端口>"`);
  }
  lines.push("", "[flags]", "private_mode = true");
  if (s.device.latency_first) lines.push("latency_first = true");
  if (s.device.enable_kcp_proxy) lines.push("enable_kcp_proxy = true");
  return lines.join("\n") + "\n";
}

function settingsView(): SettingsView {
  return {
    settings: structuredClone(state.settings),
    defaults: { pair: { ttl_days: 365, code_minutes: 15, peers: state.settings.pair.peers, transport: "wss", probe: true }, device: { latency_first: true, enable_kcp_proxy: true, udp_peers: false } },
    relays: nodesDoc.relays.map((r) => ({ name: r.name, region: r.region, overlay_ip: r.overlay_ip })),
    network: { name: nodesDoc.network.name, cidr: nodesDoc.network.cidr, edge_zone: nodesDoc.network.edge_zone, issuer: nodesDoc.network.issuer, console: nodesDoc.network.console },
    base_url: `https://${nodesDoc.network.join}`,
    preview: demoPreview(state.settings),
  };
}

const jitter = (v: number) => Math.max(0.5, v * (0.96 + Math.random() * 0.08));

function snapshot(): Snapshot {
  const s: Snapshot = structuredClone(base);
  s.fetched_at = now();
  for (const p of s.peers) if (p.lat_ms !== "-") p.lat_ms = jitter(parseFloat(p.lat_ms)).toFixed(2);
  for (const n of s.graph) for (const d of n.direct_peers) d.latency_ms = Math.round(jitter(d.latency_ms));
  for (const j of state.joined) {
    if (now() < j.at) continue;
    s.peers.push(j.peer);
    s.routes.push(j.route);
    s.graph.push(j.center);
    const hub = s.graph.find((g) => g.hostname === "hoshiguma");
    hub?.direct_peers.push({ node_id: j.center.node_id, hostname: j.peer.hostname, ipv4: j.route.ipv4, latency_ms: 24 });
  }
  return s;
}

function join(req: PairRequest) {
  const id = String(1e9 + Math.floor(Math.random() * 3e9));
  const lat = 27 + Math.random() * 6;
  state.joined.push({
    at: now() + 9,
    peer: { cidr: `${req.ip}/16`, ipv4: req.ip, hostname: req.name, cost: "relay(2)", lat_ms: "-", loss_rate: "-", rx_bytes: "-", tx_bytes: "-", tunnel_proto: "", nat_type: "Symmetric", id, version: "2.7.0-6aed7bd1~" },
    route: {
      ipv4: `${req.ip}/16`, hostname: req.name, proxy_cidrs: "", next_hop_ipv4: "10.188.0.12/16", next_hop_hostname: "hoshiguma",
      next_hop_lat: 36, path_len: 2, path_latency: Math.round(36 + lat), next_hop_ipv4_lat_first: "10.188.0.12/16",
      next_hop_hostname_lat_first: "hoshiguma", path_len_lat_first: 2, path_latency_lat_first: Math.round(36 + lat), version: "2.7.0-6aed7bd1~",
    },
    center: { node_id: id, hostname: req.name, ipv4: `${req.ip}/16`, direct_peers: [{ node_id: "1527596297", hostname: "hoshiguma", ipv4: "10.188.0.12/16", latency_ms: Math.round(lat) }] },
  });
}

const info: AgentInfo = {
  agent: "demo",
  rpc: "demo",
  socks: null,
  console: { url: nodesDoc.network.console, auth: "agent" },
  network_id: "00000000-0000-4000-8000-000000000000",
  pair: true,
  registry: true,
  join: nodesDoc.network.join,
  manager: true,
  login_required: false,
  logged_in: true,
  user: "demo",
};

export const demoBackend: Backend = {
  demo: true,
  info: async () => info,
  snapshot: async () => snapshot(),
  devices: async () => structuredClone(state.devices),
  pair: async (req) => {
    await wait(1100);
    const raw = Array.from({ length: 12 }, () => CODE[(Math.random() * CODE.length) | 0]).join("");
    const code = `${raw.slice(0, 4)}-${raw.slice(4, 8)}-${raw.slice(8)}`;
    const cid = `${req.name}-${new Date().toISOString().replace(/\D/g, "").slice(0, 14)}`;
    const expiry = now() + req.ttl_days * 86400;
    state.credentials.unshift({ credential_id: cid, expiry_unix: expiry, reusable: false, online_peers: [] });
    state.devices.unshift({ name: req.name, overlay_ip: req.ip, credential_id: cid, expiry_unix: expiry, paired_at: new Date().toISOString().slice(0, 19) });
    join(req);
    const minutes = req.code_minutes ?? state.settings.pair.code_minutes;
    return { name: req.name, ip: req.ip, code, credential_id: cid, command: `curl -fsSL https://${nodesDoc.network.join}/i | sudo sh -s ${code}`, expires_at: now() + minutes * 60 };
  },
  forget: async (cid) => {
    state.devices = state.devices.filter((d) => d.credential_id !== cid);
  },
  consoleLoggedIn: async () => true,
  login: async () => {},
  networks: async () => [
    { network_id: info.network_id!, display_name: "loli", network_name: "loli", networking_method: "Gateway", virtual_cidr: "10.188.0.0/16", secure_mode: true, member_count: 0, online_member_count: 0 },
  ],
  credentials: async () => {
    const s = snapshot();
    return state.credentials.map((c) => {
      const dev = state.devices.find((d) => d.credential_id === c.credential_id);
      const peer = dev && s.peers.find((p) => p.hostname === dev.name);
      return { ...c, online_peers: peer ? [{ peer_id: Number(peer.id), credential_id: c.credential_id, hostname: peer.hostname, ipv4: peer.ipv4, version: peer.version }] : [] };
    });
  },
  revoke: async (_nid, cid) => {
    await wait(400);
    state.credentials = state.credentials.filter((c) => c.credential_id !== cid);
  },
  logout: async () => {},
  settings: async () => settingsView(),
  saveSettings: async (s) => {
    await wait(300);
    state.settings = structuredClone(s);
    return settingsView();
  },
  previewSettings: async (s) => demoPreview(s),
  mihomo: async () => ({ mihomo: structuredClone(state.mihomo), exits: structuredClone(state.exits) }),
  saveMihomo: async (m) => {
    await wait(300);
    state.mihomo = structuredClone(m);
    return { mihomo: structuredClone(state.mihomo), exits: structuredClone(state.exits) };
  },
};
