// 演示数据（?demo）：接入点之间的延迟取自 2026-11-08 的真实测量（peer-center），负载、流量和可用率是编的。
// 和线上接口一样不放任何名字、地址和公网 IP：relay 用区域代码，设备是编的。
import type { LinkHistory, ProbeDevice, ProbeRelay, ProbeSnapshot } from "./types";

const RELAYS = ["cn-beijing", "cn-shanghai", "cn-shenzhen", "cn-hongkong", "ap-southeast-1", "us-west-1"];

/** 行 = 测量方 */
const LAT: Record<string, Record<string, number>> = {
  "cn-beijing": { "cn-shanghai": 29, "cn-shenzhen": 40, "cn-hongkong": 52, "ap-southeast-1": 97, "us-west-1": 232 },
  "cn-shanghai": { "cn-beijing": 29, "cn-shenzhen": 26, "cn-hongkong": 35, "ap-southeast-1": 63, "us-west-1": 152 },
  "cn-shenzhen": { "cn-beijing": 40, "cn-shanghai": 26, "cn-hongkong": 17, "ap-southeast-1": 61, "us-west-1": 220 },
  "cn-hongkong": { "cn-beijing": 52, "cn-shanghai": 35, "cn-shenzhen": 19, "ap-southeast-1": 35, "us-west-1": 145 },
  "ap-southeast-1": { "cn-beijing": 77, "cn-shanghai": 63, "cn-shenzhen": 65, "cn-hongkong": 35, "us-west-1": 168 },
  "us-west-1": { "cn-beijing": 278, "cn-shanghai": 176, "cn-shenzhen": 217, "cn-hongkong": 145, "ap-southeast-1": 168 },
};
const PROTO: Record<string, string> = { "cn-beijing": "tcp", "cn-shanghai": "tcp", "cn-shenzhen": "tcp", "cn-hongkong": "tcp", "ap-southeast-1": "udp", "us-west-1": "tcp" };
// 编几天不满的可用率，好看出色条的样子：[第几天前, 可用率]
const DIPS: Record<string, [number, number][]> = { "cn-beijing": [[6, 0.971]], "us-west-1": [[12, 0.993], [3, 0.9986]], "cn-shenzhen": [[19, 0.88]] };

