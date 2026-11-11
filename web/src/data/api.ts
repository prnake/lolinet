import { md5 } from "./md5";
import type { AgentInfo, Credential, MihomoRules, MihomoView, NetworkSummary, PairResult, RegisteredDevice, Settings, SettingsView, Snapshot } from "./types";

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

// probe：配对码里带公开探针配置，install.sh 一起装（默认 true；false 时 pair.py --no-probe）
// transport / code_minutes 只有 manager 认（本机 agent 转调 pair.py，用它的默认值）
export type PairRequest = {
  name: string;
  ip: string;
  ttl_days: number;
  peers: string[];
  probe?: boolean;
  transport?: "wss" | "tcp";
  code_minutes?: number;
};

/** 页面只认这个接口；真 agent 和演示数据各实现一份。 */
export interface Backend {
  demo: boolean;
  info(): Promise<AgentInfo>;
  snapshot(): Promise<Snapshot>;
  devices(): Promise<RegisteredDevice[]>;
  pair(req: PairRequest): Promise<PairResult>;
  forget(credentialId: string): Promise<void>;
  consoleLoggedIn(): Promise<boolean>;
  login(username: string, password: string): Promise<void>;
  networks(): Promise<NetworkSummary[]>;
  credentials(networkId: string): Promise<Credential[]>;
  revoke(networkId: string, credentialId: string): Promise<void>;
  /** 以下只有 manager 有 */
  logout(): Promise<void>;
  settings(): Promise<SettingsView>;
  saveSettings(s: Settings): Promise<SettingsView>;
  previewSettings(s: Settings): Promise<string>;
  /** 分流规则和登记过的出口节点 */
  mihomo(): Promise<MihomoView>;
  saveMihomo(m: MihomoRules): Promise<MihomoView>;
}

async function call<T>(path: string, init: RequestInit = {}): Promise<T> {
  // 写操作必须带 X-Mesh-Agent（agent 的 CSRF 防线），读也带上，省得区分
  const headers: Record<string, string> = { "X-Mesh-Agent": "1" };
  if (init.body) headers["Content-Type"] = "application/json";
  const r = await fetch(path, { ...init, headers, credentials: "same-origin" });
  const text = await r.text();
  let data: unknown;
  try {
    data = text ? JSON.parse(text) : undefined;
  } catch {
    data = undefined;
  }
  if (!r.ok) {
    const msg = (data as { message?: string } | undefined)?.message ?? (text.slice(0, 200) || r.statusText);
    throw new ApiError(r.status, msg);
  }
  return data as T;
}

const json = (body: unknown): RequestInit => ({ method: "POST", body: JSON.stringify(body) });

export const liveBackend: Backend = {
  demo: false,
  info: () => call("/api/mesh/info"),
  snapshot: () => call("/api/mesh/snapshot"),
  devices: async () => (await call<{ devices: RegisteredDevice[] }>("/api/mesh/devices")).devices,
  pair: (req) => call("/api/mesh/pair", json(req)),
  forget: async (credential_id) => void (await call("/api/mesh/forget", json({ credential_id }))),
  consoleLoggedIn: async () => {
    try {
      await call("/api/v1/auth/check_login_status");
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) return false;
      throw e;
    }
  },
  login: async (username, password) => void (await call("/api/v1/auth/login", json({ username, password: md5(password) }))),
  networks: async () => (await call<{ networks: NetworkSummary[] }>("/api/v1/networks")).networks,
  credentials: async (nid) => (await call<{ credentials: Credential[] }>(`/api/v1/networks/${nid}/credentials`)).credentials,
  revoke: async (nid, cid) =>
    void (await call(`/api/v1/networks/${nid}/credentials/${encodeURIComponent(cid)}`, { method: "DELETE" })),
  logout: async () => void (await call("/api/mesh/logout", json({}))),
  settings: () => call("/api/mesh/settings"),
  saveSettings: (s) => call("/api/mesh/settings", json(s)),
  previewSettings: async (s) => (await call<{ config: string }>("/api/mesh/settings/preview", json({ settings: s }))).config,
  mihomo: () => call("/api/mesh/mihomo"),
  saveMihomo: (m) => call("/api/mesh/mihomo", json(m)),
};
