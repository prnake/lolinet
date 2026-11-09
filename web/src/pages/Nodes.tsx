import { useMemo, useState } from "react";
import { ms, natLabel, pct } from "../data/format";
import { kindLabel, nodesDoc, type Kind, type MeshNode } from "../data/model";
import { useMesh } from "../data/store";
import { go } from "../router";
import { Cap, Chip, CopyText, Cursor, Drawer, GATEWAY_COLOR, Empty, Field, KV, List, Meter, Row, Segmented, StatusDot, swarmVar } from "../ui";
import { PathChain } from "../viz/PathChain";

type Filter = "all" | Kind;

const GROUPS: { kind: Kind; title: string }[] = [
  { kind: "relay", title: "接入点" },
  { kind: "device", title: "设备" },
  { kind: "gateway", title: "网关" },
];

export function Nodes({ id }: { id?: string }) {
  const { mesh } = useMesh();
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const shown = useMemo(() => {
    if (!mesh) return [];
    const needle = q.trim().toLowerCase();
    return mesh.nodes.filter(
      (n) =>
        (filter === "all" || n.kind === filter) &&
        (!needle ||
          [n.hostname, n.ipv4, n.region?.city, n.region?.en, n.direct?.proto, n.via, n.id].some((v) => v?.toLowerCase().includes(needle))),
    );
  }, [mesh, q, filter]);
  if (!mesh) return null;
  const open = id ? mesh.byId.get(id) : undefined;

  return (
    <div className="stack-6">
      <header className="pagehead">
        <div>
          <h1 className="pagehead__title">节点</h1>
          <p className="pagehead__sub">
            全网 {mesh.nodes.length} 个节点，从本机 {mesh.self?.hostname ?? ""} 看过去的延迟和走法。
          </p>
        </div>
        <div className="pagehead__tools">
          <Field icon="search" placeholder="筛选名字、IP、城市、协议…" value={q} onChange={(e) => setQ(e.target.value)} />
          <Segmented<Filter>
            value={filter}
            onChange={setFilter}
            options={[
              { value: "all", label: "全部" },
              { value: "relay", label: "接入点" },
              { value: "device", label: "设备" },
              { value: "gateway", label: "网关" },
            ]}
          />
        </div>
      </header>

      {shown.length === 0 && <Empty icon="search" title="没有匹配的节点" />}
      {GROUPS.map((g) => {
        const list = shown.filter((n) => n.kind === g.kind).sort((a, b) => Number(b.self) - Number(a.self));
        if (!list.length) return null;
        return (
          <section key={g.kind} className="stack-2">
            <Cap count={list.length}>{g.title}</Cap>
            <List>
              {list.map((n) => (
                <NodeRow key={n.id} n={n} active={open?.id === n.id} />
              ))}
            </List>
          </section>
        );
      })}

      <Drawer open={!!open} onClose={() => go("nodes")} label="节点详情">
        {open && <NodeDetail n={open} />}
      </Drawer>
    </div>
  );
}

function NodeRow({ n, active }: { n: MeshNode; active: boolean }) {
  const loss = n.direct?.loss ?? 0;
  return (
    <Row
      active={active}
      onClick={() => go(`nodes/${n.id}`)}
      lead={<Cursor color={n.kind === "gateway" ? GATEWAY_COLOR : swarmVar(n.color)} name={n.hostname} />}
      title={
        <>
          {n.hostname}
          {n.self && <Chip tone="eye">本机</Chip>}
        </>
      }
      meta={
        <>
          <span className="mono">{n.ipv4 ?? "—"}</span>
          <span>{n.region ? `${n.region.city} · ${n.relay?.region}` : kindLabel(n)}</span>
        </>
      }
    >
      {loss > 0.02 && <Chip tone="warn">丢包 {pct(loss)}</Chip>}
      {n.via && n.via !== "DIRECT" && <Chip tone="quiet">经 {n.via}</Chip>}
      {n.direct?.proto && <Chip>{n.direct.proto.toUpperCase()}</Chip>}
      {!n.direct && !n.self && !(n.via && n.via !== "DIRECT") && <Chip tone="quiet">中转</Chip>}
      <span className="row__lat mono">{n.self ? "—" : ms(n.latency)}</span>
    </Row>
  );
}

