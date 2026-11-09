import { useState } from "react";
import { nodesDoc } from "../data/model";
import { LAT_BINS } from "../viz/LatencyMatrix";
import {
  Button,
  Cap,
  Chip,
  CodeBoxes,
  CommandPill,
  ConfirmButton,
  Cursor,
  Field,
  Headline,
  KV,
  List,
  Mark,
  Meter,
  Panel,
  Row,
  Segmented,
  Stat,
  StatusDot,
  swarmVar,
  useToast,
} from "../ui";

// 活样张：DESIGN.md 里的每条规则在这里都能看到实物。改 tokens.css 后先来这页过一眼。

const PRINCIPLES = [
  { t: "黑里只亮一点", b: "夜间纯黑、日间纸白。猫眼绿只给三件事：在线、本机、当前焦点和主操作。日间写成字和细线时换成橄榄色。" },
  { t: "一个节点一只光标", b: "每个节点有一只固定颜色的光标，拓扑、列表、凭据、配对预览里都是同一只。颜色就是身份，不表达状态。" },
  { t: "一行一件事", b: "操作界面是列表：左边是谁，中间是什么，右边是取值和动作。分组用等宽大写标注，不用卡片套卡片。" },
  { t: "机器给的字用等宽", b: "IP、配对码、延迟、版本、命令一律 Geist Mono；人写的字用 Geist。命令和码都能一键复制。" },
  { t: "只在变化时动", b: "上线、断开、出码这种状态变化才有动效；常驻的只有光标的轻微漂移。遵守「减少动态效果」。" },
];

const INKS = ["--void", "--ink-1", "--ink-2", "--ink-3", "--ink-4"];
const FGS = ["--fg", "--fg-2", "--fg-3", "--fg-4"];

