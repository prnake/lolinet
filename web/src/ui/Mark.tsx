import type { CSSProperties } from "react";
import nodesDoc from "virtual:mesh-nodes";

/**
 * 标志：六辐星号 = 六个接入点，中间那一点是猫眼。
 * state=live 猫眼亮；connecting 呼吸；off 熄灭。
 */
export function Mark({ size = 28, state = "live" }: { size?: number; state?: "live" | "connecting" | "off" }) {
  return (
    <svg className={`mark mark--${state}`} width={size} height={size} viewBox="0 0 32 32" aria-label={nodesDoc.network.title}>
      <g className="mark__arms">
        {[0, 60, 120].map((r) => (
          <rect key={r} x="14" y="3" width="4" height="26" rx="1.4" transform={`rotate(${r} 16 16)`} />
        ))}
      </g>
      <circle className="mark__eye" cx="16" cy="16" r="3.4" />
    </svg>
  );
}

const RELAY_HUES = 6; // --swarm-0..5
const DEVICE_HUES = 4; // --swarm-6..9

function hash(s: string) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

/** 节点身份色序号：relay 按 nodes.yaml 顺序取 0–5，设备按名字哈希取 6–9，两边不撞色。 */
export function swarmIndex(name: string, relayOrder?: number) {
  return relayOrder !== undefined && relayOrder >= 0 ? relayOrder % RELAY_HUES : RELAY_HUES + (hash(name) % DEVICE_HUES);
}

export const swarmVar = (i: number) => `var(--swarm-${i})`;

/** 网关没有自己的身份色（它是宿主 relay 上的一个进程），用半透明墨色。 */
export const GATEWAY_COLOR = "var(--gateway)";

/** 每个节点一只光标（致敬 uwu.cat 的光标群）。tilt 让同屏的光标朝向各不相同。 */
export function Cursor({
  color,
  size = 18,
  tilt,
  name,
  className = "",
  style,
}: {
  color: string;
  size?: number;
  tilt?: number;
  name?: string;
  className?: string;
  style?: CSSProperties;
}) {
  const t = tilt ?? (name ? (hash(name) % 50) - 25 : 0);
  return (
    <svg
      className={`cursor ${className}`}
      width={size}
      height={size}
      viewBox="0 0 20 20"
      style={{ ...style, transform: `rotate(${t}deg)` }}
      aria-hidden="true"
    >
      <path d="M3 2.6l14.6 6.1-6.7 1.9-2 6.8z" fill={color} stroke="var(--cursor-edge)" strokeWidth="1.6" strokeLinejoin="round" />
    </svg>
  );
}