function NodeDetail({ n }: { n: MeshNode }) {
  const { mesh, snap } = useMesh();
  if (!mesh || !snap) return null;
  const center = snap.graph.find((g) => g.node_id === n.id);
  const peers = (center?.direct_peers ?? [])
    .map((d) => ({ node: mesh.byId.get(d.node_id), lat: d.latency_ms }))
    .filter((x): x is { node: MeshNode; lat: number } => !!x.node)
    .sort((a, b) => a.lat - b.lat);
  const max = Math.max(1, ...peers.map((p) => p.lat));
  const zone = nodesDoc.network.edge_zone;

  return (
    <div className="detail">
      <div className="detail__head">
        <Cursor color={n.kind === "gateway" ? GATEWAY_COLOR : swarmVar(n.color)} size={28} name={n.hostname} />
        <div>
          <div className="detail__name">
            {n.hostname}
            <Chip tone={n.self ? "eye" : "plain"} mono={false}>
              {n.self ? "本机" : kindLabel(n)}
            </Chip>
          </div>
          <div className="detail__sub">
            <StatusDot status="live" />
            在线 · <span className="mono">{n.ipv4 ?? "无 overlay 地址"}</span>
            {n.region && ` · ${n.region.city}`}
          </div>
        </div>
      </div>

      {!n.self && (
        <section className="stack-3">
          <Cap>本机到它</Cap>
          <PathChain mesh={mesh} target={n} />
          <div className="kvs">
            <KV k="实际延迟" mono>{ms(n.latency)}</KV>
            <KV k="直连" mono>
              {n.direct ? `${ms(n.direct.lat)} · ${n.direct.proto.toUpperCase()}` : n.cost || "—"}
            </KV>
            {n.direct && <KV k="丢包" mono>{pct(n.direct.loss)}</KV>}
            {n.direct && (
              <KV k="收 / 发" mono>
                {n.direct.rx} / {n.direct.tx}
              </KV>
            )}
          </div>
        </section>
      )}

      {peers.length > 0 && (
        <section className="stack-3">
          <Cap count={peers.length}>它的直连</Cap>
          <div className="bars">
            {peers.map(({ node, lat }) => (
              <button key={node.id} className="bars__row" onClick={() => go(`nodes/${node.id}`)}>
                <span className="bars__name">
                  <Cursor color={node.kind === "gateway" ? GATEWAY_COLOR : swarmVar(node.color)} size={12} name={node.hostname} />
                  {node.hostname}
                  {node.kind === "gateway" && <span className="bars__tag">网关</span>}
                </span>
                <Meter value={lat / max} tone={node.self ? "eye" : "plain"} />
                <span className="bars__val mono">{ms(lat)}</span>
              </button>
            ))}
          </div>
        </section>
      )}

      <section className="stack-3">
        <Cap>信息</Cap>
        <div className="kvs">
          <KV k="peer id">
            <CopyText text={n.id} />
          </KV>
          {n.ipv4 && (
            <KV k="内网名">
              <CopyText text={`${n.hostname}${nodesDoc.network.local_suffix}`} />
            </KV>
          )}
          {n.kind === "relay" && (
            <KV k="公网名">
              <CopyText text={`${n.hostname}.${zone}`} />
            </KV>
          )}
          {n.relay && <KV k="区域" mono>{`${n.relay.region} · ${n.relay.zone}`}</KV>}
          {n.relay && <KV k="VPC" mono>{`10.${n.relay.net}.0.0/16`}</KV>}
          <KV k="NAT">{natLabel(n.nat)}</KV>
          <KV k="版本" mono>{n.version ?? "—"}</KV>
          {n.self && snap.node && (
            <>
              <KV k="模式" mono>{snap.node.no_tun ? `no-tun · ${snap.node.socks5 ?? "无 SOCKS5"}` : "TUN"}</KV>
              <KV k="入口" mono>
                <span className="kv__list">
                  {snap.node.peer_uris.map((u) => (
                    <span key={u}>{u.replace(/\/\?sni=.*/, "")}</span>
                  ))}
                </span>
              </KV>
            </>
          )}
        </div>
      </section>
    </div>
  );
}
