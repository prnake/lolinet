import { useState } from "react";
import { useLoad, useMesh } from "../data/store";
import type { ExitNode, MihomoRoute, MihomoRules, MihomoView } from "../data/types";
import { Button, Cap, Chip, Empty, Field, Icon, KV, Panel, useToast } from "../ui";

// 分流只在 manager（web/agent/mesh_manager.py）上有：装了 mihomo 的电脑（lolinet mihomo on）和手机（lolinet invite --kind phone）
// 按这里的规则走：网内地址走 lolinet，然后依次是拒绝、直连、各条线路（固定从某台出口出去）、其余走出口组，最后直连。
// 规则和线路绑哪台，mihomo 每 10 分钟经内网自己取；加了新线路要换配置：电脑自动（6 小时内）或 sudo lolinet mihomo refresh，手机重新导入。

const lines = (t: string) =>
  t
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("#"));
const text = (r: string[]) => r.join("\n");
const same = (a: MihomoRules, b: MihomoRules) => JSON.stringify(a) === JSON.stringify(b);
const RULE_HINT = "一行一条 mihomo 规则，如 DOMAIN-SUFFIX,web.com（也匹配 *.web.com）、DOMAIN-KEYWORD,openai、IP-CIDR,1.2.3.0/24、GEOSITE,netflix；# 开头是注释";

type Draft = { reject: string; direct: string; exit: string; exit_order: string[]; routes: { name: string; exit: string; rules: string }[] };

const toDraft = (m: MihomoRules): Draft => ({
  reject: text(m.reject),
  direct: text(m.direct),
  exit: text(m.exit),
  exit_order: [...m.exit_order],
  routes: m.routes.map((r) => ({ name: r.name, exit: r.exit, rules: text(r.rules) })),
});
const fromDraft = (d: Draft): MihomoRules => ({
  reject: lines(d.reject),
  direct: lines(d.direct),
  exit: lines(d.exit),
  exit_order: d.exit_order,
  routes: d.routes.map((r): MihomoRoute => ({ name: r.name.trim(), exit: r.exit.trim(), rules: lines(r.rules) })),
});

export function Routing() {
  const { backend } = useMesh();
  const view = useLoad(() => backend.mihomo(), [backend]);
  if (view.error) return <Empty icon="warn" title="读不到分流规则">{view.error.message}</Empty>;
  if (!view.data) return null;
  return <Form view={view.data} onSaved={view.setData} />;
}

function RuleBox({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder?: string }) {
  const rows = Math.min(12, Math.max(3, value.split("\n").length + 1));
  return (
    <textarea className="rulebox mono" rows={rows} value={value} spellCheck={false} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} />
  );
}