export function Design() {
  const toast = useToast();
  const [seg, setSeg] = useState("a");
  const [flash, setFlash] = useState(0);
  return (
    <div className="stack-10 design">
      <section className="design__hero">
        <Mark size={56} />
        <Headline lead="猫眼 Catseye" sub="lo-li mesh 的固定美学：黑里只亮一点，纸上只点一点。" />
        <p className="design__lede">
          取 uwu.cat 的虚空、星号和光标群做气质，取 magpie 的一行一件事做操作界面，取 vibe42 的配对码、命令胶囊和三步流程做交互。右上角切日间 / 夜间 / 跟随系统。规范全文在{" "}
          <code>web/DESIGN.md</code>，取值都在 <code>src/styles/tokens.css</code>。
        </p>
      </section>

      <section className="stack-4">
        <Cap>原则</Cap>
        <div className="principles">
          {PRINCIPLES.map((p, i) => (
            <div key={p.t} className="principle">
              <span className="principle__n">{i + 1}</span>
              <div className="principle__t">{p.t}</div>
              <div className="principle__b">{p.b}</div>
            </div>
          ))}
        </div>
      </section>

      <section className="stack-4">
        <Cap>颜色</Cap>
        <div className="swatches">
          {INKS.map((v) => (
            <Swatch key={v} v={v} />
          ))}
        </div>
        <div className="swatches">
          {FGS.map((v) => (
            <Swatch key={v} v={v} light />
          ))}
          <Swatch v="--eye" light note="猫眼：填充" />
          <Swatch v="--eye-fg" light note="猫眼：字和细线" />
          <Swatch v="--warn" light note="警告：快过期、丢包" />
          <Swatch v="--bad" light note="错误、危险操作" />
        </div>
        <div className="swarmrow">
          {Array.from({ length: 10 }, (_, i) => (
            <div key={i} className="swarmrow__item">
              <Cursor color={swarmVar(i)} size={26} tilt={i * 13 - 40} />
              <span className="mono">--swarm-{i}</span>
            </div>
          ))}
          <span className="swarmrow__note">光标群：节点身份色。0–5 按 nodes.yaml 顺序给 relay，6–9 按名字哈希给设备。不用绿色。</span>
        </div>
        <div className="swatches">
          {LAT_BINS.map((b, i) => (
            <div key={b.label} className={`swatch lat-${i}`}>
              <span className="swatch__name mono">{b.label} ms</span>
            </div>
          ))}
          <span className="swarmrow__note">延迟色阶：墨色单色，越快越浓（夜间越亮、日间越深）；格子里永远写数字。绿色不进热力图。</span>
        </div>
      </section>

      <section className="stack-4">
        <Cap>字</Cap>
        <Panel className="typescale">
          <div className="t-display">44 · 配对一台新设备。</div>
          <div className="t-xl">22 · 节点详情标题</div>
          <div className="t-lg">16 · 面板标题、按钮大号</div>
          <div className="t-sm">13 · 操作界面正文：一行一件事，左边是谁，右边是取值。</div>
          <div className="t-mono">12 mono · 10.188.16.10 · wss://203.0.113.14 · 41.49 ms</div>
          <div className="cap">11 mono caps · 等宽大写标注 · CLICK THE ICON</div>
        </Panel>
      </section>

      <section className="stack-4">
        <Cap>控件</Cap>
        <Panel className="stack-5">
          <div className="gallery">
            <Button variant="eye" icon="bolt">生成配对码</Button>
            <Button variant="primary" iconRight="arrow">看节点</Button>
            <Button variant="ghost" icon="refresh">重试</Button>
            <Button variant="quiet" iconRight="chevron">详情</Button>
            <ConfirmButton onConfirm={() => toast("已吊销（样张）", "eye")}>吊销</ConfirmButton>
            <Button variant="ghost" disabled>不可用</Button>
          </div>
          <div className="gallery">
            <Chip>WSS</Chip>
            <Chip>TCP</Chip>
            <Chip tone="eye">本机</Chip>
            <Chip tone="quiet">经 saria</Chip>
            <Chip tone="warn">丢包 8%</Chip>
            <Chip tone="bad">已过期</Chip>
            <span className="gallery__dots">
              <StatusDot status="live" pulse /> 在线
              <StatusDot status="warn" /> 迟滞
              <StatusDot status="bad" /> 断开
              <StatusDot status="idle" /> 离线
            </span>
          </div>
          <div className="gallery">
            <Segmented value={seg} onChange={setSeg} options={[{ value: "a", label: "全部" }, { value: "b", label: "接入点" }, { value: "c", label: "设备" }]} />
            <div style={{ width: 280 }}>
              <Field icon="search" placeholder="筛选名字、IP、城市…" />
            </div>
            <div style={{ width: 200 }}>
              <Field placeholder="10.188.16.11" mono defaultValue="10.188.16.11" />
            </div>
          </div>
          <CommandPill command={`curl -fsSL https://${nodesDoc.network.join}/i | sudo sh -s K7QM-29XF-PA3D`} />
          <CodeBoxes code="K7QM-29XF-PA3D" />
        </Panel>
      </section>

      <section className="stack-4">
        <Cap>列表与数字</Cap>
        <div className="grid-2">
          <List>
            <Row lead={<Cursor color={swarmVar(0)} tilt={-12} />} title="chen" meta={<><span className="mono">10.188.0.11</span><span>北京 · cn-beijing</span></>} onClick={() => {}}>
              <Chip>TCP</Chip>
              <span className="row__lat mono">12 ms</span>
            </Row>
            <Row lead={<Cursor color={swarmVar(3)} tilt={8} />} title="surtr" meta={<><span className="mono">10.188.0.14</span><span>香港 · cn-hongkong</span></>} onClick={() => {}} active>
              <Chip tone="quiet">经 saria</Chip>
              <Chip>WSS</Chip>
              <span className="row__lat mono">62 ms</span>
            </Row>
            <Row lead={<Cursor color={swarmVar(5)} tilt={20} />} title="pad" meta={<span className="mono">10.188.32.1</span>} dim>
              <span className="row__lat mono">离线</span>
            </Row>
          </List>
          <Panel className="stack-5">
            <div className="stats">
              <Stat value={6} unit="/6" label="接入点" tone="eye" />
              <Stat value={12} unit="ms" label="最近接入点" />
              <Stat value={2} label="延迟优先绕路" />
            </div>
            <div className="kvs">
              <KV k="实际延迟" mono>62 ms</KV>
              <KV k="剩余">
                <span className="cred__left"><Meter value={0.8} /> <span className="mono">292 天</span></span>
              </KV>
              <KV k="快过期">
                <span className="cred__left"><Meter value={0.02} tone="warn" /> <span className="mono is-warn">5 天</span></span>
              </KV>
            </div>
          </Panel>
        </div>
      </section>

      <section className="stack-4">
        <Cap>动效</Cap>
        <Panel className="gallery">
          <div key={flash} className={`motion-demo ${flash ? "crt" : ""}`}>
            <Cursor color={swarmVar(2)} size={28} tilt={-20} />
            <span>crt-flash：节点上线</span>
          </div>
          <Button variant="ghost" onClick={() => setFlash((f) => f + 1)}>播放</Button>
          <div className="motion-demo">
            <span className="drift-demo"><Cursor color={swarmVar(4)} size={22} tilt={14} /></span>
            <span>drift：光标常驻漂移</span>
          </div>
          <div className="motion-demo">
            <Mark size={28} state="connecting" />
            <span>breathe：连接中</span>
          </div>
          <Button variant="primary" onClick={() => toast("已复制", "eye")}>toast</Button>
        </Panel>
      </section>
    </div>
  );
}

function Swatch({ v, light, note }: { v: string; light?: boolean; note?: string }) {
  return (
    <div className={`swatch ${light ? "swatch--light" : ""}`} style={{ background: `var(${v})` }}>
      <span className="swatch__name mono">{v}</span>
      {note && <span className="swatch__note">{note}</span>}
    </div>
  );
}
