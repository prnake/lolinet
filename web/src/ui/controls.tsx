import { useEffect, useRef, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "./icons";

type Variant = "primary" | "eye" | "ghost" | "quiet" | "danger";

export function Button({
  variant = "ghost",
  size = "md",
  icon,
  iconRight,
  children,
  className = "",
  ...rest
}: {
  variant?: Variant;
  size?: "sm" | "md" | "lg";
  icon?: IconName;
  iconRight?: IconName;
} & ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button className={`btn btn--${variant} btn--${size} ${className}`} {...rest}>
      {icon && <Icon name={icon} size={size === "lg" ? 18 : 14} />}
      {children}
      {iconRight && <Icon name={iconRight} size={size === "lg" ? 18 : 14} />}
    </button>
  );
}

/** 危险操作点两次：第一次变红「再点一次」，3 秒内不点就复原。 */
export function ConfirmButton({ onConfirm, children, armedLabel = "再点一次确认", busy, disabled, title }: {
  onConfirm: () => void;
  children: ReactNode;
  armedLabel?: string;
  busy?: boolean;
  disabled?: boolean;
  title?: string;
}) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button
      className={`btn btn--danger btn--sm ${armed ? "is-armed" : ""}`}
      disabled={busy || disabled}
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        if (armed) {
          setArmed(false);
          onConfirm();
        } else setArmed(true);
      }}
    >
      {busy ? "处理中…" : armed ? armedLabel : children}
    </button>
  );
}

export function Segmented<T extends string>({ value, options, onChange, size = "md" }: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (v: T) => void;
  size?: "sm" | "md";
}) {
  return (
    <div className={`seg seg--${size}`} role="tablist">
      {options.map((o) => (
        <button
          key={o.value}
          role="tab"
          aria-selected={o.value === value}
          className={`seg__item ${o.value === value ? "is-on" : ""}`}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Field({ label, hint, error, icon, mono, ...rest }: {
  label?: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  icon?: IconName;
  mono?: boolean;
} & InputHTMLAttributes<HTMLInputElement>) {
  return (
    <label className={`field ${error ? "has-error" : ""}`}>
      {label && <span className="field__label">{label}</span>}
      <span className="field__box">
        {icon && <Icon name={icon} className="field__icon" />}
        <input className={`field__input ${mono ? "mono" : ""}`} spellCheck={false} autoComplete="off" {...rest} />
      </span>
      {(error || hint) && <span className={error ? "field__error" : "field__hint"}>{error || hint}</span>}
    </label>
  );
}

export function useCopy() {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1600);
  };
  return { copied, copy };
}

/** 命令胶囊（参照 vibe42 的 Copy prompt）：图标 + 等宽命令 + 竖线 + 复制。 */
export function CommandPill({ command, icon = "terminal", label = "复制", size = "md" }: {
  command: string;
  icon?: IconName;
  label?: string;
  size?: "md" | "lg";
}) {
  const { copied, copy } = useCopy();
  return (
    <div className={`cmd cmd--${size}`}>
      <Icon name={icon} className="cmd__icon" />
      <code className="cmd__text" title={command}>{command}</code>
      <span className="cmd__sep" />
      <button className={`cmd__copy ${copied ? "is-done" : ""}`} onClick={() => copy(command)}>
        <Icon name={copied ? "check" : "copy"} />
        {copied ? "已复制" : label}
      </button>
    </div>
  );
}

export function CopyText({ text, children }: { text: string; children?: ReactNode }) {
  const { copied, copy } = useCopy();
  return (
    <button className={`copytext ${copied ? "is-done" : ""}`} onClick={() => copy(text)} title="复制">
      <span>{children ?? text}</span>
      <Icon name={copied ? "check" : "copy"} size={12} />
    </button>
  );
}

const GLYPHS = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/** 配对码格子：生成时逐格「解码」，之后静止。code 形如 ABCD-EFGH-JKMN，每 4 位一个短横。 */
export function CodeBoxes({ code, dim }: { code: string; dim?: boolean }) {
  const flat = code.replace(/-/g, "");
  const [shown, setShown] = useState(flat);
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setShown(flat);
      return;
    }
    let frame = 0;
    const id = setInterval(() => {
      frame++;
      setShown(
        [...flat]
          .map((c, i) => (frame > 4 + i * 1.6 ? c : GLYPHS[(Math.random() * GLYPHS.length) | 0]))
          .join(""),
      );
      if (frame > 6 + flat.length * 1.6) clearInterval(id);
    }, 38);
    return () => clearInterval(id);
  }, [flat]);
  return (
    <div className={`code ${dim ? "is-dim" : ""}`} aria-label={code}>
      {[...flat].map((_, i) => [
        i > 0 && i % 4 === 0 && <span className="code__dash" key={`d${i}`}>–</span>,
        <span className="code__box" key={i}>
          {shown[i] ?? ""}
        </span>,
      ])}
    </div>
  );
}
