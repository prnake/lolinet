import { useState } from "react";
import { ms, natLabel } from "../data/format";
import { nodesDoc } from "../data/model";
import { useMesh } from "../data/store";
import { go } from "../router";
import { Button, Cap, CommandPill, Cursor, Headline, Mark, Panel, Stat, swarmVar } from "../ui";
import { LatencyMatrix } from "../viz/LatencyMatrix";
import { PathChain } from "../viz/PathChain";
import { Topology } from "../viz/Topology";

export function Overview() {
  const { mesh, snap } = useMesh();
  const [sel, setSel] = useState<string | null>(null);
  if (!mesh || !snap) return null;

  const self = mesh.self;
  const total = nodesDoc.relays.length;
  const up = mesh.relays.length;
  // 「最近」按实际走的延迟算（开了延迟优先就是绕路后的值），和拓扑图上的数字一致
  const nearest = mesh.relays.filter((r) => r.latency !== null).sort((a, b) => a.latency! - b.latency!)[0];
  const others = mesh.nodes.filter((n) => !n.self);
  const direct = others.filter((n) => n.cost === "p2p").length;
  const detour = others.filter((n) => n.via && n.via !== "DIRECT").length;
  const issuer = mesh.relays.find((r) => r.hostname === nodesDoc.network.issuer);
  const target = (sel && mesh.byId.get(sel)) || issuer || mesh.relays[0];
  const proto = nearest?.direct?.proto?.toUpperCase();

  return (
    <div className="stack-8">
      <section className="hero">
        <Headline
          lead={up === total ? `${total} 个接入点全部在线。` : `${up}/${total} 个接入点在线。`}
          sub={
            self && nearest
              ? `离本机最近的是 ${nearest.hostname}（${nearest.region?.city}），${ms(nearest.latency)}${proto ? ` · ${proto}` : ""}。`
              : "本机还没连上任何接入点。"
          }
        />
        <div className="stats">
          <Stat value={up} unit={`/${total}`} label="接入点" tone={up === total ? "eye" : "warn"} />
          <Stat value={mesh.devices.length} label="设备" />
          <Stat value={direct} label="直连 p2p" />
          <Stat value={detour} label="延迟优先绕路" />
          <Stat value={nearest ? Math.round(nearest.latency!) : "—"} unit="ms" label="最近接入点" />
          <Stat value={<span className="stat__text">{natLabel(self?.nat)}</span>} label="本机 NAT" />
        </div>
      </section>

      {!snap.node ? (
        <CoreDown rpc={snap.rpc} errors={snap.errors} />
      ) : (
        <>
          <Panel pad={false} className="topo-panel">
            <div className="panel__head">
              <Cap>拓扑</Cap>
              <span className="panel__hint">悬停看延迟，点一下看本机怎么走过去</span>
            </div>
            <Topology mesh={mesh} selected={target?.id} onSelect={setSel} />
          </Panel>

          <div className="grid-2">
            <Panel>
              <div className="panel__head panel__head--flush">
                <Cap>链路</Cap>
                {target && (
                  <Button size="sm" variant="quiet" iconRight="chevron" onClick={() => go(`nodes/${target.id}`)}>
                    {target.hostname} 详情
                  </Button>
                )}
              </div>
              <div className="pick">
                {mesh.relays.map((r) => (
                  <button key={r.id} className={`pick__item ${target?.id === r.id ? "is-on" : ""}`} onClick={() => setSel(r.id)}>
                    <Cursor color={swarmVar(r.color)} size={12} name={r.hostname} />
                    {r.hostname}
                  </button>
                ))}
              </div>
              {target && <PathChain mesh={mesh} target={target} />}
              <div className="facts">
                <Fact on={!!snap.node?.credential_peer} title="凭据入网" body="本机只持有自己的凭据，没有 network_secret；丢了在控制台吊销。" />
                <Fact on={!!snap.node?.secure_mode} title="secure mode" body={`节点间加密握手；外层 WSS 的 SNI 伪装成 ${nodesDoc.network.sni}。`} />
                <Fact on={!!snap.node?.latency_first} title="延迟优先" body="按实测延迟选路，直连慢时自动经更近的接入点中转。" />
              </div>
            </Panel>
            <Panel>
              <div className="panel__head panel__head--flush">
                <Cap>延迟矩阵</Cap>
                <span className="panel__hint">RTT，毫秒</span>
              </div>
              <LatencyMatrix mesh={mesh} graph={snap.graph} onPick={(id) => setSel(id)} />
            </Panel>
          </div>
        </>
      )}
    </div>
  );
}

/** 本机内核没跑：不画空拓扑，直接说怎么办。 */
function CoreDown({ rpc, errors }: { rpc: string; errors: string[] }) {
  return (
    <Panel className="coredown">
      <Mark size={40} state="off" />
      <div className="coredown__title">本机的 easytier-core 没有响应</div>
      <p className="coredown__body">
        agent 读的是 <code>{rpc}</code>。先 <code>sudo lolinet up</code> 装好系统服务（管理口 15888），或者起一个 no-tun 实例（如菜单栏 App，管理口 15897），再用 <code>--rpc</code> 指过去。
      </p>
      <CommandPill command={`python3 web/agent/mesh_agent.py --rpc 127.0.0.1:15897 --socks 127.0.0.1:18080`} />
      {errors[0] && <div className="coredown__err mono">{errors[0]}</div>}
    </Panel>
  );
}

function Fact({ on, title, body }: { on: boolean; title: string; body: string }) {
  return (
    <div className={`fact ${on ? "is-on" : ""}`}>
      <div className="fact__title">
        <span className="fact__mark">{on ? "开" : "关"}</span>
        {title}
      </div>
      <div className="fact__body">{body}</div>
    </div>
  );
}
