import { useEffect, useState } from "react";
import { useLoad, useMesh } from "../data/store";
import type { Settings as S, SettingsView } from "../data/types";
import { Button, Cap, Chip, Cursor, Empty, Field, Icon, KV, Panel, Segmented, swarmVar, useToast } from "../ui";

// 设置只在 manager（web/agent/mesh_manager.py）上有：配对默认值 + 新设备配置里的开关。
// 改了只影响之后配对的设备；已经入网的设备配置在它自己那里，要重新配对才会变。

const same = (a: S, b: S) => JSON.stringify(a) === JSON.stringify(b);
const onOff = [
  { value: "on", label: "开" },
  { value: "off", label: "关" },
] as const;

export function Settings() {
  const { backend } = useMesh();
  const view = useLoad(() => backend.settings(), [backend]);
  if (view.error) return <Empty icon="warn" title="读不到设置">{view.error.message}</Empty>;
  if (!view.data) return null;
  return <Form view={view.data} onSaved={view.setData} />;
}

function Form({ view, onSaved }: { view: SettingsView; onSaved: (v: SettingsView) => void }) {
  const { backend } = useMesh();
  const toast = useToast();
  const [draft, setDraft] = useState<S>(view.settings);
  const [ttl, setTtl] = useState(String(view.settings.pair.ttl_days));
  const [code, setCode] = useState(String(view.settings.pair.code_minutes));
  const [preview, setPreview] = useState(view.preview);
  const [busy, setBusy] = useState(false);

  const ttlN = Number(ttl);
  const codeN = Number(code);
  const ttlErr = Number.isInteger(ttlN) && ttlN >= 1 && ttlN <= 365 ? null : "1–365 天";
  const codeErr = Number.isInteger(codeN) && codeN >= 1 && codeN <= 1440 ? null : "1–1440 分钟";
  const peersErr = draft.pair.peers.length ? null : "至少选一台";
  const next: S = { ...draft, pair: { ...draft.pair, ttl_days: ttlErr ? draft.pair.ttl_days : ttlN, code_minutes: codeErr ? draft.pair.code_minutes : codeN } };
  const valid = !ttlErr && !codeErr && !peersErr;
  const dirty = !same(next, view.settings);
  const isDefault = same(next, view.defaults);

  const setPair = (p: Partial<S["pair"]>) => setDraft((d) => ({ ...d, pair: { ...d.pair, ...p } }));
  const setDevice = (v: Partial<S["device"]>) => setDraft((d) => ({ ...d, device: { ...d.device, ...v } }));

  // 草稿一变就重新渲染一份预览（服务端用真的 relay 地址渲染）
  const key = JSON.stringify(next);
  useEffect(() => {
    if (!valid) return;
    let dead = false;
    const t = setTimeout(() => {
      backend.previewSettings(next).then((c) => !dead && setPreview(c)).catch(() => {});
    }, 250);
    return () => {
      dead = true;
      clearTimeout(t);
    };
  }, [key, valid]); // eslint-disable-line react-hooks/exhaustive-deps

  const save = async () => {
    setBusy(true);
    try {
      const v = await backend.saveSettings(next);
      onSaved(v);
      setDraft(v.settings);
      toast("设置已保存，之后配对的设备按新设置来", "eye");
    } catch (e) {
      toast(`保存失败：${e instanceof Error ? e.message : e}`, "bad");
    } finally {
      setBusy(false);
    }
  };
  const load = (s: S) => {
    setDraft(s);
    setTtl(String(s.pair.ttl_days));
    setCode(String(s.pair.code_minutes));
  };

  return (
    <div className="stack-6">
      <header className="pagehead">
        <div>
          <h1 className="pagehead__title">设置</h1>
          <p className="pagehead__sub">配对新设备时的默认值，和发给新设备的配置。只影响之后配对的设备，已经入网的要重新配对才会变。</p>
        </div>
      </header>

      <div className="grid-pair">
        <Panel className="stack-5">
          <Cap>配对</Cap>
          <div className="formgrid">
            <Field label="凭据有效期（天）" value={ttl} onChange={(e) => setTtl(e.target.value.trim())} error={ttlErr ?? undefined} hint="到期设备掉线，要重新配对" inputMode="numeric" mono />
            <Field
              label="配对码有效期（分钟）"
              value={code}
              onChange={(e) => setCode(e.target.value.trim())}
              error={codeErr ?? undefined}
              hint="只能兑换一次，过期没用的凭据要到凭据页吊销"
              inputMode="numeric"
              mono
            />
          </div>
          <div className="formrow formrow--top">
            <span className="field__label">默认接入点</span>
            <div className="pick pick--wrap">
              {view.relays.map((r, k) => {
                const on = draft.pair.peers.includes(r.name);
                return (
                  <button
                    key={r.name}
                    className={`pick__item ${on ? "is-on" : ""}`}
                    onClick={() => setPair({ peers: on ? draft.pair.peers.filter((p) => p !== r.name) : [...draft.pair.peers, r.name] })}
                    title={r.region ?? undefined}
                  >
                    <Cursor color={swarmVar(k)} size={12} name={r.name} />
                    {r.name}
                    {on && <span className="pick__ord mono">{draft.pair.peers.indexOf(r.name) + 1}</span>}
                  </button>
                );
              })}
            </div>
            <span className={peersErr ? "field__error" : "field__hint"}>
              {peersErr ?? `设备同时连这几台；第一台（${draft.pair.peers[0]}）也是探针「附近」的兜底`}
            </span>
          </div>
          <div className="formrow">
            <span className="field__label">传输</span>
            <Segmented
              value={draft.pair.transport}
              onChange={(v) => setPair({ transport: v })}
              options={[
                { value: "wss", label: "wss 443 · 伪装 SNI" },
                { value: "tcp", label: "tcp · 官方二进制" },
              ]}
            />
            <span className="field__hint">tcp 给没有 SNI 补丁的官方 EasyTier（Docker、路由器固件）；配对页可以单台改</span>
          </div>
          <div className="formrow">
            <span className="field__label">公开探针</span>
            <Segmented value={draft.pair.probe ? "on" : "off"} onChange={(v) => setPair({ probe: v === "on" })} options={[{ value: "on", label: "装" }, { value: "off", label: "不装" }]} />
            <span className="field__hint">准入站首页的设备卡片，只报硬件和大致位置，不报名字和地址</span>
          </div>

          <Cap className="settings__cap">设备配置</Cap>
          <Flag
            label="延迟优先 latency_first"
            on={draft.device.latency_first}
            set={(v) => setDevice({ latency_first: v })}
            hint="按全网延迟表选路，经最近的 relay 中转；关了是最少跳数，有直连就直发"
          />
          <Flag
            label="KCP 代理 enable_kcp_proxy"
            on={draft.device.enable_kcp_proxy}
            set={(v) => setDevice({ enable_kcp_proxy: v })}
            hint="TCP 流包进 KCP/UDP，绕开运营商对出境 TCP 的整形（实测下载 67 → 150 Mbps）"
          />
          <Flag
            label="额外的 udp 连接"
            on={draft.device.udp_peers}
            set={(v) => setDevice({ udp_peers: v })}
            hint="每台接入点再连一条 udp，配合 lolinet 的隧道偏好补丁让内层 UDP 走 UDP；只跟 wss 一起加"
            warn={draft.device.udp_peers ? "relay 换上打了隧道偏好补丁的 lolinet 之后再开" : undefined}
          />

          <div className="formfoot">
            <Button variant="eye" size="lg" icon="check" disabled={!dirty || !valid || busy} onClick={save}>
              {busy ? "保存中…" : "保存"}
            </Button>
            {dirty && (
              <Button variant="quiet" onClick={() => load(view.settings)}>
                撤销修改
              </Button>
            )}
            {!isDefault && (
              <Button variant="quiet" icon="refresh" onClick={() => load(view.defaults)}>
                恢复默认
              </Button>
            )}
            <span className="formfoot__note">{dirty ? "有没保存的修改" : "已保存"}</span>
          </div>
        </Panel>

        <aside className="stack-4">
          <Panel className="stack-3">
            <div className="panel__head panel__head--flush">
              <Cap>新设备会拿到的配置</Cap>
              {dirty && <Chip tone="warn">草稿</Chip>}
            </div>
            <pre className="confpre mono">{preview}</pre>
            <p className="preview__note">凭据私钥由控制台每台单独签，探针密钥由准入站派生；整份配置用配对码加密后才离开这台机器。</p>
          </Panel>
          <Panel className="stack-3">
            <Cap>网络</Cap>
            <div className="kvs">
              <KV k="网络" mono>
                {view.network.name} · {view.network.cidr}
              </KV>
              <KV k="签发节点" mono>
                {view.network.issuer}
              </KV>
              <KV k="准入站" mono>
                {view.base_url.replace(/^https?:\/\//, "")}
              </KV>
              <KV k="官方控制台">
                <a className="mono" href={view.network.console} target="_blank" rel="noreferrer">
                  {view.network.console.replace(/^https?:\/\//, "")} <Icon name="external" size={12} />
                </a>
              </KV>
            </div>
          </Panel>
        </aside>
      </div>
    </div>
  );
}

function Flag({ label, on, set, hint, warn }: { label: string; on: boolean; set: (v: boolean) => void; hint: string; warn?: string }) {
  return (
    <div className="formrow">
      <span className="field__label">{label}</span>
      <Segmented value={on ? "on" : "off"} onChange={(v) => set(v === "on")} options={[...onOff]} />
      <span className={warn ? "field__error" : "field__hint"}>{warn ?? hint}</span>
    </div>
  );
}
