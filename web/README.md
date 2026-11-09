# lo-li mesh 控制台（React）

EasyTier 自己的 `easytier-web` 只管凭据和成员，看不到「全网现在怎么连的」。这个前端把两边合起来：
本机内核看到的全网拓扑和延迟 + 控制台的凭据 + 配对流程，用一套固定的美学（[DESIGN.md](DESIGN.md)「猫眼」）。

```
浏览器 ──http──▶ agent 127.0.0.1:10188 ─┬─ easytier-cli -o json  node / peer / route / peer-center ─▶ 本机 easytier-core
 (React)                                ├─ /api/v1/* 反代 ─(TUN 直连或 SOCKS5)─▶ 控制台 network.console（easytier-web）
                                        └─ /api/mesh/pair ─▶ uv run scripts/pair.py（签凭据、加密、放进 KV）
```

## 页面

| 页 | 数据 | 内容 |
|---|---|---|
| 总览 `#/overview` | `peer-center` / `route` / `peer` | 双色大标题直接给结论；示意地图上的拓扑（一个节点一只光标、悬停看延迟、点一下画出本机实际走的路）；链路（本机 → 中转 → 目标，延迟优先比直连快多少）；全网延迟矩阵 |
| 节点 `#/nodes` | 同上 | 按接入点 / 设备 / 网关分组，筛选；右侧抽屉：本机到它的链路、它的直连、内网名 / 公网名 / NAT / 版本 |
| 凭据 `#/access` | 控制台 `/api/v1/networks/*/credentials` + 设备登记 | 在线状态、剩余有效期（< 7 天变黄）、两次确认吊销（顺手删掉 `state/mesh-devices.json` 里那条） |
| 配对 `#/pair` | `pair.py` | 三步：填设备（自动取 ipam 里下一个空地址）→ 配对码 + 一行安装命令（15 分钟倒计时）→ 路由表里出现新设备就亮起来 |
| 设计语言 `#/design` | — | 活样张：原则、颜色、字、控件、动效 |

顶栏右上角切「跟随系统 / 日间 / 夜间」，默认跟随系统，选择存在浏览器里。

## 用

```sh
cd web && npm install && npm run build      # dist/（控制台）和 dist-probe/（公开探针）

# 菜单栏 App 的 no-tun 内核（管理口 15897，SOCKS5 18080）
python3 web/agent/mesh_agent.py --rpc 127.0.0.1:15897 --socks 127.0.0.1:18080 --allow-pair
# TUN 版系统服务（管理口 15888，overlay 直连，不用 --socks）
python3 web/agent/mesh_agent.py --allow-pair

open http://127.0.0.1:10188/
```

- agent 只用 Python 标准库（macOS 自带的 3.9 就行），relay 上也能直接跑。
- 能读到$LOLINET_HOME/`state/mesh-console.json` 时由 agent 登录控制台，浏览器不碰密码；读不到就在凭据页登录（控制台接口要 md5(密码)，在浏览器里算）。
- 不加 `--allow-pair` 时配对页只给出等价的 `pair.py` 命令。
- 局域网看：`--listen 0.0.0.0:10188 --allow-host <本机局域网 IP>`。监听非回环地址时 agent **强制只读**（同网段谁都能自己带
  `X-Mesh-Agent` 头）：拓扑、节点、凭据列表照常看，配对、吊销、删登记返回 403，页面上对应按钮变灰并说明原因。
- `?demo` 用内置演示数据（2026-11-08 从本机内核抓的真实拓扑，凭据和设备是编的），配对流程能完整走一遍；agent 连不上时开屏也有「看演示数据」。

开发：先起 agent，再 `npm run dev`（5188 端口，`/api` 代理到 agent；`MESH_AGENT=http://…` 可改）。
刷新演示数据：`python3 web/agent/mesh_agent.py --rpc <内核> --dump > web/src/data/demo-snapshot.json`。

