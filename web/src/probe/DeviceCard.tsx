import { ago } from "../data/format";
import { Chip, Cursor, StatusDot } from "../ui";
import { bytes, deviceColor, deviceLocation, deviceTitle, relayLabel } from "./model";
import type { ProbeDevice } from "./types";

const chipLabel = (s: string | null) => (s ? s.replace(/\(R\)|\(TM\)|CPU /g, "").replace(/\s+/g, " ").trim() : null);

const STATE = { online: "在线", standby: "未入网", offline: "离线" } as const;

/**
 * 一台设备一张卡片：硬件（装了探针才有）、大致位置、直连了哪些接入点、在不在网里。没有名字和地址。
 * 光标颜色由匿名 id 决定，和地球上那只是同一只。
 */
export function DeviceCard({ d, now, focused }: { d: ProbeDevice; now: number; focused?: boolean }) {
  const storage = d.disks.filter((x) => x.total).map((x) => bytes(x.total, 1));
  return (
    <article id={`device-${d.id}`} className={`dcard ${d.online ? "" : "is-off"} ${focused ? "is-focused" : ""}`}>
      <header className="dcard__head">
        <Cursor color={deviceColor(d)} size={18} name={d.id} />
        <div className="dcard__id">
          <div className="dcard__name">
            {deviceTitle(d)}
            {d.kind && d.model && <Chip mono={false}>{d.kind}</Chip>}
          </div>
          <div className="dcard__where">{deviceLocation(d)}</div>
        </div>
        <span className={`rcard__state ${d.status === "offline" ? "is-off" : ""}`}>
          <StatusDot status={d.online ? "live" : d.status === "standby" ? "warn" : "idle"} pulse={d.online} />
          {d.status === "offline" && d.last_seen ? `离线 · ${ago(d.last_seen, now)}` : STATE[d.status]}
        </span>
      </header>
      <dl className="dcard__spec">
        <Spec k="接入">
          {d.links.length ? (
            d.links.slice(0, 3).map((l, i) => (
              <span key={l.region} className="dcard__link">
                {i > 0 && <span className="rcard__dim"> · </span>}
                {relayLabel(l.region)} {l.lat} ms
              </span>
            ))
          ) : (
            <span className="rcard__dim">{d.online ? "经中转" : "—"}</span>
          )}
        </Spec>
        {d.hardware ? (
          <>
            <Spec k="处理器">
              {chipLabel(d.chip) ?? "—"}
              {d.cores ? <span className="rcard__dim"> · {d.cores} 核</span> : null}
            </Spec>
            <Spec k="内存">{bytes(d.mem, 0)}</Spec>
            <Spec k="存储">{storage.length ? storage.join(" + ") : "—"}</Spec>
            <Spec k="系统">
              {d.os ?? "—"}
              {d.arch ? <span className="rcard__dim"> · {d.arch}</span> : null}
            </Spec>
          </>
        ) : (
          <p className="dcard__note">没装探针，只知道它在网里；硬件不公开。</p>
        )}
      </dl>
    </article>
  );
}

function Spec({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div className="dcard__row">
      <dt>{k}</dt>
      <dd className="mono">{children}</dd>
    </div>
  );
}
