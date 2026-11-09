import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { Plugin } from "vite";
import { parse } from "yaml";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
/** 节点清单：LOLINET_NODES，否则仓库根的 nodes.yaml，没有就用 nodes.example.yaml（公开仓库只有这份） */
const NODES_YAML =
  process.env.LOLINET_NODES ?? [`${ROOT}nodes.yaml`, `${ROOT}nodes.example.yaml`].find((p) => existsSync(p)) ?? `${ROOT}nodes.yaml`;

/** 读节点清单；LOLINET_BASE_URL 设了就用它的主机当 network.join */
export function loadNodes() {
  // network.console 在 nodes.yaml 里出现了两次（节点名、URL），和 PyYAML 一样取后一个
  const doc = parse(readFileSync(NODES_YAML, "utf8"), { uniqueKeys: false });
  if (process.env.LOLINET_BASE_URL) doc.network.join = new URL(process.env.LOLINET_BASE_URL).host;
  return doc;
}

/** 准入站地址：LOLINET_BASE_URL，不设就是 https://<network.join> */
export const baseUrl = () => (process.env.LOLINET_BASE_URL ?? `https://${loadNodes().network.join}`).replace(/\/+$/, "");

/**
 * 节点清单是唯一来源：构建时读成 virtual:mesh-nodes，前端不手抄 relay 列表。
 * public = 公开探针用：只留网络名和域名后缀。relay 和设备的名字、EIP / VPC / overlay 地址、伪装 SNI、控制台地址都不进公开包。
 */
export function meshNodes({ public: isPublic = false } = {}): Plugin {
  const id = "virtual:mesh-nodes";
  return {
    name: "mesh-nodes",
    resolveId: (s) => (s === id ? "\0" + id : undefined),
    load(s) {
      if (s !== "\0" + id) return;
      this.addWatchFile(NODES_YAML);
      const { network, relays, devices } = loadNodes();
      const doc = isPublic
        ? {
            network: { name: network.name, cidr: network.cidr, edge_zone: network.edge_zone, local_suffix: network.local_suffix, join: network.join },
            relays: [], // 公开页面只用接口里的区域代码，relay 名字也不进包
            devices: [],
          }
        : { network, relays, devices };
      return `export default ${JSON.stringify(doc)};`;
    },
  };
}
