import { useMemo, useState } from "react";
import { Cursor, swarmVar } from "../ui";
import { hhmm } from "./model";
import type { LinkHistory } from "./types";

const GAP = 900; // 秒
const W = 640;
const H = 220;
const PAD = { l: 44, r: 14, t: 14, b: 26 };

/**
 * 一条链路 24 小时的延迟：两条线各是一端测到的 RTT。线用墨色、实线 / 虚线区分方向，
 * 测量方是谁看图例里的光标（两个光标色可能很接近，不靠线色区分）。一根 y 轴、2px 线、悬停十字线 + 两个值。
 */
const STYLE = [
  { stroke: "var(--fg)", dash: undefined },
  { stroke: "var(--fg-3)", dash: "5 4" },
];
export function LinkChart({ data, colorOf, labelOf }: {
  data: LinkHistory | null;
  colorOf: (id: string) => number;
  /** 区域代码 -> 城市 */
  labelOf: (id: string) => string;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const series = useMemo(
    () => (data ? [{ from: data.a, to: data.b, pts: data.ab }, { from: data.b, to: data.a, pts: data.ba }].filter((s) => s.pts.length) : []),
    [data],
  );
  if (!data) return <div className="lchart lchart--empty">点上面延迟矩阵里的一格，看这条链路 24 小时的延迟。</div>;
  if (!series.length) return <div className="lchart lchart--empty">这条链路还没有历史数据：探针每 5 分钟记一格。</div>;

  const all = series.flatMap((s) => s.pts);
  // 横轴固定是最近 24 小时：刚开始记录时曲线只占右边一小段，如实反映
  const t1 = Math.max(Date.now() / 1000, ...all.map((p) => p.t));
  const t0 = t1 - 86400;
  const first = Math.min(...all.map((p) => p.t));
  const max = Math.max(...all.map((p) => p.lat)) * 1.15 || 1;
  const step = max > 200 ? 100 : max > 80 ? 40 : max > 40 ? 20 : 10;
  const x = (t: number) => PAD.l + ((t - t0) / (t1 - t0)) * (W - PAD.l - PAD.r);
  const y = (v: number) => H - PAD.b - (v / max) * (H - PAD.t - PAD.b);
  const ticks = Array.from({ length: Math.floor(max / step) + 1 }, (_, i) => i * step);
  const hours = Array.from({ length: 5 }, (_, i) => t0 + ((t1 - t0) * i) / 4);
  const nearest = (pts: { t: number; lat: number }[], t: number) =>
    pts.reduce((a, p) => (Math.abs(p.t - t) < Math.abs(a.t - t) ? p : a), pts[0]);
  // 相邻两格隔了 15 分钟以上就断开：中断不画成一条平线
  const path = (pts: { t: number; lat: number }[]) =>
    pts.map((p, i) => `${i && p.t - pts[i - 1].t <= GAP ? "L" : "M"}${x(p.t).toFixed(1)},${y(p.lat).toFixed(1)}`).join("");
  const lonely = (pts: { t: number; lat: number }[]) =>
    pts.filter((p, i) => !(i && p.t - pts[i - 1].t <= GAP) && !(i < pts.length - 1 && pts[i + 1].t - p.t <= GAP));

  const onMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    const px = ((e.clientX - box.left) / box.width) * W;
    const t = t0 + ((px - PAD.l) / (W - PAD.l - PAD.r)) * (t1 - t0);
    if (px < PAD.l || px > W - PAD.r || t < first - 600) return setHover(null);
    setHover(t);
  };
  const at = hover !== null ? series.map((s) => ({ ...s, p: nearest(s.pts, hover) })) : [];

  return (
    <div className="lchart">
      <div className="lchart__legend">
        {series.map((s, i) => (
          <span key={s.from} className="lchart__key">
            <svg width="22" height="8" aria-hidden="true">
              <line x1="1" x2="21" y1="4" y2="4" stroke={STYLE[i].stroke} strokeWidth="2" strokeDasharray={STYLE[i].dash} strokeLinecap="round" />
            </svg>
            <Cursor color={swarmVar(colorOf(s.from))} size={12} name={s.from} />
            <span>
              {labelOf(s.from)} → {labelOf(s.to)}
            </span>
          </span>
        ))}
        <span className="lchart__read mono">
          {at.length
            ? `${hhmm(at[0].p.t)} · ${at.map((s) => `${Math.round(s.p.lat)} ms`).join(" / ")}`
            : t1 - first < 6 * 3600
              ? `${hhmm(first)} 开始记录 · RTT，毫秒`
              : "RTT，毫秒 · 北京时间"}
        </span>
      </div>
      <svg viewBox={`0 0 ${W} ${H}`} className="lchart__svg" onMouseMove={onMove} onMouseLeave={() => setHover(null)} role="img"
        aria-label={`${labelOf(data.a)} 和 ${labelOf(data.b)} 之间 24 小时延迟`}>
        {ticks.map((v) => (
          <g key={v}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(v)} y2={y(v)} className="lchart__grid" />
            <text x={PAD.l - 8} y={y(v)} dy="0.35em" textAnchor="end" className="lchart__tick">{v}</text>
          </g>
        ))}
        {hours.map((t) => (
          <text key={t} x={x(t)} y={H - 6} textAnchor="middle" className="lchart__tick">{hhmm(t)}</text>
        ))}
        {series.map((s, k) => (
          <g key={s.from}>
            <path
              d={path(s.pts)}
              fill="none"
              stroke={STYLE[k].stroke}
              strokeDasharray={STYLE[k].dash}
              strokeWidth={2}
              strokeLinejoin="round"
              strokeLinecap="round"
            />
            {lonely(s.pts).map((p) => (
              <circle key={p.t} cx={x(p.t)} cy={y(p.lat)} r={3} fill={STYLE[k].stroke} />
            ))}
          </g>
        ))}
        {at.length > 0 && (
          <g>
            <line x1={x(at[0].p.t)} x2={x(at[0].p.t)} y1={PAD.t} y2={H - PAD.b} className="lchart__cross" />
            {at.map((s, k) => (
              <circle key={s.from} cx={x(s.p.t)} cy={y(s.p.lat)} r={4.5} fill={STYLE[k].stroke} className="lchart__dot" />
            ))}
          </g>
        )}
      </svg>
    </div>
  );
}
