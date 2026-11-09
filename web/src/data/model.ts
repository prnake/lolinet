import nodesDoc from "virtual:mesh-nodes";
import { swarmIndex } from "../ui/Mark";
import { regionOf, type Region } from "./regions";
import type { RelaySpec, Snapshot } from "./types";

export { nodesDoc };

export type Kind = "relay" | "device" | "gateway";

export type MeshNode = {
  id: string;
  hostname: string;
  ipv4: string | null;
  kind: Kind;
  self: boolean;
  relay?: RelaySpec;
  region?: Region;
  color: number;
  version?: string;
  nat?: string;
  /** 本机看它：p2p / relay(n) / Local */
  cost: string;
  /** 本机和它直连时的实测值 */
  direct: { lat: number | null; loss: number | null; proto: string; rx: string; tx: string } | null;
  route: { hop: string; len: number; latency: number; lfHop: string; lfLen: number; lfLatency: number } | null;
  /** 本机到它的实际延迟：开了 latency_first 就按延迟优先的路径算 */
  latency: number | null;
  /** 实际走的下一跳（DIRECT 表示直连） */
  via: string | null;
};

export type Edge = { a: string; b: string; latency: number; ab?: number; ba?: number };

export type Mesh = {
  self: MeshNode | null;
  nodes: MeshNode[];
  byId: Map<string, MeshNode>;
  edges: Edge[];
  relays: MeshNode[];
  devices: MeshNode[];
  gateways: MeshNode[];
  latencyFirst: boolean;
};

const bareIp = (s?: string | null) => (s ? s.split("/")[0] : "") || null;
const num = (s: string | number | undefined | null) => {
  if (s === undefined || s === null) return null;
  const n = typeof s === "number" ? s : parseFloat(s);
  return Number.isFinite(n) ? n : null;
};
const key = (hostname: string, ip: string | null) => `${hostname}|${ip ?? ""}`;

const RELAYS = new Map(nodesDoc.relays.map((r, i) => [r.name, { spec: r, order: i }]));

/** 双向测量差得离谱时（常见是一侧刚起步报 1ms），取大的；否则取平均。 */
function combine(ab?: number, ba?: number) {
  if (ab === undefined) return ba ?? 0;
  if (ba === undefined) return ab;
  const hi = Math.max(ab, ba);
  return Math.abs(ab - ba) > hi / 2 ? hi : (ab + ba) / 2;
}

export function buildMesh(s: Snapshot): Mesh {
  const latencyFirst = !!s.node?.latency_first;
  const nodes = new Map<string, MeshNode>();
  const routeByKey = new Map(s.routes.map((r) => [key(r.hostname, bareIp(r.ipv4)), r]));

  const make = (id: string, rawName: string, ip: string | null): MeshNode => {
    // 刚上线或正在退出的节点，peer-center 里偶尔只有 id 没有主机名
    const hostname = rawName || `#${id.slice(-4)}`;
    const rel = RELAYS.get(hostname);
    const isRelay = !!rel && ip === rel.spec.overlay_ip;
    const kind: Kind = isRelay ? "relay" : ip ? "device" : "gateway";
    return {
      id,
      hostname,
      ipv4: ip,
      kind,
      self: s.node?.peer_id === id,
      relay: isRelay ? rel!.spec : undefined,
      region: isRelay ? regionOf(rel!.spec.region) : undefined,
      // 网关和它所在的 relay 同色：一眼看出它住在哪台机器上
      color: swarmIndex(hostname, rel ? rel.order : undefined),
      cost: "",
      direct: null,
      route: null,
      latency: null,
      via: null,
    };
  };

  for (const p of s.peers) {
    const ip = bareIp(p.ipv4);
    const n = make(p.id, p.hostname, ip);
    n.version = p.version;
    n.nat = p.nat_type;
    n.cost = p.cost;
    if (p.cost === "Local") n.self = true;
    if (p.cost === "p2p")
      n.direct = { lat: num(p.lat_ms), loss: num(p.loss_rate) !== null ? num(p.loss_rate)! / 100 : null, proto: p.tunnel_proto, rx: p.rx_bytes, tx: p.tx_bytes };
    nodes.set(n.id, n);
  }
  for (const g of s.graph) {
    if (!nodes.has(g.node_id)) nodes.set(g.node_id, make(g.node_id, g.hostname, bareIp(g.ipv4)));
  }
  for (const n of nodes.values()) {
    const r = routeByKey.get(key(n.hostname, n.ipv4));
    if (r && !n.self) {
      n.route = {
        hop: r.next_hop_hostname,
        len: r.path_len,
        latency: r.path_latency,
        lfHop: r.next_hop_hostname_lat_first,
        lfLen: r.path_len_lat_first,
        lfLatency: r.path_latency_lat_first,
      };
      n.latency = latencyFirst ? r.path_latency_lat_first : r.path_latency;
      n.via = latencyFirst ? r.next_hop_hostname_lat_first : r.next_hop_hostname;
      n.version ??= r.version;
    }
  }
  if (s.node) {
    const me = nodes.get(s.node.peer_id);
    if (me) {
      me.self = true;
      me.nat = s.node.udp_nat_type ?? me.nat;
    }
  }

  const half = new Map<string, Edge>();
  for (const g of s.graph) {
    for (const d of g.direct_peers) {
      if (!nodes.has(d.node_id) || !nodes.has(g.node_id) || d.node_id === g.node_id) continue;
      const [a, b] = g.node_id < d.node_id ? [g.node_id, d.node_id] : [d.node_id, g.node_id];
      const k = `${a}-${b}`;
      const e = half.get(k) ?? { a, b, latency: 0 };
      if (g.node_id === a) e.ab = d.latency_ms;
      else e.ba = d.latency_ms;
      half.set(k, e);
    }
  }
  const edges = [...half.values()].map((e) => ({ ...e, latency: combine(e.ab, e.ba) }));

  const all = [...nodes.values()];
  const byOrder = (x: MeshNode) => (x.relay ? RELAYS.get(x.hostname)!.order : 99);
  return {
    self: all.find((n) => n.self) ?? null,
    nodes: all,
    byId: nodes,
    edges,
    relays: all.filter((n) => n.kind === "relay").sort((a, b) => byOrder(a) - byOrder(b)),
    devices: all.filter((n) => n.kind === "device").sort((a, b) => Number(b.self) - Number(a.self) || a.hostname.localeCompare(b.hostname)),
    gateways: all.filter((n) => n.kind === "gateway"),
    latencyFirst,
  };
}

export function edgeBetween(m: Mesh, a: string, b: string) {
  return m.edges.find((e) => (e.a === a && e.b === b) || (e.a === b && e.b === a));
}

export function kindLabel(n: MeshNode) {
  if (n.kind === "relay") return "接入点";
  if (n.kind === "gateway") return n.hostname === nodesDoc.network.issuer ? "控制台网关" : "无地址节点";
  return "设备";
}

/** 下一个空闲地址：电脑（TUN）10.188.16.0/20，App（无 TUN）10.188.32.0/20，见 ipam.md。 */
export function nextFreeIp(pool: "tun" | "app", used: Iterable<string>) {
  const taken = new Set(used);
  const base = pool === "tun" ? 16 : 32;
  for (let third = base; third < base + 16; third++) {
    for (let last = third === base ? 10 : 1; last < 255; last++) {
      const ip = `10.188.${third}.${last}`;
      if (!taken.has(ip)) return ip;
    }
  }
  return "";
}