function rand(seed: number) {
  let s = seed;
  return () => ((s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
}

export function demoSnapshot(): ProbeSnapshot {
  const now = Math.floor(Date.now() / 1000);
  const today = Math.floor((now + 8 * 3600) / 86400);
  const relays: ProbeRelay[] = RELAYS.map((region, i) => {
    const r = rand(i + 7);
    const cpu = 2 + r() * 9 + (region === "cn-hongkong" ? 14 : 0);
    const rx = 20e3 + r() * 600e3 + (region === "cn-hongkong" ? 2.4e6 : 0);
    const tx = rx * (0.6 + r() * 0.5);
    const jit = (v: number) => v * (0.97 + Math.random() * 0.06);
    return {
      id: region,
      region,
      spec: "阿里云 ecs.c9i.xlarge",
      online: true,
      last_seen: now - Math.floor(Math.random() * 25),
      host: {
        os: "Ubuntu 24.04.3 LTS",
        cpu_model: "Intel(R) Xeon(R) Platinum 8575C",
        cores: 4,
        cpu: Math.round(jit(cpu) * 10) / 10,
        uptime: 86400 * (1 + i) + 3600 * 7,
        load: [cpu / 25, cpu / 30, cpu / 40].map((v) => Math.round(v * 100) / 100),
        mem: [(1.1 + r() * 1.4 + (region === "cn-hongkong" ? 1.2 : 0)) * 2 ** 30, 7.5 * 2 ** 30],
        swap: [0, 0],
        disk: [(6 + r() * 5) * 2 ** 30, 39 * 2 ** 30],
        net: { rx: Math.round(jit(rx)), tx: Math.round(jit(tx)), rx_total: (40 + r() * 300) * 2 ** 30, tx_total: (30 + r() * 260) * 2 ** 30 },
      },
      et: {
        ok: true,
        version: "2.7.0-8b7f1f01",
        devices: 2,
        links: Object.entries(LAT[region]).map(([peer, lat]) => ({
          peer, direct: true, lat: Math.round(jit(lat) * 10) / 10, loss: 0, proto: PROTO[region] === "udp" || PROTO[peer] === "udp" ? "udp" : "tcp",
        })),
      },
      uptime30: 0,
      days: Array.from({ length: 30 }, (_, k) => {
        const ago = 29 - k;
        const dip = DIPS[region]?.find(([d]) => d === ago);
        return { day: (today - ago) * 86400 - 8 * 3600, up: dip ? dip[1] : 1 };
      }),
      series: Array.from({ length: 48 }, (_, k) => ({
        t: now - (47 - k) * 1800,
        cpu: cpu * (0.6 + 0.8 * Math.abs(Math.sin((k + i * 5) / 7))),
        rx: rx * (0.4 + Math.abs(Math.sin((k + i) / 9))),
        tx: tx * (0.4 + Math.abs(Math.sin((k + i) / 9))),
      })),
    };
  });
  for (const r of relays) r.uptime30 = r.days.reduce((a, d) => a + (d.up ?? 0), 0) / r.days.length;
  const blank = { kind: null, model: null, chip: null, cores: null, mem: null, os: null, arch: null, disks: [], location: null };
  const devices: ProbeDevice[] = [
    { id: "d-demo000001", kind: "Mac", model: "Mac mini", chip: "Apple M4", cores: 10, mem: 16 * 2 ** 30, os: "macOS 26.1", arch: "arm64",
      disks: [{ label: null, total: 512e9 }], location: null, near_region: "cn-beijing",
      links: [{ region: "cn-beijing", lat: 12 }, { region: "cn-shanghai", lat: 36 }], status: "online", online: true, hardware: true, last_seen: now },
    { id: "d-demo000002", kind: "NAS", model: "四盘位 NAS", chip: "Intel(R) N100", cores: 4, mem: 16 * 2 ** 30, os: "Debian 12", arch: "x86_64",
      disks: [{ label: "卷 1", total: 1e12 }, { label: "卷 2", total: 8e12 }], location: null, near_region: "cn-shanghai",
      links: [{ region: "cn-shanghai", lat: 8 }, { region: "cn-hongkong", lat: 41 }], status: "online", online: true, hardware: true, last_seen: now },
    { id: "d-demo000003", ...blank, near_region: "cn-hongkong", links: [{ region: "cn-hongkong", lat: 61 }],
      status: "online", online: true, hardware: false, last_seen: now },
    { id: "d-demo000004", kind: "笔记本", model: "MacBook Air", chip: "Apple M3", cores: 8, mem: 8 * 2 ** 30, os: "macOS 26.1", arch: "arm64",
      disks: [{ label: null, total: 256e9 }], location: "新加坡", near_region: "ap-southeast-1", links: [],
      status: "standby", online: false, hardware: true, last_seen: now - 40 },
  ];
  return { now, interval: 30, relays, peers: 3, devices };
}

export function demoLink(a: string, b: string): LinkHistory {
  const now = Math.floor(Date.now() / 1000 / 300) * 300;
  const series = (base: number | undefined, seed: number) => {
    if (base === undefined) return [];
    const r = rand(seed);
    return Array.from({ length: 288 }, (_, k) => {
      const spike = k > 180 && k < 190 ? base * 0.6 : 0; // 编一段晚高峰抖动
      return { t: now - (287 - k) * 300, lat: Math.round((base * (0.94 + r() * 0.1) + spike) * 10) / 10 };
    });
  };
  return { a, b, ab: series(LAT[a]?.[b], a.length * 31 + b.length), ba: series(LAT[b]?.[a], b.length * 17 + a.length) };
}
