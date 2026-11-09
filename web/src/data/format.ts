export function ms(v: number | null | undefined) {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  return v < 10 ? `${v.toFixed(1)} ms` : `${Math.round(v)} ms`;
}

export function pct(v: number | null | undefined) {
  if (v === null || v === undefined) return "—";
  return v === 0 ? "0%" : `${(v * 100).toFixed(v < 0.1 ? 1 : 0)}%`;
}

export function ago(unix: number, now = Date.now() / 1000) {
  const d = Math.max(0, Math.round(now - unix));
  if (d < 5) return "刚刚";
  if (d < 60) return `${d} 秒前`;
  if (d < 3600) return `${Math.floor(d / 60)} 分钟前`;
  return `${Math.floor(d / 3600)} 小时前`;
}

export function left(unix: number, now = Date.now() / 1000) {
  const d = unix - now;
  if (d <= 0) return "已过期";
  if (d < 3600) return `${Math.ceil(d / 60)} 分钟`;
  if (d < 86400 * 2) return `${Math.round(d / 3600)} 小时`;
  return `${Math.floor(d / 86400)} 天`;
}

export function clock(sec: number) {
  const s = Math.max(0, Math.floor(sec));
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

export const date = (unix: number) =>
  new Date(unix * 1000).toLocaleDateString("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit" });

/** EasyTier 的 NAT 类型名 -> 人话 */
export function natLabel(t?: string) {
  if (!t) return "—";
  const m: Record<string, string> = {
    OpenInternet: "公网直通",
    NoPAT: "无端口转换",
    FullCone: "全锥形",
    Restricted: "受限锥形",
    PortRestricted: "端口受限锥形",
    Symmetric: "对称型",
    SymmetricEasyInc: "对称型（端口递增）",
    SymmetricEasyDec: "对称型（端口递减）",
    SymUdpFirewall: "UDP 防火墙",
    Unknown: "未知",
  };
  return m[t] ?? t;
}
