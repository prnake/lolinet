import type { HTMLAttributes, ReactNode } from "react";
import { Icon, type IconName } from "./icons";

/** 等宽大写标注（uwu.cat 的 CLICK THE ICON）：分组标题、坐标轴、角标。 */
export function Cap({ children, count, className = "", right }: {
  children: ReactNode;
  count?: ReactNode;
  className?: string;
  right?: ReactNode;
}) {
  return (
    <div className={`cap ${className}`}>
      <span>{children}</span>
      {count !== undefined && <span className="cap__count">{count}</span>}
      {right && <span className="cap__right">{right}</span>}
    </div>
  );
}

export function Chip({ children, tone = "plain", mono = true, title }: {
  children: ReactNode;
  tone?: "plain" | "eye" | "warn" | "bad" | "quiet";
  mono?: boolean;
  title?: string;
}) {
  return (
    <span className={`chip chip--${tone} ${mono ? "mono" : ""}`} title={title}>
      {children}
    </span>
  );
}

export type Status = "live" | "relay" | "warn" | "bad" | "idle";

export function StatusDot({ status, pulse }: { status: Status; pulse?: boolean }) {
  return <span className={`dot dot--${status} ${pulse ? "dot--pulse" : ""}`} aria-hidden="true" />;
}

/** 大数字（vibe42 的「120Hz refresh」）：数字 + 小单位 + 等宽标注。 */
export function Stat({ value, unit, label, tone }: { value: ReactNode; unit?: ReactNode; label: ReactNode; tone?: "eye" | "warn" }) {
  return (
    <div className={`stat ${tone ? "stat--" + tone : ""}`}>
      <div className="stat__value">
        {value}
        {unit && <span className="stat__unit">{unit}</span>}
      </div>
      <div className="stat__label">{label}</div>
    </div>
  );
}

export function Panel({ children, className = "", pad = true, ...rest }: { pad?: boolean } & HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={`panel ${pad ? "panel--pad" : ""} ${className}`} {...rest}>
      {children}
    </div>
  );
}

/** magpie 式列表：一行一件事，左边身份，右边取值。 */
export function List({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`list ${className}`}>{children}</div>;
}

export function Row({ lead, title, meta, children, onClick, active, dim }: {
  lead?: ReactNode;
  title: ReactNode;
  meta?: ReactNode;
  children?: ReactNode;
  onClick?: () => void;
  active?: boolean;
  dim?: boolean;
}) {
  const Tag = onClick ? "button" : "div";
  return (
    <Tag className={`row ${onClick ? "row--action" : ""} ${active ? "is-active" : ""} ${dim ? "is-dim" : ""}`} onClick={onClick}>
      {lead && <span className="row__lead">{lead}</span>}
      <span className="row__main">
        <span className="row__title">{title}</span>
        {meta && <span className="row__meta">{meta}</span>}
      </span>
      {children && <span className="row__trail">{children}</span>}
      {onClick && <Icon name="chevron" className="row__chev" size={14} />}
    </Tag>
  );
}

/** 键值行：详情抽屉、设置页。 */
export function KV({ k, children, mono }: { k: ReactNode; children: ReactNode; mono?: boolean }) {
  return (
    <div className="kv">
      <span className="kv__k">{k}</span>
      <span className={`kv__v ${mono ? "mono" : ""}`}>{children}</span>
    </div>
  );
}

export function Empty({ icon = "nodes", title, children }: { icon?: IconName; title: ReactNode; children?: ReactNode }) {
  return (
    <div className="empty">
      <Icon name={icon} size={20} />
      <div className="empty__title">{title}</div>
      {children && <div className="empty__body">{children}</div>}
    </div>
  );
}

/** 细条：剩余有效期、相对延迟。 */
export function Meter({ value, tone = "plain" }: { value: number; tone?: "plain" | "eye" | "warn" | "bad" }) {
  return (
    <span className={`meter meter--${tone}`}>
      <span style={{ width: `${Math.max(2, Math.min(100, value * 100))}%` }} />
    </span>
  );
}

/** 页面大标题：第一行亮，第二行灰（vibe42 的双色标题）。 */
export function Headline({ lead, sub }: { lead: ReactNode; sub?: ReactNode }) {
  return (
    <h1 className="headline">
      <span className="headline__lead">{lead}</span>
      {sub && <span className="headline__sub">{sub}</span>}
    </h1>
  );
}
