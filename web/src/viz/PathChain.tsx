import { Fragment } from "react";
import { ms } from "../data/format";
import { edgeBetween, kindLabel, type Mesh, type MeshNode } from "../data/model";
import { Cursor, GATEWAY_COLOR, Icon, swarmVar } from "../ui";
import { pathTo } from "./Topology";

/** 链路（参照 vibe42 的加密链和 magpie 的流向图）：本机 → 中转 → 目标，每段写延迟。 */
export function PathChain({ mesh, target }: { mesh: Mesh; target: MeshNode }) {
  const path = pathTo(mesh, target);
  if (path.length < 2) return <div className="chain chain--empty">这就是本机。</div>;
  const direct = target.direct?.lat ?? null;
  const saved = mesh.latencyFirst && direct !== null && target.latency !== null && path.length > 2 ? direct - target.latency : 0;
  return (
    <div className="chain-wrap">
      <div className="chain">
        {path.map((n, i) => (
          <Fragment key={n.id}>
            {i > 0 && <Link mesh={mesh} a={path[i - 1]} b={n} first={i === 1} />}
            <div className={`chain__box ${i === 0 ? "is-self" : ""} ${i > 0 && i < path.length - 1 ? "is-hop" : ""}`}>
              <span className="chain__role">{i === 0 ? "本机" : i < path.length - 1 ? "中转" : kindLabel(n)}</span>
              <span className="chain__name">
                <Cursor color={n.kind === "gateway" ? GATEWAY_COLOR : swarmVar(n.color)} size={14} name={n.hostname} />
                {n.hostname}
              </span>
              <span className="chain__ip mono">{n.ipv4 ?? "无 overlay 地址"}</span>
            </div>
          </Fragment>
        ))}
      </div>
      <div className="chain__sum mono">
        <span>
          合计 <b>{ms(target.latency)}</b> · {path.length - 1} 跳
        </span>
        {saved > 1 && (
          <span className="chain__saved">
            <Icon name="bolt" size={12} />
            延迟优先绕路，比直连（{ms(direct)}）快 {ms(saved)}
          </span>
        )}
      </div>
    </div>
  );
}

function Link({ mesh, a, b, first }: { mesh: Mesh; a: MeshNode; b: MeshNode; first: boolean }) {
  const e = edgeBetween(mesh, a.id, b.id);
  // 第一段是本机自己的直连，有隧道协议；后面的段只有 peer-center 的延迟
  const proto = first ? b.direct?.proto : undefined;
  return (
    <div className="chain__link">
      <span className="chain__line" />
      <span className="chain__label mono">
        <Icon name="lock" size={11} />
        {e ? ms(e.latency) : "—"}
        {proto && <span className="chain__proto">{proto}</span>}
      </span>
    </div>
  );
}
