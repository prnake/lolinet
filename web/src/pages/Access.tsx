import { useState, type FormEvent } from "react";
import { ApiError } from "../data/api";
import { date, left } from "../data/format";
import { nodesDoc } from "../data/model";
import { useLoad, useMesh } from "../data/store";
import type { Credential, RegisteredDevice } from "../data/types";
import { go } from "../router";
import { Button, Cap, Chip, ConfirmButton, Cursor, Empty, Field, List, Mark, Meter, Panel, Row, StatusDot, swarmIndex, swarmVar, useToast } from "../ui";

export function Access() {
  const { backend, info } = useMesh();
  const auth = useLoad(() => backend.consoleLoggedIn(), [backend]);
  if (auth.loading && auth.data === null) return <Loading />;
  if (auth.error) return <ConsoleDown error={auth.error} />;
  if (!auth.data) return <Login onDone={auth.reload} />;
  return <Credentials networkHint={info?.network_id ?? null} />;
}

function Credentials({ networkHint }: { networkHint: string | null }) {
  const { backend, info } = useMesh();
  const toast = useToast();
  const [busy, setBusy] = useState<string | null>(null);
  const nets = useLoad(() => backend.networks(), [backend]);
  const net = nets.data?.find((n) => n.network_id === networkHint) ?? nets.data?.find((n) => n.network_name === nodesDoc.network.name);
  const creds = useLoad(() => (net ? backend.credentials(net.network_id) : Promise.resolve([] as Credential[])), [backend, net?.network_id]);
  const devs = useLoad(() => (info?.registry ? backend.devices() : Promise.resolve([] as RegisteredDevice[])), [backend, info?.registry]);

  const revoke = async (c: Credential, dev?: RegisteredDevice) => {
    if (!net) return;
    setBusy(c.credential_id);
    try {
      await backend.revoke(net.network_id, c.credential_id);
      if (dev) await backend.forget(c.credential_id).catch(() => {});
      toast(`已吊销 ${dev?.name ?? c.credential_id}，设备会立刻掉线`, "eye");
      creds.reload();
      devs.reload();
    } catch (e) {
      toast(`吊销失败：${e instanceof Error ? e.message : e}`, "bad");
    } finally {
      setBusy(null);
    }
  };

  const list = [...(creds.data ?? [])].sort((a, b) => b.online_peers.length - a.online_peers.length || a.expiry_unix - b.expiry_unix);

  return (
    <div className="stack-6">
      <header className="pagehead">
        <div>
          <h1 className="pagehead__title">凭据</h1>
          <p className="pagehead__sub">控制台签发的设备凭据，一台设备一条、不可复用。吊销后设备立刻掉线。</p>
        </div>
        <div className="pagehead__tools">
          <Button variant="eye" icon="plus" onClick={() => go("pair")}>
            配对新设备
          </Button>
        </div>
      </header>

      {net && (
        <div className="netline">
          <span className="netline__name">
            <Mark size={16} />
            {net.display_name}
          </span>
          <Chip>{net.networking_method}</Chip>
          <Chip>{net.virtual_cidr}</Chip>
          {net.secure_mode && <Chip>secure mode</Chip>}
          <span className="netline__right mono">
            {list.length} 条凭据 · {list.filter((c) => c.online_peers.length).length} 台在线
          </span>
        </div>
      )}

      {nets.error && <Empty icon="warn" title="读不到控制台网络">{nets.error.message}</Empty>}
      {net && !creds.loading && list.length === 0 && (
        <Empty icon="key" title="控制台里还没有凭据">
          配对新设备时会在这里签一条。用 easytier-cli 直接在 relay 上签的凭据不在控制台里。
        </Empty>
      )}

      {list.length > 0 && (
        <section className="stack-2">
          <Cap count={list.length}>凭据</Cap>
          <List>
            {list.map((c) => {
              const dev = devs.data?.find((d) => d.credential_id === c.credential_id);
              const online = c.online_peers[0];
              const name = dev?.name ?? online?.hostname ?? c.credential_id.replace(/-\d{14}$/, "");
              const remain = c.expiry_unix - Date.now() / 1000;
              const tone = remain < 7 * 86400 ? "warn" : "plain";
              return (
                <Row
                  key={c.credential_id}
                  dim={!online}
                  lead={<Cursor color={swarmVar(swarmIndex(name))} name={name} />}
                  title={
                    <>
                      {name}
                      {c.reusable && <Chip tone="warn">可复用</Chip>}
                    </>
                  }
                  meta={
                    <>
                      <span className="mono">{c.credential_id}</span>
                      {dev && <span className="mono">{dev.overlay_ip}</span>}
                      {dev && <span>配对于 {dev.paired_at.slice(0, 10)}</span>}
                    </>
                  }
                >
                  <span className="cred__state">
                    <StatusDot status={online ? "live" : "idle"} pulse={!!online} />
                    {online ? <span className="mono">{online.ipv4 ?? "在线"}</span> : "离线"}
                  </span>
                  <span className="cred__left" title={`到期 ${date(c.expiry_unix)}`}>
                    <Meter value={Math.max(0, remain) / (365 * 86400)} tone={tone} />
                    <span className={`mono ${tone === "warn" ? "is-warn" : ""}`}>{left(c.expiry_unix)}</span>
                  </span>
                  <ConfirmButton
                    busy={busy === c.credential_id}
                    disabled={info?.read_only}
                    title={info?.read_only ? "只读模式：在 agent 那台机器上用 127.0.0.1 打开才能吊销" : undefined}
                    onConfirm={() => revoke(c, dev)}
                  >
                    吊销
                  </ConfirmButton>
                </Row>
              );
            })}
          </List>
        </section>
      )}
    </div>
  );
}

function Loading() {
  return (
    <div className="center-fill">
      <Mark size={40} state="connecting" />
    </div>
  );
}

function ConsoleDown({ error }: { error: Error }) {
  return (
    <Empty icon="warn" title="连不上控制台">
      {error.message}
      <br />
      控制台只在 overlay 上（{nodesDoc.network.console.replace("https://", "")}）。本机用 no-tun 内核时，agent 要带 --socks。
    </Empty>
  );
}

/** 浏览器自己登录（agent 读不到 state/mesh-console.json 时）。 */
function Login({ onDone }: { onDone: () => void }) {
  const { backend } = useMesh();
  const [user, setUser] = useState("admin");
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(null);
    try {
      await backend.login(user, pw);
      onDone();
    } catch (x) {
      setErr(x instanceof ApiError && x.status === 401 ? "用户名或密码不对" : String(x instanceof Error ? x.message : x));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="center-fill">
      <Panel className="login">
        <form onSubmit={submit} className="stack-4">
          <Mark size={36} state="off" />
          <div>
            <div className="login__title">登录控制台</div>
            <div className="login__sub">easytier-web · {nodesDoc.network.console.replace("https://", "")}</div>
          </div>
          <Field label="用户名" value={user} onChange={(e) => setUser(e.target.value)} mono />
          <Field label="密码" type="password" value={pw} onChange={(e) => setPw(e.target.value)} error={err ?? undefined} autoFocus />
          <Button variant="primary" size="lg" disabled={busy || !pw}>
            {busy ? "登录中…" : "登录"}
          </Button>
        </form>
      </Panel>
    </div>
  );
}
