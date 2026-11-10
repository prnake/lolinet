import { useEffect, useMemo, useRef, useState } from "react";
import { clock, ms } from "../data/format";
import { nextFreeIp, nodesDoc } from "../data/model";
import { useLoad, useMesh, useNow } from "../data/store";
import type { PairResult, RegisteredDevice, Settings } from "../data/types";
import { go } from "../router";
import { Button, Cap, Chip, CodeBoxes, CommandPill, Cursor, Field, Headline, Icon, Mark, Panel, Segmented, swarmIndex, swarmVar, useToast } from "../ui";

// 和 scripts/device_config.py 一致：network.device_peers，没写就取前三台
const DEFAULT_PEERS = nodesDoc.network.device_peers ?? nodesDoc.relays.slice(0, 3).map((r) => r.name);
const NAME_RE = /^[a-z0-9][a-z0-9-]{0,30}$/;
const IP_RE = /^10\.188\.(\d{1,3})\.(\d{1,3})$/;

// 本机 agent（转调 pair.py）的默认值；manager 上用设置页存的
const FALLBACK: Settings["pair"] = { ttl_days: 365, code_minutes: 15, peers: DEFAULT_PEERS, transport: "wss", probe: true };

const minutesText = (m: number) => (m % 60 === 0 ? `${m / 60} 小时` : `${m} 分钟`);

export function Pair() {
  const { backend, info } = useMesh();
  const sv = useLoad(() => (info?.manager ? backend.settings().then((v) => v.settings.pair) : Promise.resolve(null)), [backend, info?.manager]);
  if (info?.manager && sv.loading && !sv.data) return null;
  return <PairFlow defaults={sv.data ?? FALLBACK} />;
}

