import { useMemo, useState } from "react";
import { ms } from "../data/format";
import { kindLabel, type Mesh, type MeshNode } from "../data/model";
import { GATEWAY_COLOR, swarmVar } from "../ui";

const W = 1100;
const H = 520;
const ORBIT = 104;
const SPREAD = 0.95; // 同一个 relay 旁边的设备之间隔多少弧度

type P = { x: number; y: number; side: "left" | "right" };

/** relay 按区域落位；网关挂在宿主 relay 下方；设备绕着离它最近的 relay、朝画布外侧排开。 */
function layout(m: Mesh): Map<string, P> {
  const pos = new Map<string, P>();
  for (const r of m.relays) if (r.region) pos.set(r.id, { x: r.region.x, y: r.region.y, side: r.region.side });
  const relayByName = new Map(m.relays.map((r) => [r.hostname, r]));
  for (const g of m.gateways) {
    const host = relayByName.get(g.hostname);
    const p = host && pos.get(host.id);
    pos.set(g.id, p ? { x: p.x + 34, y: p.y + 62, side: "right" } : { x: W / 2, y: H - 40, side: "right" });
  }
  const orbits = new Map<string, MeshNode[]>();
  for (const d of m.devices) {
    let best: { id: string; lat: number } | null = null;
    for (const e of m.edges) {
      const other = e.a === d.id ? e.b : e.b === d.id ? e.a : null;
      if (!other || !pos.has(other) || m.byId.get(other)?.kind !== "relay") continue;
      if (!best || e.latency < best.lat) best = { id: other, lat: e.latency };
    }
    const hub = best?.id ?? (d.via && relayByName.get(d.via)?.id) ?? m.relays[0]?.id ?? "";
    orbits.set(hub, [...(orbits.get(hub) ?? []), d]);
  }
  const inside = (x: number, y: number) => x >= 130 && x <= W - 130 && y >= 40 && y <= H - 40;
  for (const [hub, list] of orbits) {
    const c = pos.get(hub) ?? { x: W / 2, y: H / 2, side: "right" as const };
    const out = Math.atan2(c.y - H / 2, c.x - W / 2);
    list.forEach((d, i) => {
      // 朝画布外侧排开；出界就沿水平、再沿竖直方向镜像，最后才夹到边上
      let a = out + (i - (list.length - 1) / 2) * SPREAD;
      const at = (t: number) => ({ x: c.x + Math.cos(t) * ORBIT, y: c.y + Math.sin(t) * ORBIT });
      if (!inside(at(a).x, at(a).y)) a = -a;
      if (!inside(at(a).x, at(a).y)) a = Math.PI - a;
      const p = at(a);
      pos.set(d.id, {
        x: Math.min(W - 130, Math.max(130, p.x)),
        y: Math.min(H - 40, Math.max(40, p.y)),
        side: Math.cos(a) < -0.2 ? "left" : "right",
      });
    });
  }
  return pos;
}

function curve(a: P, b: P, bend = 0.08) {
  const mx = (a.x + b.x) / 2;
  const my = (a.y + b.y) / 2;
  const cx = mx - (b.y - a.y) * bend;
  const cy = my + (b.x - a.x) * bend;
  return { d: `M${a.x},${a.y} Q${cx},${cy} ${b.x},${b.y}`, mid: { x: (mx + cx) / 2, y: (my + cy) / 2 } };
}

const edgeAlpha = (lat: number) => (lat <= 40 ? 0.34 : lat <= 100 ? 0.22 : lat <= 200 ? 0.13 : 0.07);
const pairKey = (a: string, b: string) => (a < b ? `${a}-${b}` : `${b}-${a}`);
// 同一条边无论从哪头画，弯向都一样
const bendFor = (a: string, b: string) => (a < b ? 0.08 : -0.08);

/** 本机到目标实际走的路：本机 → 下一跳（不是直连时）→ 目标。 */
export function pathTo(m: Mesh, target: MeshNode): MeshNode[] {
  if (!m.self || target.self) return [];
  const hop =
    target.via && target.via !== "DIRECT" && target.via !== target.hostname
      ? m.nodes.find((n) => n.hostname === target.via && n.kind !== "gateway")
      : undefined;
  return hop ? [m.self, hop, target] : [m.self, target];
}

const tilt = (n: MeshNode) => ((n.hostname.length * 37 + n.color * 23) % 50) - 25;

