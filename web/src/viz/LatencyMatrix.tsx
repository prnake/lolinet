import { Fragment, useState } from "react";
import type { Mesh, MeshNode } from "../data/model";
import type { CenterNode } from "../data/types";
import { Cursor, swarmVar } from "../ui";

// 单色顺序色阶：越快越亮（白 -> 墨）。格子里一定写数字，颜色只是辅助。
export const LAT_BINS = [
  { max: 40, label: "≤40" },
  { max: 80, label: "≤80" },
  { max: 160, label: "≤160" },
  { max: 320, label: "≤320" },
  { max: Infinity, label: ">320" },
];
const binOf = (v: number) => LAT_BINS.findIndex((b) => v <= b.max);

/** 行 = 测量方，列 = 对端；数值是行节点自己测到的 RTT。 */
export function LatencyMatrix({ mesh, graph, onPick, onCell, picked }: {
  mesh: Mesh;
  graph: CenterNode[];
  onPick?: (id: string) => void;
  /** 点格子：公开探针用来切换下面的 24 小时曲线 */
  onCell?: (r: string, c: string) => void;
  picked?: { r: string; c: string } | null;
}) {
  const [hot, setHot] = useState<{ r: string; c: string } | null>(null);
  const rows: MeshNode[] = [...mesh.relays, ...(mesh.self && mesh.self.kind === "device" ? [mesh.self] : [])];
  const seen = new Map<string, Map<string, number>>();
  for (const g of graph) seen.set(g.node_id, new Map(g.direct_peers.map((d) => [d.node_id, d.latency_ms])));
  const val = (r: string, c: string) => seen.get(r)?.get(c);

  const hv = hot ? val(hot.r, hot.c) : undefined;
  const back = hot ? val(hot.c, hot.r) : undefined;
  const name = (id: string) => mesh.byId.get(id)?.hostname ?? id;

  return (
    <div className="matrix">
      <div className="matrix__grid" style={{ gridTemplateColumns: `112px repeat(${rows.length}, minmax(44px, 1fr))` }} onMouseLeave={() => setHot(null)}>
        <span />
        {rows.map((c) => (
          <span key={c.id} className={`matrix__col ${hot?.c === c.id ? "is-hot" : ""}`}>
            {c.hostname}
          </span>
        ))}
        {rows.map((r) => (
          <Fragment key={r.id}>
            <button className={`matrix__row ${hot?.r === r.id ? "is-hot" : ""}`} onClick={() => onPick?.(r.id)}>
              <Cursor color={swarmVar(r.color)} size={13} name={r.hostname} />
              <span>{r.hostname}</span>
              {r.self && <span className="matrix__self">本机</span>}
            </button>
            {rows.map((c) => {
              if (r.id === c.id) return <span key={c.id} className="matrix__cell matrix__cell--diag" />;
              const v = val(r.id, c.id);
              const b = v === undefined ? -1 : binOf(v);
              return (
                <span
                  key={c.id}
                  className={`matrix__cell ${b >= 0 ? `lat-${b}` : "matrix__cell--none"} ${hot && (hot.r === r.id || hot.c === c.id) ? "is-cross" : ""} ${hot?.r === r.id && hot?.c === c.id ? "is-hot" : ""} ${picked && ((picked.r === r.id && picked.c === c.id) || (picked.r === c.id && picked.c === r.id)) ? "is-picked" : ""} ${onCell && v !== undefined ? "is-clickable" : ""}`}
                  onMouseEnter={() => setHot({ r: r.id, c: c.id })}
                  onClick={onCell && v !== undefined ? () => onCell(r.id, c.id) : undefined}
                >
                  {v === undefined ? "·" : Math.round(v)}
                </span>
              );
            })}
          </Fragment>
        ))}
      </div>
      <div className="matrix__foot">
        <div className="matrix__legend" aria-label="延迟色阶（毫秒）">
          {LAT_BINS.map((b, i) => (
            <span key={b.label} className="matrix__key">
              <i className={`lat-${i}`} />
              {b.label}
            </span>
          ))}
          <span className="matrix__unit">ms</span>
        </div>
        <div className="matrix__read mono">
          {hot && hv !== undefined
            ? `${name(hot.r)} → ${name(hot.c)} ${Math.round(hv)} ms${back !== undefined ? ` · 反向 ${Math.round(back)} ms` : ""}`
            : "行 = 测量方，悬停看双向"}
        </div>
      </div>
    </div>
  );
}