function PairFlow({ defaults }: { defaults: Settings["pair"] }) {
  const { mesh, backend, info } = useMesh();
  const toast = useToast();
  const devs = useLoad(() => (info?.registry ? backend.devices() : Promise.resolve([] as RegisteredDevice[])), [backend, info?.registry]);
  const [name, setName] = useState("");
  const [pool, setPool] = useState<"tun" | "app">("tun");
  const [ipEdit, setIpEdit] = useState<string | null>(null);
  const [ttl, setTtl] = useState(String(defaults.ttl_days));
  const [peers, setPeers] = useState<string[]>(defaults.peers);
  const [probe, setProbe] = useState<"on" | "off">(defaults.probe ? "on" : "off");
  const [transport, setTransport] = useState<"wss" | "tcp">(defaults.transport);
  const ttlOptions = [...new Set([30, 90, 365, defaults.ttl_days])].sort((a, b) => a - b).map((d) => ({ value: String(d), label: `${d} 天` }));
  const STEPS = [
    { title: "填设备", body: "名字、地址、有效期。" },
    { title: "在设备上执行", body: `一行命令，配对码 ${minutesText(defaults.code_minutes)}内有效、只能用一次。` },
    { title: "上线", body: "设备自己打洞，接到最近的接入点。" },
  ];
  const [result, setResult] = useState<PairResult | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useNow();

  const used = useMemo(
    () => [
      ...(mesh?.nodes.map((n) => n.ipv4 ?? "") ?? []),
      ...(devs.data?.map((d) => d.overlay_ip) ?? []),
      ...nodesDoc.devices.map((d) => d.overlay_ip),
    ],
    [mesh, devs.data],
  );
  const taken = new Set([...(mesh?.nodes.map((n) => n.hostname) ?? []), ...(devs.data?.map((d) => d.name) ?? [])]);
  const ip = ipEdit ?? nextFreeIp(pool, used);
  const m = IP_RE.exec(ip);
  const nameErr = name && !NAME_RE.test(name) ? "小写字母、数字和 -，32 个字符以内" : taken.has(name) ? "已经有同名节点" : null;
  const ipErr = !m || +m[1] > 255 || +m[2] > 254 || +m[2] < 1 ? "要在 10.188.0.0/16 里" : used.includes(ip) ? "这个地址已经有人用了" : null;
  const ready = !!name && !nameErr && !ipErr && peers.length > 0 && !!info?.pair;

  const online = result ? mesh?.nodes.find((n) => n.hostname === result.name && n.ipv4 === result.ip) : undefined;
  const remain = result ? result.expires_at - now / 1000 : 0;
  const step = !result ? 1 : online ? 3 : 2;

  const submit = async () => {
    setBusy(true);
    try {
      setResult(await backend.pair({ name, ip, ttl_days: Number(ttl), peers, probe: probe === "on", transport, code_minutes: defaults.code_minutes }));
    } catch (e) {
      toast(`生成失败：${e instanceof Error ? e.message : e}`, "bad");
    } finally {
      setBusy(false);
    }
  };
  const reset = () => {
    setResult(null);
    setName("");
    setIpEdit(null);
    devs.reload();
  };

  return (
    <div className="stack-8">
      <Headline lead="配对一台新设备。" sub="一行命令、一个配对码，设备就入网。" />

      <ol className="steps">
        {STEPS.map((s, i) => (
          <li key={s.title} className={`steps__item ${step === i + 1 ? "is-on" : step > i + 1 ? "is-done" : ""}`}>
            <span className="steps__num">{i + 1}</span>
            <span className="steps__text">
              <span className="steps__title">
                {s.title}
                {step > i + 1 && <Icon name="check" size={14} />}
              </span>
              <span className="steps__body">{s.body}</span>
            </span>
          </li>
        ))}
      </ol>

      {step === 1 && (
        <div className="grid-pair">
          <Panel className="stack-5">
            <Field
              label="设备名"
              placeholder="mbp-air"
              value={name}
              onChange={(e) => setName(e.target.value.toLowerCase())}
              error={nameErr ?? undefined}
              hint={`内网名 ${name || "<名字>"}${nodesDoc.network.local_suffix}`}
              mono
              autoFocus
            />
            <div className="formrow">
              <span className="field__label">类型</span>
              <Segmented
                value={pool}
                onChange={(v) => {
                  setPool(v);
                  setIpEdit(null);
                }}
                options={[
                  { value: "tun", label: "电脑 · TUN" },
                  { value: "app", label: "App · 无 TUN" },
                ]}
              />
            </div>
            <Field
              label="地址"
              value={ip}
              onChange={(e) => setIpEdit(e.target.value)}
              error={ipErr ?? undefined}
              hint={pool === "tun" ? "人用电脑段 10.188.16.0/20，自动取下一个空位" : "无 TUN App 段 10.188.32.0/20，自动取下一个空位"}
              mono
            />
            <div className="formrow">
              <span className="field__label">有效期</span>
              <Segmented value={ttl} onChange={setTtl} options={ttlOptions} />
            </div>
            <div className="formrow formrow--top">
              <span className="field__label">接入点</span>
              <div className="pick pick--wrap">
                {nodesDoc.relays.map((r, k) => {
                  const on = peers.includes(r.name);
                  return (
                    <button
                      key={r.name}
                      className={`pick__item ${on ? "is-on" : ""}`}
                      onClick={() => setPeers(on ? peers.filter((p) => p !== r.name) : [...peers, r.name])}
                    >
                      <Cursor color={swarmVar(k)} size={12} name={r.name} />
                      {r.name}
                    </button>
                  );
                })}
              </div>
            </div>
            <div className="formrow">
              <span className="field__label">传输</span>
              <Segmented
                value={transport}
                onChange={setTransport}
                options={[
                  { value: "wss", label: "wss · lolinet" },
                  { value: "tcp", label: "tcp · 官方 EasyTier" },
                ]}
              />
            </div>
            <div className="formrow">
              <span className="field__label">公开探针</span>
              <Segmented
                value={probe}
                onChange={setProbe}
                options={[
                  { value: "on", label: "装" },
                  { value: "off", label: "不装" },
                ]}
              />
            </div>
            <div className="formfoot">
              <Button variant="eye" size="lg" icon="bolt" disabled={!ready || busy} onClick={submit}>
                {busy ? "签发中…" : "生成配对码"}
              </Button>
              <span className="formfoot__note">
                在控制台签一条不可复用的凭据，{ttl} 天后过期；配对码 {minutesText(defaults.code_minutes)}内有效。
                {probe === "on" ? "安装时一起装公开探针（只报硬件和大致位置）。" : "不装公开探针。"}
              </span>
            </div>
          </Panel>
          <aside className="stack-4">
            {!info?.pair && (
              <Panel className="notice stack-3">
                <div className="notice__title">
                  <Icon name="warn" size={14} />
                  {info?.read_only ? "只读模式，不能在这里签凭据" : "这个 agent 没开配对"}
                </div>
                <p>
                  {info?.read_only ? (
                    <>现在是从局域网打开的，agent 不接受写操作。到 agent 那台机器上用 127.0.0.1 打开，或者直接在终端里签：</>
                  ) : (
                    <>
                      启动 agent 时加 <code>--allow-pair</code>（要能读到主仓库 .env 和 state）。或者直接在终端里签：
                    </>
                  )}
                </p>
                <CommandPill
                  command={`uv run scripts/pair.py new --name ${name || "<名字>"} --ip ${ip} --ttl-days ${ttl}${transport === "tcp" ? " --transport tcp" : ""}${probe === "off" ? " --no-probe" : ""}${info?.socks ? ` --socks ${info.socks}` : ""}`}
                />
              </Panel>
            )}
            <Preview name={name} ip={ip} />
          </aside>
        </div>
      )}

      {result && (
        <div className="grid-pair">
          <Panel className="stack-5 codepanel">
            <div className="panel__head panel__head--flush">
              <Cap>配对码</Cap>
              {!online && (
                <Chip tone={remain < 120 ? "warn" : "plain"}>{remain > 0 ? `${clock(remain)} 后失效` : "已过期"}</Chip>
              )}
            </div>
            <CodeBoxes code={result.code} dim={remain <= 0 && !online} />
            <CommandPill command={result.command} size="lg" />
            <ul className="notes">
              <li>macOS / Linux 都是这一行；要 sudo，装完注册成系统服务。</li>
              <li>join Worker 只存加密后的配置，解密在设备上做；兑换一次就删。</li>
              <li>
                凭据 <span className="mono">{result.credential_id ?? "—"}</span>，地址 <span className="mono">{result.ip}</span>
              </li>
            </ul>
            <div>
              <Button variant="quiet" size="sm" icon="refresh" onClick={reset}>
                重新开始
              </Button>
            </div>
          </Panel>
          <Waiting result={result} online={online ? { via: online.via, latency: online.latency, id: online.id } : null} expired={remain <= 0} onAgain={reset} />
        </div>
      )}
    </div>
  );
}

