// agent / easytier-cli / easytier-web 的原样结构。cli 的数字很多是字符串（"41.49"、"0.0%"、"3.61 kB"）。

export type CliPeer = {
  cidr: string;
  ipv4: string;
  hostname: string;
  cost: string; // "Local" | "p2p" | "relay(2)"
  lat_ms: string;
  loss_rate: string;
  rx_bytes: string;
  tx_bytes: string;
  tunnel_proto: string;
  nat_type: string;
  id: string;
  version: string;
};

export type CliRoute = {
  ipv4: string;
  hostname: string;
  proxy_cidrs: string;
  next_hop_ipv4: string;
  next_hop_hostname: string;
  next_hop_lat: number;
  path_len: number;
  path_latency: number;
  next_hop_ipv4_lat_first: string;
  next_hop_hostname_lat_first: string;
  path_len_lat_first: number;
  path_latency_lat_first: number;
  version: string;
};

export type CenterNode = {
  node_id: string;
  hostname: string;
  ipv4: string;
  direct_peers: { node_id: string; hostname: string; ipv4: string; latency_ms: number }[];
};

export type SelfNode = {
  peer_id: string;
  hostname: string;
  ipv4_addr: string;
  version: string;
  udp_nat_type?: string;
  tcp_nat_type?: string;
  listeners: string[];
  peer_uris: string[];
  socks5: string | null;
  no_tun: boolean;
  latency_first: boolean;
  private_mode: boolean;
  secure_mode: boolean;
  credential_peer: boolean;
};

export type Snapshot = {
  fetched_at: number;
  rpc: string;
  node: SelfNode | null;
  peers: CliPeer[];
  routes: CliRoute[];
  graph: CenterNode[];
  errors: string[];
};

export type AgentInfo = {
  agent: string;
  rpc: string;
  socks: string | null;
  console: { url: string; auth: "agent" | "browser" };
  network_id: string | null;
  pair: boolean;
  registry: boolean;
  /** 局域网访问时 agent 强制只读：不配对、不吊销 */
  read_only?: boolean;
  join: string;
};

export type TemporaryPeer = {
  peer_id: number;
  credential_id?: string | null;
  credential_expiry_unix?: number | null;
  hostname?: string | null;
  ipv4?: string | null;
  version?: string | null;
};

export type Credential = {
  credential_id: string;
  expiry_unix: number;
  reusable: boolean;
  online_peers: TemporaryPeer[];
};

export type NetworkSummary = {
  network_id: string;
  display_name: string;
  network_name: string;
  networking_method: string;
  virtual_cidr: string;
  secure_mode: boolean;
  member_count: number;
  online_member_count: number;
};

export type RegisteredDevice = {
  name: string;
  overlay_ip: string;
  credential_id: string;
  expiry_unix: number;
  paired_at: string;
};

export type PairResult = {
  name: string;
  ip: string;
  code: string;
  credential_id: string | null;
  command: string;
  expires_at: number;
};

export type RelaySpec = {
  name: string;
  region: string;
  zone: string;
  net: number;
  overlay_ip: string;
  wss_port: number;
};

export type NodesDoc = {
  network: {
    name: string;
    /** 显示名（控制台侧栏、探针页顶栏、页面标题）；nodes.yaml 不写时构建插件填 lolinet */
    title: string;
    cidr: string;
    sni: string;
    easytier: string;
    console: string;
    edge_zone: string;
    local_suffix: string;
    join: string;
    issuer: string;
    device_peers?: string[];
  };
  relays: RelaySpec[];
  devices: { name: string; os: string; mode: string; overlay_ip: string; peers: string[] }[];
};