export function Topology({ mesh, selected, onSelect, sub: subOf, dim: dimOf }: {
  mesh: Mesh;
  /** 选中的目标：画出本机到它的路径 */
  selected?: string | null;
  onSelect?: (id: string) => void;
  /** 节点名下面那行小字；不给就是「城市 · 本机到它的延迟」 */
  sub?: (n: MeshNode) => string;
  /** 压暗的节点（公开探针里的离线 relay） */
  dim?: (n: MeshNode) => boolean;
}) {
  const [hover, setHover] = useState<string | null>(null);
  const pos = useMemo(() => layout(mesh), [mesh]);
  const focus = hover ?? selected ?? null;
  const focusNode = focus ? mesh.byId.get(focus) : undefined;
  const path = focusNode ? pathTo(mesh, focusNode) : [];
  const onPath = new Set(path.slice(1).map((n, i) => pairKey(path[i].id, n.id)));
  const near = (id: string) => !!hover && mesh.edges.some((e) => (e.a === hover && e.b === id) || (e.b === hover && e.a === id));

  return (
    <div className="topo">
      <svg viewBox={`0 0 ${W} ${H}`} className="topo__svg" role="img" aria-label="网络拓扑">
        <defs>
          <pattern id="topo-dots" width="22" height="22" patternUnits="userSpaceOnUse">
            <circle cx="1" cy="1" r="0.9" fill="rgb(var(--tint) / 0.07)" />
          </pattern>
          <filter id="topo-glow" x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="3" result="b" />
            <feMerge>
              <feMergeNode in="b" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>
        <rect width={W} height={H} fill="url(#topo-dots)" />
        <text x={770} y={H - 28} className="topo__sea">PACIFIC · 太平洋</text>

        {mesh.edges.map((e) => {
          const a = pos.get(e.a);
          const b = pos.get(e.b);
          if (!a || !b) return null;
          const k = pairKey(e.a, e.b);
          const lit = !!hover && (e.a === hover || e.b === hover);
          const { d, mid } = curve(a, b, bendFor(e.a, e.b));
          const sameHost = mesh.byId.get(e.a)?.hostname === mesh.byId.get(e.b)?.hostname;
          return (
            <g key={k}>
              <path
                d={d}
                fill="none"
                stroke="var(--fg)"
                strokeOpacity={lit ? 0.55 : hover ? edgeAlpha(e.latency) * 0.4 : edgeAlpha(e.latency)}
                strokeWidth={lit ? 1.25 : 1}
                strokeDasharray={sameHost ? "2 4" : undefined}
                className="topo__edge"
              />
              {lit && !onPath.has(k) && !sameHost && (
                <g transform={`translate(${mid.x},${mid.y})`} className="topo__lat">
                  <rect x={-20} y={-9} width={40} height={18} rx={9} />
                  <text textAnchor="middle" dy="0.35em">{Math.round(e.latency)}</text>
                </g>
              )}
            </g>
          );
        })}

        {path.slice(1).map((n, i) => {
          const from = path[i];
          const a = pos.get(from.id);
          const b = pos.get(n.id);
          if (!a || !b) return null;
          const { d, mid } = curve(a, b, bendFor(from.id, n.id));
          const e = mesh.edges.find((x) => pairKey(x.a, x.b) === pairKey(from.id, n.id));
          return (
            <g key={`p${i}`}>
              <path d={d} className="topo__path" filter="url(#topo-glow)" />
              {e && (
                <g transform={`translate(${mid.x},${mid.y})`} className="topo__lat topo__lat--eye">
                  <rect x={-24} y={-9} width={48} height={18} rx={9} />
                  <text textAnchor="middle" dy="0.35em">{Math.round(e.latency)} ms</text>
                </g>
              )}
            </g>
          );
        })}

        {mesh.nodes.map((n, i) => {
          const p = pos.get(n.id);
          if (!p) return null;
          const on = focus === n.id;
          const dim = (!!hover && hover !== n.id && !path.some((x) => x.id === n.id) && !near(n.id)) || !!dimOf?.(n);
          const color = n.kind === "gateway" ? GATEWAY_COLOR : swarmVar(n.color);
          const sub = subOf
            ? subOf(n)
            : n.self
            ? "本机"
            : n.kind === "relay"
              ? `${n.region?.city ?? ""} · ${ms(n.latency)}`
              : n.kind === "gateway"
                ? kindLabel(n)
                : ms(n.latency);
          const left = p.side === "left";
          return (
            <g
              key={n.id}
              transform={`translate(${p.x},${p.y})`}
              className={`topo__node ${on ? "is-on" : ""} ${dim ? "is-dim" : ""} ${n.kind === "gateway" ? "is-small" : ""}`}
              onMouseEnter={() => setHover(n.id)}
              onMouseLeave={() => setHover(null)}
              onClick={() => onSelect?.(n.id)}
              tabIndex={0}
              onKeyDown={(e) => e.key === "Enter" && onSelect?.(n.id)}
              role="button"
              aria-label={`${n.hostname} ${sub}`}
            >
              <circle r={26} fill="transparent" />
              {n.self && (
                <>
                  <circle r={17} className="topo__halo" />
                  <circle r={17} className="topo__halo-ring" />
                </>
              )}
              {on && !n.self && <circle r={17} className="topo__ring" />}
              <g className="drift" style={{ animationDelay: `${-(i * 1.37) % 6}s` }}>
                <path
                  d="M-7 -7.4l14.6 6.1-6.7 1.9-2 6.8z"
                  fill={color}
                  stroke="var(--cursor-edge)"
                  strokeWidth={1.6}
                  strokeLinejoin="round"
                  transform={`rotate(${tilt(n)}) scale(${n.kind === "gateway" ? 0.8 : 1.1})`}
                />
              </g>
              <text x={left ? -16 : 16} y={-1} textAnchor={left ? "end" : "start"} className="topo__name">
                {n.hostname}
              </text>
              <text x={left ? -16 : 16} y={14} textAnchor={left ? "end" : "start"} className={`topo__sub ${n.self ? "is-eye" : ""}`}>
                {sub}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}