function Preview({ name, ip }: { name: string; ip: string }) {
  const n = name || "新设备";
  return (
    <Panel className="preview">
      <Cap>预览</Cap>
      <div className="preview__row">
        <Cursor color={swarmVar(swarmIndex(n))} size={22} name={n} />
        <div>
          <div className="preview__name">{n}</div>
          <div className="preview__ip mono">{ip}</div>
        </div>
      </div>
      <p className="preview__note">每个节点一只光标，颜色由名字决定，拓扑图、列表、凭据里都是这一只。</p>
    </Panel>
  );
}

function Waiting({ result, online, expired, onAgain }: {
  result: PairResult;
  online: { via: string | null; latency: number | null; id: string } | null;
  expired: boolean;
  onAgain: () => void;
}) {
  const start = useRef(Date.now());
  const now = useNow();
  const [flash, setFlash] = useState(false);
  useEffect(() => {
    if (online) setFlash(true);
  }, [!!online]); // eslint-disable-line react-hooks/exhaustive-deps

  if (online)
    return (
      <Panel className={`waiting is-online ${flash ? "crt" : ""}`}>
        <div className="waiting__hero">
          <Cursor color={swarmVar(swarmIndex(result.name))} size={40} name={result.name} />
        </div>
        <div className="waiting__title">{result.name} 已上线</div>
        <div className="waiting__sub mono">
          {result.ip} · {online.via && online.via !== "DIRECT" ? `经 ${online.via} · ` : "直连 · "}
          {ms(online.latency)}
        </div>
        <div className="waiting__actions">
          <Button variant="primary" iconRight="arrow" onClick={() => go(`nodes/${online.id}`)}>
            看节点
          </Button>
          <Button variant="ghost" icon="plus" onClick={onAgain}>
            再配一台
          </Button>
        </div>
      </Panel>
    );
  return (
    <Panel className="waiting">
      <div className="waiting__hero">
        <span className="radar" />
        <Mark size={44} state={expired ? "off" : "connecting"} />
      </div>
      <div className="waiting__title">{expired ? "配对码过期了" : `等 ${result.name} 上线`}</div>
      <div className="waiting__sub mono">
        {expired ? "凭据已签，可以在凭据页吊销后重来" : `已等 ${clock((now - start.current) / 1000)} · 每 3 秒看一次路由表`}
      </div>
      {expired && (
        <div className="waiting__actions">
          <Button variant="ghost" icon="refresh" onClick={onAgain}>
            重新生成
          </Button>
        </div>
      )}
    </Panel>
  );
}
