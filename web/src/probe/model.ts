import type { Edge, Mesh, MeshNode } from "../data/model";
import { CITIES, regionOf } from "../data/regions";
import type { CenterNode } from "../data/types";
import { swarmIndex, swarmVar } from "../ui/Mark";
import type { GlobeArc, GlobeNode } from "../viz/GlobeTopology";
import type { ProbeDevice, ProbeSnapshot } from "./types";

/** relay 在页面上的名字：城市（没有名字可公开，区域代码兜底）。 */
export const relayLabel = (region: string) => regionOf(region)?.city ?? region;

/** 把探针快照拼成控制台的 Mesh，好直接复用拓扑图和延迟矩阵。节点 id 是区域代码，显示名是城市。 */
export function buildProbeMesh(s: ProbeSnapshot): { mesh: Mesh; graph: CenterNode[] } {
  const nodes: MeshNode[] = s.relays.map((r, i) => ({
    id: r.id,
    hostname: relayLabel(r.region),
    ipv4: null,
    kind: "relay",
    self: false,
    region: regionOf(r.region),
    color: swarmIndex(r.id, i),
    version: r.et?.version ?? undefined,
    cost: "",
    direct: null,
    route: null,
    latency: null,
    via: null,
  }));
  const byId = new Map(nodes.map((n) => [n.id, n]));

  const graph: CenterNode[] = s.relays
    .filter((r) => r.online && r.et)
    .map((r) => ({
      node_id: r.id,
      hostname: relayLabel(r.region),
      ipv4: "",
      direct_peers: r.et!.links
        .filter((l) => l.direct && l.lat !== null && byId.has(l.peer))
        .map((l) => ({ node_id: l.peer, hostname: relayLabel(l.peer), ipv4: "", latency_ms: Math.round(l.lat!) })),
    }));

  const half = new Map<string, Edge>();
  for (const g of graph) {
    for (const d of g.direct_peers) {
      const [a, b] = g.node_id < d.node_id ? [g.node_id, d.node_id] : [d.node_id, g.node_id];
      const e = half.get(`${a}-${b}`) ?? { a, b, latency: 0 };
      if (g.node_id === a) e.ab = d.latency_ms;
      else e.ba = d.latency_ms;
      half.set(`${a}-${b}`, e);
    }
  }
  const edges = [...half.values()].map((e) => ({ ...e, latency: e.ab !== undefined && e.ba !== undefined ? (e.ab + e.ba) / 2 : (e.ab ?? e.ba ?? 0) }));

  return {
    mesh: { self: null, nodes, byId, edges, relays: nodes, devices: [], gateways: [], latencyFirst: false },
    graph,
  };
}

/** 接入点之间直连延迟的中位数（双向都算）。 */
export function medianLatency(s: ProbeSnapshot) {
  const v = s.relays
    .filter((r) => r.online)
    .flatMap((r) => r.et?.links ?? [])
    .filter((l) => l.direct && l.lat !== null)
    .map((l) => l.lat!)
    .sort((a, b) => a - b);
  return v.length ? v[Math.floor(v.length / 2)] : null;
}

const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function bytes(n: number | null | undefined, digits = 1) {
  if (n === null || n === undefined || !Number.isFinite(n)) return "—";
  let i = 0;
  while (n >= 1024 && i < UNITS.length - 1) {
    n /= 1024;
    i++;
  }
  return `${i === 0 ? Math.round(n) : n.toFixed(n >= 100 ? 0 : digits)} ${UNITS[i]}`;
}

/** 设备的公开位置：配置写死的城市，否则「离它最近的接入点所在城市」附近。 */
export function deviceLocation(d: { location: string | null; near_region: string | null }) {
  if (d.location) return d.location;
  if (d.near_region) return `${relayLabel(d.near_region)}附近`;
  return "位置未公开";
}

function hash(s: string) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

/**
 * 设备在地球上的位置：手写的城市，否则最近的接入点。偏到接入点标签的另一侧（标签在右就往西边放），
 * 免得盖住城市名；同一个点旁边的几台设备在这个扇区里均匀排开（slot / of），设备自己的文字也朝外写。
 */
