import { useState } from "react";
import { ago } from "../data/format";
import { Cursor, Meter, StatusDot, swarmVar } from "../ui";
import { bytes, dayLabel, duration, percent, rate, relayLabel } from "./model";
import type { ProbeRelay } from "./types";

const ratio = (p: [number | null, number | null] | null | undefined) =>
  p && p[0] !== null && p[1] ? p[0] / p[1] : null;

/** 一台接入点一张卡片（参照哪吒监控的服务器卡片）：状态、负载、网络、30 天可用率。标题是城市，不写名字。 */
export function RelayCard({ r, color, now, focused }: { r: ProbeRelay; color: number; now: number; focused?: boolean }) {
  const h = r.host;
  const cpu = h?.cpu ?? null;
  const mem = ratio(h?.mem);
  const disk = ratio(h?.disk);
  const tone = (v: number | null, warn: number) => (v !== null && v >= warn ? "warn" : "plain");
  return (
    <article id={`relay-${r.id}`} className={`rcard ${r.online ? "" : "is-off"} ${focused ? "is-focused" : ""}`}>
      <header className="rcard__head">
        <Cursor color={swarmVar(color)} size={20} name={r.id} />
        <div className="rcard__id">
          <div className="rcard__name">{relayLabel(r.region)}</div>
          <div className="rcard__where">
            {r.spec ?? "接入点"} · <span className="mono">{r.region}</span>
          </div>
        </div>
        <span className={`rcard__state ${r.online ? "" : "is-off"}`}>
          <StatusDot status={r.online ? "live" : "bad"} pulse={r.online} />
          {r.online ? "在线" : r.last_seen ? `离线 · ${ago(r.last_seen, now)}` : "还没上报"}
        </span>
      </header>

      <div className="rcard__rows">
        {h?.cpu_model && <Line label="处理器">{h.cpu_model.replace(/\(R\)|\(TM\)|CPU /g, "").replace(/\s+/g, " ")}</Line>}
        <Gauge label="CPU" value={cpu !== null ? cpu / 100 : null} tone={tone(cpu, 80)}>
          {cpu !== null ? `${cpu.toFixed(1)}%` : "—"}
          {h?.cores ? <span className="rcard__dim"> · {h.cores} 核</span> : null}
        </Gauge>
        <Gauge label="内存" value={mem} tone={tone(mem, 0.9)}>
          {bytes(h?.mem?.[0])} / {bytes(h?.mem?.[1], 0)}
        </Gauge>
        <Gauge label="硬盘" value={disk} tone={tone(disk, 0.9)}>
          {bytes(h?.disk?.[0])} / {bytes(h?.disk?.[1], 0)}
        </Gauge>
        <Line label="网络">
          <span className="rcard__io">↓ {rate(h?.net.rx)}</span>
          <span className="rcard__io">↑ {rate(h?.net.tx)}</span>
        </Line>
        <Line label="流量">
          <span className="rcard__io">↓ {bytes(h?.net.rx_total)}</span>
          <span className="rcard__io">↑ {bytes(h?.net.tx_total)}</span>
        </Line>
        <Line label="负载">{h?.load ? h.load.map((v) => (v === null ? "—" : v.toFixed(2))).join("  ") : "—"}</Line>
        <Line label="运行">{duration(h?.uptime)}</Line>
      </div>

      <Availability days={r.days} uptime30={r.uptime30} />

      <footer className="rcard__foot mono">
        <span>EasyTier {r.et?.version?.split("-")[0] ?? "—"}</span>
        <span>{h?.os ?? ""}</span>
      </footer>
    </article>
  );
}

function Gauge({ label, value, tone, children }: { label: string; value: number | null; tone: "plain" | "warn"; children: React.ReactNode }) {
  return (
    <div className="rcard__row">
      <span className="rcard__k">{label}</span>
      <span className="rcard__gauge">
        <Meter value={value ?? 0} tone={tone} />
      </span>
      <span className="rcard__v mono">{children}</span>
    </div>
  );
}

function Line({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="rcard__row rcard__row--line">
      <span className="rcard__k">{label}</span>
      <span className="rcard__v mono">{children}</span>
    </div>
  );
}

const level = (up: number | null) => (up === null ? "none" : up >= 0.999 ? "ok" : up >= 0.95 ? "warn" : "bad");

/** 30 天可用率：一天一格。满格是墨色，不满才出状态色；还没开始记录的日子是空框。 */
function Availability({ days, uptime30 }: { days: ProbeRelay["days"]; uptime30: number | null }) {
  const [hot, setHot] = useState<number | null>(null);
  const d = hot !== null ? days[hot] : null;
  return (
    <div className="avail">
      <div className="avail__head">
        <span className="cap">30 天</span>
        <span className="avail__read mono">
          {d ? `${dayLabel(d.day)} · ${d.up === null ? "没有数据" : percent(d.up)}` : uptime30 === null ? "还没有数据" : percent(uptime30)}
        </span>
      </div>
      <div className="avail__bars" onMouseLeave={() => setHot(null)} role="img" aria-label={`过去 30 天可用率 ${percent(uptime30)}`}>
        {days.map((x, i) => (
          <span
            key={x.day}
            className={`avail__bar is-${level(x.up)} ${hot === i ? "is-hot" : ""}`}
            onMouseEnter={() => setHot(i)}
            title={`${dayLabel(x.day)} ${x.up === null ? "没有数据" : percent(x.up)}`}
          />
        ))}
      </div>
    </div>
  );
}
