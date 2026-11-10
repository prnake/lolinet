// 准入站 /v1/probe 的结构（join/worker.js ProbeHub.snapshot）。公开数据里没有任何名字和地址：
// relay 的 id 是区域代码（页面上写城市），设备的 id 是 d-<十六进制>，设备只有硬件和位置。

export type Pair = [number | null, number | null];

export type Host = {
  os: string | null;
  cpu_model?: string | null;
  cores: number | null;
  /** 0–100，过去 30 秒平均 */
  cpu: number | null;
  /** 秒 */
  uptime: number | null;
  load: (number | null)[] | null;
  /** [已用, 总量]，字节 */
  mem: Pair | null;
  swap: Pair | null;
  disk: Pair | null;
  /** rx / tx：字节每秒；*_total：开机以来字节数 */
  net: { rx: number | null; tx: number | null; rx_total: number | null; tx_total: number | null };
};

/** peer 是对端 relay 的区域代码 */
export type ProbeLink = { peer: string; direct: boolean; lat: number | null; loss: number | null; proto: string | null };

export type ProbeRelay = {
  /** 区域代码，比如 cn-hongkong */
  id: string;
  region: string;
  /** 实例规格，比如「阿里云 ecs.c9i.xlarge」 */
  spec: string | null;
  online: boolean;
  last_seen: number | null;
  host: Host | null;
  et: { ok: boolean; version: string | null; devices: number | null; links: ProbeLink[] } | null;
  /** 过去 30 天（有数据的部分）的可用率 */
  uptime30: number | null;
  /** 30 天，每天一格；up = null 表示那天还没开始记录 */
  days: { day: number; up: number | null }[];
  /** 24 小时，每 30 分钟一点 */
  series: { t: number; cpu: number; rx: number; tx: number }[];
};

/**
 * 网里的设备：relay 看到的（匿名 id + 直连了哪些接入点）并上设备自己报的硬件。
 * 没装探针的设备 hardware = false，硬件字段都是 null。
 */
export type ProbeDevice = {
  id: string;
  kind: string | null;
  model: string | null;
  chip: string | null;
  cores: number | null;
  /** 字节 */
  mem: number | null;
  os: string | null;
  arch: string | null;
  disks: { label: string | null; total: number | null }[];
  /** 配置写死的位置（城市名）；没写时看 near_region */
  location: string | null;
  /** 离它最近的接入点所在区域（在网里时是实测的） */
  near_region: string | null;
  /** 它直连的接入点，按延迟从低到高 */
  links: { region: string; lat: number }[];
  /** online：relay 看得到它；standby：探针在报但没连进网；offline：都没有 */
  status: "online" | "standby" | "offline";
  online: boolean;
  /** 装了探针、报了硬件 */
  hardware: boolean;
  last_seen: number | null;
};

export type ProbeSnapshot = {
  now: number;
  interval: number;
  relays: ProbeRelay[];
  /** 在网的设备数 */
  peers: number;
  devices: ProbeDevice[];
};

export type LinkHistory = { a: string; b: string; ab: { t: number; lat: number }[]; ba: { t: number; lat: number }[] };