`nodes.yaml` 是节点清单的唯一来源：构建时由 Vite 插件读成 `virtual:mesh-nodes`，前端不手抄 relay 列表。

## 公开探针（join.edge.lo-li.co 首页）

同一套组件的第二份构建：`probe.html` → `src/probe/`，`vite.probe.config.ts` 输出到 `dist-probe/`。
数据来自 `join.edge.lo-li.co/v1/probe`（各 relay 自报，见 `README.md`「公开探针」）。

```sh
npm run dev:probe                 # http://127.0.0.1:5189/probe.html，/v1 代理到线上 join 站；加 ?demo 用演示数据
npm run build:probe               # dist-probe/
uv run ../join/deploy.py site     # 传到 R2 site/（在 web 下执行时路径按需调整）
```

- 页面：结论大标题 + 六个数字、拓扑（只有 relay）、每台一张卡片（CPU / 内存 / 硬盘 / 网速 / 流量 / 负载 / 运行时间 / 30 天可用率）、
  延迟矩阵（点格子切换）+ 那条链路 24 小时双向延迟、加入说明。
- 公开包用 `meshNodes({ public: true })`：节点清单只留 relay 名字和区域；也不从 `data/store` 引东西（那边会带上控制台的演示快照）。
  `npm run build` 之后用字符串检查过 `dist-probe/` 里没有设备名、EIP、伪装 SNI、控制台域名、`*_secret`。
- `?demo`：接入点之间延迟是 2026-11-08 的真实测量，负载、流量、可用率是编的。

## agent 的边界

| 接口 | |
|---|---|
| `GET /api/mesh/info` | 能力：控制台鉴权方式、能不能配对、network_id |
| `GET /api/mesh/snapshot` | 四条 cli 并发跑，1.5 秒内合并；`node` 只留主机名、地址、NAT、版本和几个开关 |
| `GET /api/mesh/devices` | $LOLINET_HOME/`state/mesh-devices.json` |
| `POST /api/mesh/pair` | 校验名字 / 地址 / 有效期 / 接入点后调 `pair.py` |
| `POST /api/mesh/forget` | 从设备登记删一条 |
| `/api/v1/*` | 反代控制台；读只放前端用到的（登录状态、网络、凭据列表），写只放登录和吊销凭据 |

- 只听 `127.0.0.1`；`Host` 必须是 127.0.0.1 / localhost（防 DNS rebinding），反代部署时用 `--allow-host` 加名字。
- 非 GET 必须带 `X-Mesh-Agent: 1`，跨站页面带不上这个头。
- 控制台响应里的 `credential_secret`、`network_secret`、私钥字段一律删掉再给浏览器；`easytier-cli node` 输出的配置原文（含 secure mode 私钥）和公网 IP 不出 agent。
- 控制台 https 校验证书（`*.<edge_zone>`），经 SOCKS 时 SNI 仍是控制台域名。

## 验证情况（2026-11-08）

- 演示数据：五个页面、抽屉、配对三步、开屏离线态都在浏览器里过了一遍。
- 真数据：本机 no-tun 内核（rana-app）→ agent → 页面，6 台 relay + 控制台网关、peer-center 全图都对；
  经 SOCKS 反代控制台，`networks` / `credentials` 正常，`network_secret` 已删。
- agent 防线：错 Host 421、缺头 403、白名单外的写操作 403、路径穿越回 index、配对参数非法 400。
- **没测**：真的点「生成配对码」（会在控制台签真凭据、往 KV 写配对包），agent 这一步只是转调已经端到端验过的 `pair.py`；
  浏览器自己登录控制台的模式（本机有 state 文件，走的一直是 agent 登录）。

## 还没做

- 部署到控制台那台 relay（network.issuer）：agent 用 `--console http://127.0.0.1:11211 --allow-host <控制台域名>` 跑在那台上，
  nginx 把控制台域名的 `/` 换成这个前端（官方前端挪到子路径）。签发节点的视角就是全网视角。
- 菜单栏 App 按 DESIGN.md 换皮。
