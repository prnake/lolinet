import { useCallback, useEffect, useMemo, useState } from "react";
import nodesDoc from "virtual:mesh-nodes";
import { ago } from "../data/format";
import { regionOf } from "../data/regions";
import { Button, Cap, Chip, CommandPill, Headline, Mark, Panel, Stat, StatusDot, ThemeSwitch } from "../ui";
import { LatencyMatrix } from "../viz/LatencyMatrix";
import { GlobeTopology } from "../viz/GlobeTopology";
import { Topology } from "../viz/Topology";
import { demoLink, demoSnapshot } from "./demo";
import { LinkChart } from "./LinkChart";
import { DeviceCard } from "./DeviceCard";
import { buildGlobe, buildProbeMesh, medianLatency, percent, rate, relayLabel } from "./model";
import { RelayCard } from "./RelayCard";
import type { LinkHistory, ProbeSnapshot } from "./types";

const POLL_MS = 15000; // Worker 缓存 10 秒，各 relay 30 秒上报一次
const JOIN = nodesDoc.network.join;
// 全网：北太平洋居中，硅谷和新加坡都在正面；东亚：拉近到中国沿海，五个接入点和设备散开。?view=asia 直接打开东亚
const GLOBE_VIEWS = [
  { id: "all", label: "全网", lat: 30, lng: 160, altitude: 1.55 },
  { id: "asia", label: "东亚", lat: 21, lng: 112, altitude: 1.0 },
];
const demo = new URLSearchParams(location.search).has("demo");

