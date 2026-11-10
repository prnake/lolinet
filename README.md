# lolinet

基于 EasyTier 的 vibe coding 端到端加密网络。

线上例子：**https://join.edge.lo-li.co**。浏览器打开是公开探针，`curl` 打开是安装命令。

![公开探针：6 个接入点、设备、拓扑和延迟](docs/probe.png)

一个人自建的私有组网，用来替代 Tailscale：
- 6 台云上 relay 全互联，分布在北京、上海、深圳、香港、新加坡、硅谷。
- 电脑、NAS、服务器都是一行命令入网。

内核是 [EasyTier](https://github.com/EasyTier/EasyTier)，打了几个小补丁。外面这层是和 Claude 一起 vibe coding 写出来的：命令行、准入站、配对、控制台和公开探针。

这个仓库只放代码。我自己那套网络的真实节点清单和运维记录在另一个私有仓库里，用同一份代码部署。

## 组成

| 目录 | 内容 |
|---|---|
| [`lolinet/`](lolinet/README.md) | 命令行 `lolinet`。整个 EasyTier 编进同一个二进制，负责装后台服务（launchd / systemd）和看状态 |
| `patches/`、`easytier/` | 私有 EasyTier：上游 v2.7.0 加两个补丁。一个让 WSS 支持自定义 SNI，一个是 lolinet 的入口。`easytier/check_sni.py` 起一个假 relay，实测补丁确实生效 |
| `join/` | 准入站，跑在 Cloudflare Worker 上：配对码兑换、客户端分发、公开探针接口、`<名字>.<edge_zone>` 跳转页。另有 `install.sh` 和部署脚本 |
| `scripts/` | `pair.py` 签凭据、出配对码；`revoke.py` 吊销凭据；`device_config.py` 渲染设备配置 |
| `probe/` | 公开探针上报，relay 和设备都用，只依赖 Python 标准库 |
| `web/` | 前端：本地控制台（拓扑、节点、凭据、配对）和公开探针页面。设计语言叫「猫眼」，见 [web/DESIGN.md](web/DESIGN.md) |
| [`desk/`](desk/README.md) | lolidesk：网里的远程终端和远程桌面。主机端 `lolidesk-host`，客户端 `lolidesk` 带图形界面和终端界面。独立发包，不编进 `lolinet` |

## 怎么连

```
设备 ──wss 443（自定义 SNI）──▶ relay ◀── tcp / udp 全互联 ──▶ relay …
 │                                 ▲ 签发 relay 上跑 easytier-web 控制台（只在 overlay 上）
 └─ curl <准入站>/i | sudo sh -s <配对码>
      └─ Cloudflare Worker：兑换一次性密文、下发客户端、公开探针
```

- **入网：** 用 EasyTier 的 secure mode。每台设备一条控制台签发的凭据，最长 1 年、不可复用，可以单独吊销。设备配置里没有 network secret。
- **配对码：** `pair.py` 用配对码加密设备配置（PBKDF2-SHA256 20 万轮 + AES-256-CBC，和 OpenSSL 兼容），密文放进 Workers KV，15 分钟内有效、只能兑换一次。Worker 只碰得到密文，解密在设备上做。
- **传输：**
  - 设备经 wss 443 连 relay，补丁让 ClientHello 带上自定义 SNI。
  - 开启 KCP 代理，绕开运营商对长 TCP 流的整形。
  - 开启 `latency_first`，按全网延迟表选路。
  - relay 之间用 tcp / udp 全互联。
- **公开探针：** relay 和设备都签名上报（HMAC）。页面上不出现任何名字和地址：relay 只显示城市和规格，设备只显示硬件和大致位置。

<img src="docs/probe-asia.png" width="720" alt="东亚视角">

## 配置

把节点清单 [`nodes.example.yaml`](nodes.example.yaml) 复制成 `nodes.yaml` 再改，或者用 `LOLINET_NODES` 指向别的路径。

| 环境变量 | 作用 |
|---|---|
| `LOLINET_BASE_URL` | 准入站地址，例如 `https://join.edge.lo-li.co`。不设时取节点清单的 `https://<network.join>`。`join/deploy.py` 把它绑进 Worker，Worker 下发 `/i` 时会把 `install.sh` 里的 `@LOLINET_BASE_URL@` 换成这个地址。`pair.py`、探针、前端构建也从这里取 |
| `LOLINET_NODES` | 节点清单的路径。默认是仓库根目录的 `nodes.yaml`，没有就用 `nodes.example.yaml` |
| `LOLINET_HOME` | 运行数据目录，默认 `~/.config/lolinet`，下面放 `.env` 和 `state/` |
| `CLOUDFLARE_API_TOKEN`、`CLOUDFLARE_ACCOUNT_ID` | 部署准入站用。token 只需要 Workers Scripts / Routes / KV / R2 权限。也可以写进 `$LOLINET_HOME/.env`，键名是 `CLOUDFLARE_API` 和 `CLOUDFLARE_ID` |
| `PROBE_MASTER` | 探针上报签名的主密钥，64 位十六进制。本机没有 `state/mesh-probe.json` 时 `deploy.py` 会生成一个。在 CI 里必须提供，CI 不会自己生成 |

`$LOLINET_HOME/state/` 下的文件都是 600 权限，不进仓库：

| 文件 | 内容 |
|---|---|
| `provision-<region>-<name>.json` | 每台 relay 一份：`{"name", "role": "mesh-relay", "eip", "et_port"}` |
| `mesh-console.json` | 控制台账号和网络：`admin_user`、`admin_pw`、`network_id` |
| `mesh-probe.json` | 探针主密钥：`{"master": …}` |
| `mesh-devices.json` | `pair.py` 写的设备登记，不含密钥 |

**仓库里没有任何密钥。** network secret、凭据私钥、控制台密码、探针主密钥都是部署时生成的，只存在 `state/` 和 Worker secret 里。

## 搭一套

1. **准备 relay。** 开几台有公网地址的机器，装 EasyTier，开 secure mode 和 private mode，监听 tcp / udp / wss，并且两两互联。其中一台（`network.issuer`）跑 `easytier-web-embed` 控制台，中心化网络用 Gateway 模式，控制台只监听 overlay 地址。每台 relay 的 `name / eip / et_port` 写进上面的 `provision-*.json`。开 relay 的脚本和云厂商绑得太紧，没有放进这个仓库。

   EasyTier v2.7.0 不认配置文件里的 `rpc_portal`。要在命令行加 `--rpc-portal 127.0.0.1:15888`，否则管理口会监听 0.0.0.0。

2. **编客户端。** 本机目标用 `sh lolinet/build.sh`，产物是 `.build/out/lolinet-<os>-<arch>.tar.gz`。四个平台的 EasyTier 和 lolinet 由 Actions 里的 [build](.github/workflows/build.yml) 编译，编好的包在 [Releases](https://github.com/prnake/lolinet/releases)。

3. **部署准入站。** 先在 Cloudflare 建好节点清单 `cloudflare` 段写的 KV namespace 和 R2 bucket，再执行：

   ```sh
   cp nodes.example.yaml nodes.yaml                     # 改成自己的
   export LOLINET_BASE_URL=https://join.edge.example.com
   uv run join/deploy.py worker                         # Worker + KV / R2 / Durable Object 绑定 + 自定义域
   (cd web && npm ci && npm run build:probe) && uv run join/deploy.py site   # 公开探针页面
   gh release download --repo prnake/lolinet --dir out  # 最新一版的包和清单，也可以用自己 build 的产物
   uv run join/deploy.py release out                    # 客户端包、探针包、升级清单 dl/lolinet.json
   uv run join/deploy.py install                        # install.sh，要先有上面的包
   ```

4. **加设备。** 本机要先在网里，或者用 `--socks` 指向一个 no-tun 实例：

   ```sh
   uv run scripts/pair.py new --name laptop --ip 10.188.16.11
   # 在设备上执行：
   curl -fsSL https://join.edge.example.com/i | sudo sh -s XXXX-XXXX-XXXX
   ```

   入网时默认顺带装上公开探针，不想装就加 `--no-probe`，控制台配对页上也有这个开关。吊销用 `uv run scripts/revoke.py laptop -`。

5. **看状态。** 运行 `lolinet`，可以看到服务状态、本机、凭证剩余天数，以及每个接入点和设备是直连还是中转、延迟多少。也可以打开本地控制台：先运行 `python3 web/agent/mesh_agent.py`，再访问 http://127.0.0.1:10188/ 。

6. **自动升级。** `install.sh` 装的是 lolinet，并且默认带一个定时任务：每 6 小时按发布清单 `lolinet.json` 检查一次。

   - 清单默认在准入站的 `dl/lolinet.json`，由 `join/deploy.py release` 发布。
   - 也可以改用 GitHub Releases：`sudo lolinet up --upgrade-source github:prnake/lolinet`。这个仓库每次发版都带清单，`releases/latest` 始终是 lolinet 的最新一版。

   每次换版本之前都会校验 sha256，并试跑一次新的二进制。手动升级用 `sudo lolinet upgrade`，只看有没有新版本用 `lolinet upgrade --check`，关掉自动升级用 `--auto-upgrade off`。详见 [lolinet/README.md](lolinet/README.md#自动升级)。

   以前用 `easytier-core` / `easytier-cli` 装的设备，跑一次 `sudo lolinet up` 就能切换过来。

## 发布和改动

- **包只在这个仓库编和发。** 在 main 上手动运行 [build](.github/workflows/build.yml) 并勾上 release，就发一版 `lolinet-<版本>`：四个平台的 `lolinet-*`、`easytier-*`、探针包和清单 `lolinet.json`。
- **lolidesk 单独发。** 运行 [desk](.github/workflows/desk.yml) 并勾上 release，发 `lolidesk-<版本>`。它不占 `releases/latest`，自动升级认的是 latest。
- **改动先开 PR。** CI（build、desk、web）通过后再合并，不直接推 `main`。

## 许可

LGPL-3.0，和 EasyTier 一致。lolinet 把 EasyTier 编进了同一个二进制，补丁也是在 EasyTier 上改的。
