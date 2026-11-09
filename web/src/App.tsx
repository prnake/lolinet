import { useEffect, useState } from "react";
import { ago } from "./data/format";
import { nodesDoc } from "./data/model";
import { MeshProvider, useMesh, useNow } from "./data/store";
import { Access } from "./pages/Access";
import { Design } from "./pages/Design";
import { Nodes } from "./pages/Nodes";
import { Overview } from "./pages/Overview";
import { Pair } from "./pages/Pair";
import { go, useRoute } from "./router";
import { Button, Chip, CommandPill, Cursor, Icon, Mark, StatusDot, swarmVar, ThemeSwitch, ToastProvider, type IconName } from "./ui";

const NAV: { id: string; label: string; icon: IconName }[] = [
  { id: "overview", label: "总览", icon: "overview" },
  { id: "nodes", label: "节点", icon: "nodes" },
  { id: "access", label: "凭据", icon: "key" },
  { id: "pair", label: "配对", icon: "pair" },
];

export default function App() {
  return (
    <ToastProvider>
      <MeshProvider>
        <Shell />
      </MeshProvider>
    </ToastProvider>
  );
}

function Shell() {
  const { phase } = useMesh();
  const route = useRoute();
  if (route[0] === "design") return <Frame route={route}><Design /></Frame>;
  if (phase !== "ready") return <Boot />;
  const page = { overview: <Overview />, nodes: <Nodes id={route[1]} />, access: <Access />, pair: <Pair /> }[route[0]] ?? <Overview />;
  return <Frame route={route}>{page}</Frame>;
}

function Frame({ route, children }: { route: string[]; children: React.ReactNode }) {
  return (
    <div className="app">
      <Sidebar active={route[0]} />
      <main className="main">
        <Topbar section={route[0]} />
        <div className="page" key={route[0]}>
          {children}
        </div>
      </main>
    </div>
  );
}

function Sidebar({ active }: { active: string }) {
  const { mesh, info, backend, phase } = useMesh();
  const self = mesh?.self;
  const counts: Record<string, number | undefined> = { nodes: mesh?.nodes.length };
  return (
    <aside className="side">
      <a className="side__brand" href="#/overview">
        <Mark size={24} state={phase === "ready" ? "live" : phase === "connecting" ? "connecting" : "off"} />
        <span className="side__name">lo-li mesh</span>
      </a>
      <div className="side__net mono">
        {nodesDoc.network.name} · {nodesDoc.network.cidr}
      </div>
      <nav className="side__nav">
        {NAV.map((n) => (
          <a key={n.id} href={`#/${n.id}`} className={`side__item ${active === n.id ? "is-on" : ""}`}>
            <Icon name={n.icon} />
            <span>{n.label}</span>
            {counts[n.id] !== undefined && <span className="side__count mono">{counts[n.id]}</span>}
          </a>
        ))}
        <div className="side__gap" />
        <a href="#/design" className={`side__item ${active === "design" ? "is-on" : ""}`}>
          <Icon name="design" />
          <span>设计语言</span>
        </a>
      </nav>
      <div className="side__foot">
        {self && (
          <button className="side__self" onClick={() => go(`nodes/${self.id}`)}>
            <Cursor color={swarmVar(self.color)} size={16} name={self.hostname} />
            <span className="side__self-main">
              <span className="side__self-name">
                {self.hostname}
                <StatusDot status="live" pulse />
              </span>
              <span className="side__self-ip mono">{self.ipv4 ?? "—"}</span>
            </span>
          </button>
        )}
        <div className="side__agent mono">
          {backend.demo ? (
            <Chip tone="warn">演示数据</Chip>
          ) : (
            info && (
              <>
                {info.read_only && <Chip>只读</Chip>} agent {info.agent} · rpc {info.rpc.split(":").pop()}
              </>
            )
          )}
        </div>
      </div>
    </aside>
  );
}

const TITLES: Record<string, string> = { overview: "总览", nodes: "节点", access: "凭据", pair: "配对", design: "设计语言" };

function Topbar({ section }: { section: string }) {
  const { updatedAt, error, refresh, snap, phase } = useMesh();
  const now = useNow();
  const age = (now - updatedAt) / 1000;
  const coreDown = !!snap?.errors.length && !snap.node;
  const status = error || coreDown ? "bad" : age > 10 ? "warn" : "live";
  return (
    <header className="top">
      <div className="top__crumb cap">
        <span>{nodesDoc.network.name}</span>
        <span className="top__slash">/</span>
        <span className="top__here">{TITLES[section] ?? section}</span>
      </div>
      {phase === "ready" && (
        <div className="top__right">
          <span className={`top__fresh top__fresh--${status}`} title={error ?? snap?.errors.join("\n") ?? ""}>
            <StatusDot status={status} pulse={status === "live"} />
            {error ? "agent 断开" : coreDown ? `内核没响应 · ${snap?.rpc}` : updatedAt ? `${ago(updatedAt / 1000, now / 1000)}更新` : "—"}
          </span>
          <button className="iconbtn" onClick={refresh} aria-label="刷新">
            <Icon name="refresh" />
          </button>
          <ThemeSwitch />
        </div>
      )}
    </header>
  );
}

const CAPTIONS = ["正在连接 agent", "CONNECTING", "接続中", "CONNEXION", "VERBINDEN", "連線中"];

/** 开屏：致敬 uwu.cat —— 虚空、星号、绕着它飞的光标、轮换语言的等宽标注。 */
function Boot() {
  const { phase, retry, enterDemo } = useMesh();
  const [i, setI] = useState(0);
  useEffect(() => {
    if (phase !== "connecting") return;
    const id = setInterval(() => setI((x) => (x + 1) % CAPTIONS.length), 1400);
    return () => clearInterval(id);
  }, [phase]);
  const offline = phase === "offline";
  return (
    <div className="boot">
      <div className="boot__theme">
        <ThemeSwitch />
      </div>
      <div className="boot__stage">
        <div className="boot__swarm" aria-hidden="true">
          {nodesDoc.relays.map((r, k) => (
            <span key={r.name} className="boot__orbit" style={{ ["--k" as string]: k }}>
              <Cursor color={swarmVar(k)} size={20} tilt={k * 47} />
            </span>
          ))}
        </div>
        <Mark size={72} state={offline ? "off" : "connecting"} />
      </div>
      <span className="boot__chev" aria-hidden="true" />
      <p className="boot__cap" key={offline ? "off" : i}>
        {offline ? "agent 没有响应" : CAPTIONS[i]}
      </p>
      {offline && (
        <div className="boot__off">
          <CommandPill command="python3 web/agent/mesh_agent.py --allow-pair" />
          <div className="boot__actions">
            <Button variant="ghost" icon="refresh" onClick={retry}>
              重试
            </Button>
            <Button variant="primary" iconRight="arrow" onClick={enterDemo}>
              看演示数据
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