// 不从 data/store 引：那边会把控制台的演示快照（含设备名和地址）带进公开包
function useNow(ms = 1000) {
  const [t, setT] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setT(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return t;
}

async function getJson<T>(path: string): Promise<T> {
  const r = await fetch(path, { cache: "no-store" });
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

export function ProbeApp() {
  const [snap, setSnap] = useState<ProbeSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [focus, setFocus] = useState<string | null>(null);
  const [pick, setPick] = useState<{ r: string; c: string } | null>(null);
  const [link, setLink] = useState<LinkHistory | null>(null);
  const now = useNow() / 1000;

  const pull = useCallback(async () => {
    try {
      setSnap(demo ? demoSnapshot() : await getJson<ProbeSnapshot>("/v1/probe"));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);
  useEffect(() => {
    pull();
    const id = setInterval(() => document.visibilityState === "visible" && pull(), POLL_MS);
    return () => clearInterval(id);
  }, [pull]);

  const built = useMemo(() => (snap ? buildProbeMesh(snap) : null), [snap]);
  const globe = useMemo(() => (snap && built ? buildGlobe(snap, built.mesh) : null), [snap, built]);
  const select = useCallback((id: string, kind: "relay" | "device") => {
    setFocus(id);
    document.getElementById(`${kind}-${id}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
  }, []);
  const colorOf = useCallback((id: string) => built?.mesh.byId.get(id)?.color ?? 0, [built]);

  // 默认看最慢的那条链路，延迟曲线最有看头
  useEffect(() => {
    if (pick || !built) return;
    const e = [...built.mesh.edges].sort((a, b) => b.latency - a.latency)[0];
    if (e) setPick({ r: e.a, c: e.b });
  }, [built, pick]);
  useEffect(() => {
    if (!pick) return;
    let dead = false;
    (demo ? Promise.resolve(demoLink(pick.r, pick.c)) : getJson<LinkHistory>(`/v1/probe/link?a=${encodeURIComponent(pick.r)}&b=${encodeURIComponent(pick.c)}`))
      .then((d) => !dead && setLink(d))
      .catch(() => !dead && setLink({ a: pick.r, b: pick.c, ab: [], ba: [] }));
    return () => {
      dead = true;
    };
  }, [pick]);

  if (!snap || !built || !globe) return <Loading error={error} retry={pull} />;

  const total = snap.relays.length;
  const up = snap.relays.filter((r) => r.online).length;
  const median = medianLatency(snap);
  const avail = snap.relays.map((r) => r.uptime30).filter((v): v is number => v !== null);
  const avail30 = avail.length ? avail.reduce((a, b) => a + b, 0) / avail.length : null;
  // 可用率按已有天数说：刚开始记录时不写「过去 30 天」
  const days = Math.max(0, ...snap.relays.map((r) => r.days.filter((d) => d.up !== null).length));
  const span = days >= 30 ? "过去 30 天" : `近 ${days} 天`;
  const bw = snap.relays.filter((r) => r.online).reduce((a, r) => a + (r.host?.net.rx ?? 0) + (r.host?.net.tx ?? 0), 0);
  const lastSeen = Math.max(0, ...snap.relays.map((r) => r.last_seen ?? 0));
  const stale = now - snap.now > 60;
  const devOnline = snap.devices.filter((d) => d.online).length;
  const devHidden = snap.devices.filter((d) => !d.hardware).length;
  const sub = `${avail30 !== null ? `${span}可用率 ${percent(avail30)}，` : ""}${snap.peers} 台设备在线。`;

  return (
    <div className="probe">
      <header className="probe__top">
        <a className="probe__brand" href="/">
          <Mark size={22} state={up === total ? "live" : up ? "connecting" : "off"} />
          <span className="probe__name">lo-li mesh</span>
          <span className="cap">status</span>
        </a>
        <div className="probe__right">
          {demo && <Chip tone="warn">演示数据</Chip>}
          <span className={`top__fresh ${error || stale ? "top__fresh--bad" : ""}`} title={error ?? ""}>
            <StatusDot status={error || stale ? "bad" : "live"} pulse={!error && !stale} />
            {error ? "读取失败，稍后重试" : lastSeen ? `${ago(lastSeen, now)}更新` : "等待上报"}
          </span>
          <ThemeSwitch />
        </div>
      </header>

      <main className="probe__main stack-8">
        <section className="hero">
          <Headline lead={up === total ? `${total} 个接入点全部在线。` : `${up}/${total} 个接入点在线。`} sub={sub} />
          <div className="stats">
            <Stat value={up} unit={`/${total}`} label="接入点" tone={up === total ? "eye" : "warn"} />
            <Stat value={snap.peers} label="设备在线" />
            <Stat value={median !== null ? Math.round(median) : "—"} unit="ms" label="中位延迟" />
            <Stat value={avail30 !== null ? percent(avail30).replace("%", "") : "—"} unit="%" label={`${days >= 30 ? 30 : days} 天可用率`} />
            <Stat value={<span className="stat__text">{rate(bw)}</span>} label="实时流量" />
            <Stat value={<span className="stat__text">{regionCount(snap)}</span>} label="覆盖区域" />
          </div>
        </section>

        <Panel pad={false} className="topo-panel">
          <div className="panel__head">
            <Cap count={`${total} + ${snap.devices.length}`}>拓扑</Cap>
            <span className="panel__hint">拖动旋转 · 实线是接入点之间直连，虚线是设备接入 · 点一下看卡片</span>
          </div>
          <GlobeTopology
            nodes={globe.nodes}
            arcs={globe.arcs}
            selected={focus}
            onSelect={select}
            label={`拓扑：${up}/${total} 个接入点在线，${snap.peers} 台设备在网`}
            views={GLOBE_VIEWS}
            initialView={new URLSearchParams(location.search).get("view")}
            fallback={
              <Topology
                mesh={built.mesh}
                selected={focus}
                onSelect={(id) => select(id, "relay")}
                sub={(n) => {
                  const r = snap.relays.find((x) => x.id === n.id);
                  return r?.online ? `CPU ${Math.round(r.host?.cpu ?? 0)}%` : "离线";
                }}
                dim={(n) => !snap.relays.find((x) => x.id === n.id)?.online}
              />
            }
          />
        </Panel>

        <section className="stack-3">
          <Cap count={total}>接入点</Cap>
          <div className="rcards">
            {snap.relays.map((r) => (
              <RelayCard key={r.id} r={r} color={colorOf(r.id)} now={now} focused={focus === r.id} />
            ))}
          </div>
        </section>

        {snap.devices.length > 0 && (
          <section className="stack-3">
            <Cap
              count={`${devOnline}/${snap.devices.length}`}
              right={<span className="panel__hint">只公开硬件和大致位置{devHidden ? ` · ${devHidden} 台没装探针` : ""}</span>}
            >
              设备
            </Cap>
            <div className="dcards">
              {snap.devices.map((d) => (
                <DeviceCard key={d.id} d={d} now={now} focused={focus === d.id} />
              ))}
            </div>
          </section>
        )}

        <div className="grid-2">
          <Panel>
            <div className="panel__head panel__head--flush">
              <Cap>延迟矩阵</Cap>
              <span className="panel__hint">RTT，毫秒 · 点格子看 24 小时</span>
            </div>
            <LatencyMatrix mesh={built.mesh} graph={built.graph} onCell={(r, c) => setPick({ r, c })} picked={pick} />
          </Panel>
          <Panel>
            <div className="panel__head panel__head--flush">
              <Cap>24 小时</Cap>
              {pick && (
                <span className="panel__hint">
                  {relayLabel(pick.r)} ↔ {relayLabel(pick.c)}
                </span>
              )}
            </div>
            <LinkChart data={link && pick && link.a === pick.r && link.b === pick.c ? link : null} colorOf={colorOf} labelOf={relayLabel} />
          </Panel>
        </div>

        <section className="probe__join stack-5">
          <Cap>加入</Cap>
          <ol className="steps">
            <li className="steps__item">
              <span className="steps__num">1</span>
              <span className="steps__text">
                <span className="steps__title">要一个配对码</span>
                <span className="steps__body">找网络管理员签发。15 分钟内有效，只能用一次。</span>
              </span>
            </li>
            <li className="steps__item">
              <span className="steps__num">2</span>
              <span className="steps__text">
                <span className="steps__title">在电脑上执行</span>
                <span className="steps__body">下面这一行，需要 sudo。macOS 和 Linux（x86_64 / arm64）都行。</span>
              </span>
            </li>
            <li className="steps__item">
              <span className="steps__num">3</span>
              <span className="steps__text">
                <span className="steps__title">上线</span>
                <span className="steps__body">设备自己打洞，接到最近的接入点，出现在上面的「设备在线」里。</span>
              </span>
            </li>
          </ol>
          <CommandPill command={`curl -fsSL https://${JOIN}/i | sudo sh -s <配对码>`} size="lg" />
        </section>
      </main>

      <footer className="probe__foot mono">
        <span>只读 · 接入点每 {snap.interval} 秒、设备每分钟自报 · 不公开任何名字和地址，设备只有硬件和大致位置</span>
        <a href="/v1/probe">/v1/probe</a>
      </footer>
    </div>
  );
}

function regionCount(s: ProbeSnapshot) {
  const cities = new Set(s.relays.map((r) => regionOf(r.region)?.city ?? r.region));
  return `${cities.size} 个城市`;
}

function Loading({ error, retry }: { error: string | null; retry: () => void }) {
  return (
    <div className="boot">
      <div className="boot__theme">
        <ThemeSwitch />
      </div>
      <Mark size={64} state={error ? "off" : "connecting"} />
      <span className="boot__chev" aria-hidden="true" />
      <p className="boot__cap">{error ? "读不到网络状态" : "正在读取网络状态"}</p>
      {error && (
        <div className="boot__actions" style={{ marginTop: 24 }}>
          <Button variant="ghost" icon="refresh" onClick={retry}>
            重试
          </Button>
          <Button variant="primary" iconRight="arrow" onClick={() => (location.search = "?demo")}>
            看演示数据
          </Button>
        </div>
      )}
    </div>
  );
}