function routeError(r: Draft["routes"][number], all: Draft["routes"]): string | null {
  const name = r.name.trim();
  if (!name || name.length > 24 || /[,"'\\]/.test(name)) return "线路名 1–24 个字，不能有逗号、引号";
  if (all.filter((x) => x.name.trim() === name).length > 1) return "线路名重复了";
  if (!/^[a-z0-9][a-z0-9-]{0,30}$/.test(r.exit.trim())) return "选一台出口节点（网里机器的主机名）";
  return null;
}

function Form({ view, onSaved }: { view: MihomoView; onSaved: (v: MihomoView) => void }) {
  const { backend } = useMesh();
  const toast = useToast();
  const [draft, setDraft] = useState<Draft>(toDraft(view.mihomo));
  const [busy, setBusy] = useState(false);
  const next = fromDraft(draft);
  const dirty = !same(next, view.mihomo);
  const errors = draft.routes.map((r) => routeError(r, draft.routes));
  const valid = errors.every((e) => !e);
  const exits = view.exits;
  const registered = (name: string) => exits.some((e) => e.name === name);

  const setRoute = (i: number, p: Partial<Draft["routes"][number]>) =>
    setDraft((d) => ({ ...d, routes: d.routes.map((r, k) => (k === i ? { ...r, ...p } : r)) }));
  const addRoute = () =>
    setDraft((d) => ({ ...d, routes: [...d.routes, { name: `线路 ${d.routes.length + 1}`, exit: exits[0]?.name ?? "", rules: "" }] }));
  const dropRoute = (i: number) => setDraft((d) => ({ ...d, routes: d.routes.filter((_, k) => k !== i) }));
  const toggleOrder = (name: string) =>
    setDraft((d) => ({ ...d, exit_order: d.exit_order.includes(name) ? d.exit_order.filter((n) => n !== name) : [...d.exit_order, name] }));

  const save = async () => {
    setBusy(true);
    try {
      const added = next.routes.filter((r) => !view.mihomo.routes.some((o) => o.name === r.name)).length;
      const v = await backend.saveMihomo(next);
      onSaved(v);
      setDraft(toDraft(v.mihomo));
      toast(
        added
          ? "已保存。新线路要换配置才生效：电脑 6 小时内自动换（马上：sudo lolinet mihomo refresh），手机重新导入"
          : "已保存，各台 mihomo 10 分钟内取到",
        "eye",
      );
    } catch (e) {
      toast(`保存失败：${e instanceof Error ? e.message : e}`, "bad");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack-6">
      <header className="pagehead">
        <div>
          <h1 className="pagehead__title">分流</h1>
          <p className="pagehead__sub">
            装了 mihomo 的电脑和手机按这里走：网内地址走 lolinet，其余依次匹配下面的规则。规则和线路绑哪台，各台 mihomo 10 分钟内经内网取到。
          </p>
        </div>
      </header>

      <div className="grid-pair">
        <Panel className="stack-5">
          <Cap>拒绝</Cap>
          <RuleBox value={draft.reject} onChange={(v) => setDraft({ ...draft, reject: v })} placeholder="DOMAIN-SUFFIX,ads.example.com" />
          <Cap>直连</Cap>
          <RuleBox value={draft.direct} onChange={(v) => setDraft({ ...draft, direct: v })} placeholder="GEOIP,CN" />

          <Cap right={<Button size="sm" icon="plus" onClick={addRoute} disabled={draft.routes.length >= 16}>加一条线路</Button>}>线路</Cap>
          <p className="field__hint">一组规则固定从一台出口节点出去（那台要 sudo lolinet exit on）。节点不在线时这条线路连不通，不会变成直连。</p>
          {draft.routes.length === 0 && <p className="field__hint">还没有线路。比如「办公室」：DOMAIN-SUFFIX,web.com 从 office 出去。</p>}
          {draft.routes.map((r, i) => (
            <div key={i} className="route-card">
              <div className="route-card__head">
                <Field label="线路名" value={r.name} onChange={(e) => setRoute(i, { name: e.target.value })} error={errors[i] ?? undefined} />
                <Button variant="quiet" icon="close" onClick={() => dropRoute(i)}>
                  删除
                </Button>
              </div>
              <div className="formrow">
                <span className="field__label">从哪台出去</span>
                <div className="pick pick--wrap">
                  {exits.map((e) => (
                    <button key={e.name} className={`pick__item ${r.exit === e.name ? "is-on" : ""}`} onClick={() => setRoute(i, { exit: e.name })}>
                      {e.name}
                    </button>
                  ))}
                  {r.exit && !registered(r.exit) && <button className="pick__item is-on">{r.exit}</button>}
                </div>
                {r.exit && !registered(r.exit) ? (
                  <span className="field__error">{r.exit} 还没开出口（在它上面 sudo lolinet exit on），这条线路现在连不通</span>
                ) : (
                  exits.length === 0 && <span className="field__error">还没有出口节点：在要当出口的机器上 sudo lolinet exit on</span>
                )}
              </div>
              <div className="formrow">
                <span className="field__label">命中这些规则的</span>
                <RuleBox value={r.rules} onChange={(v) => setRoute(i, { rules: v })} placeholder="DOMAIN-SUFFIX,web.com" />
              </div>
            </div>
          ))}

          <Cap>其余走出口</Cap>
          <RuleBox value={draft.exit} onChange={(v) => setDraft({ ...draft, exit: v })} placeholder="GEOSITE,openai" />
          <div className="formrow">
            <span className="field__label">出口组的顺序</span>
            <div className="pick pick--wrap">
              {exits.map((e) => {
                const on = draft.exit_order.includes(e.name);
                return (
                  <button key={e.name} className={`pick__item ${on ? "is-on" : ""}`} onClick={() => toggleOrder(e.name)}>
                    {e.name}
                    {on && <span className="pick__ord mono">{draft.exit_order.indexOf(e.name) + 1}</span>}
                  </button>
                );
              })}
            </div>
            <span className="field__hint">这几条规则走「出口」组：默认用排第一的，客户端里可以手动换；没排的在后面，最后是直连</span>
          </div>
          <p className="field__hint">{RULE_HINT}</p>

          <div className="formfoot">
            <Button variant="eye" size="lg" icon="check" disabled={!dirty || !valid || busy} onClick={save}>
              {busy ? "保存中…" : "保存"}
            </Button>
            {dirty && (
              <Button variant="quiet" onClick={() => setDraft(toDraft(view.mihomo))}>
                撤销修改
              </Button>
            )}
            <span className="formfoot__note">{dirty ? "有没保存的修改" : "已保存"}</span>
          </div>
        </Panel>

        <aside className="stack-4">
          <Panel className="stack-3">
            <Cap>匹配顺序</Cap>
            <ol className="order">
              <li>lolinet 自己连接入点的流量 · <b>直连</b></li>
              <li>网内地址、管理面 · <b>lolinet</b></li>
              <li>拒绝 · <b>{next.reject.length} 条</b></li>
              <li>直连 · <b>{next.direct.length} 条</b></li>
              {next.routes.map((r) => (
                <li key={r.name}>
                  线路「{r.name}」· <b>{r.rules.length} 条 → {r.exit || "?"}</b>
                </li>
              ))}
              <li>其余走出口 · <b>{next.exit.length} 条 → 出口组</b></li>
              <li>别的 · <b>直连</b></li>
            </ol>
          </Panel>
          <Panel className="stack-3">
            <Cap count={exits.length}>出口节点</Cap>
            {exits.length === 0 ? (
              <p className="field__hint">在要当出口的机器上 sudo lolinet exit on，它会登记到这里。</p>
            ) : (
              <div className="kvs">
              {exits.map((e: ExitNode) => (
                <KV
                  key={e.name}
                  k={
                    <>
                      <Icon name="gateway" size={14} /> {e.name}
                    </>
                  }
                  mono
                >
                  {e.ip}:{e.port} {e.name === next.exit_order[0] && <Chip tone="quiet">出口组默认</Chip>}
                </KV>
              ))}
              </div>
            )}
          </Panel>
          <Panel className="stack-3">
            <Cap>怎么用上</Cap>
            <p className="preview__note">
              电脑：sudo lolinet mihomo on（Lolidesk.app 带着 mihomo）。手机：lolinet invite --kind phone，在手机浏览器里打开链接、导入 mihomo 客户端。
              改规则、改线路绑哪台不用动客户端；加了新线路要换配置：电脑 6 小时内自动换，马上换就 sudo lolinet mihomo refresh；手机重新导入。
            </p>
          </Panel>
        </aside>
      </div>
    </div>
  );
}