export function devicePosition(d: ProbeDevice, slot = 0, of = 1): { lat: number; lng: number; side: "left" | "right" } | null {
  const relay = d.location && CITIES[d.location] ? undefined : regionOf(d.near_region ?? undefined);
  const base = (d.location && CITIES[d.location]) || relay;
  if (!base) return null;
  const h = hash(d.id);
  const center = relay ? (relay.side === "left" ? 0 : 180) : h % 360;
  const spread = of > 1 ? (slot - (of - 1) / 2) * Math.min(40, 120 / (of - 1)) : 0;
  const angle = ((center + spread) * Math.PI) / 180;
  const r = 3 + (of > 2 ? (slot % 2) * 1.2 : 0);
  const lat = base.lat + r * Math.sin(angle);
  const lng = base.lng + (r * Math.cos(angle)) / Math.max(0.3, Math.cos((lat * Math.PI) / 180));
  return { lat, lng, side: Math.cos(angle) < 0 ? "left" : "right" };
}

export const deviceColor = (d: ProbeDevice) => swarmVar(swarmIndex(d.id));
export const deviceTitle = (d: ProbeDevice) => d.model ?? d.kind ?? "设备";

export function deviceStatus(d: ProbeDevice) {
  if (d.status === "online") return d.links[0] ? `在线 · ${d.links[0].lat} ms` : "在线";
  return d.status === "standby" ? "未入网" : "离线";
}

/** 地球上的节点和弧线：接入点两两直连（实线），设备连它直连的接入点（虚线）。 */
export function buildGlobe(s: ProbeSnapshot, mesh: Mesh): { nodes: GlobeNode[]; arcs: GlobeArc[] } {
  const nodes: GlobeNode[] = [];
  for (const r of s.relays) {
    const g = regionOf(r.region);
    if (!g) continue;
    nodes.push({
      id: r.id, kind: "relay", lat: g.lat, lng: g.lng, side: g.side, color: swarmVar(mesh.byId.get(r.id)?.color ?? 0),
      label: g.city, sub: r.online ? `CPU ${Math.round(r.host?.cpu ?? 0)}%` : "离线", dim: !r.online,
    });
  }
  const arcs: GlobeArc[] = mesh.edges.map((e) => ({ a: e.a, b: e.b, kind: "relay", latency: e.latency }));
  // 同一个落点（城市 / 最近的接入点）的设备排在一起编号，好在扇区里排开
  const groups = new Map<string, ProbeDevice[]>();
  for (const d of s.devices) {
    const k = (d.location && CITIES[d.location] ? d.location : d.near_region) ?? "";
    groups.set(k, [...(groups.get(k) ?? []), d]);
  }
  for (const list of groups.values()) {
    list.forEach((d, i) => {
      const at = devicePosition(d, i, list.length);
      if (!at) return;
      nodes.push({ id: d.id, kind: "device", ...at, color: deviceColor(d), label: deviceTitle(d), sub: deviceStatus(d), dim: !d.online });
      d.links.forEach((l, j) => arcs.push({ a: d.id, b: l.region, kind: "device", latency: l.lat, secondary: j > 0 }));
    });
  }
  return { nodes, arcs };
}

export const rate = (n: number | null | undefined) => (n === null || n === undefined ? "—" : `${bytes(n)}/s`);

export function duration(sec: number | null | undefined) {
  if (sec === null || sec === undefined) return "—";
  const d = Math.floor(sec / 86400);
  const h = Math.floor((sec % 86400) / 3600);
  const m = Math.floor((sec % 3600) / 60);
  if (d) return `${d} 天 ${h} 小时`;
  if (h) return `${h} 小时 ${m} 分`;
  return `${Math.max(1, m)} 分钟`;
}

export function percent(v: number | null | undefined, digits = 2) {
  if (v === null || v === undefined) return "—";
  const p = v * 100;
  return p >= 99.995 ? "100%" : `${p.toFixed(digits)}%`;
}

/** 北京时间的「月-日」。 */
export const dayLabel = (unix: number) =>
  new Date((unix + 8 * 3600) * 1000).toISOString().slice(5, 10);

export const hhmm = (unix: number) => new Date((unix + 8 * 3600) * 1000).toISOString().slice(11, 16);
