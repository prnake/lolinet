# lolinet

lo-li mesh 的命令行。EasyTier（[easytier/build.env](../easytier/build.env) 的版本 + [patches/](../patches/)，含 WSS SNI 补丁）
整个编进同一个二进制，负责后台进程、看状态和公开探针。

```
lolinet                   # = lolinet status：服务、升级、探针、本机、凭证剩余、接入点和设备（直连 / 中转、延迟、丢包）
lolinet peers | routes    # easytier-cli peer / route
sudo lolinet up [-c 配置]  # 装成系统服务并启动；-c 换配置（配对得到的 loli.toml，带探针的话一起装）
sudo lolinet down         # 停掉并卸载服务（含探针），配置留着
sudo lolinet restart
lolinet logs [-f] [-n 50] # 内核日志 /var/log/lolinet/easytier.log
sudo lolinet upgrade      # 按发布清单升级（--check 只看、--dry-run 下载校验不换、--source 换来源）
sudo lolinet probe [send]  # 公开探针这一轮要报什么（send：真发一次）；sudo lolinet probe off 关掉
lolinet core …            # 原版 easytier-core
lolinet cli …             # 原版 easytier-cli
```

## 怎么做的

- **一个二进制，按 `argv[0]` 分发**（busybox 那种）：叫 `easytier-core` / `easytier-cli` 时直接进 EasyTier 原版入口
  （core 是库里的 `easytier::core::main`，cli 是把 `easytier-cli.rs` 当模块编进来），叫 `lolinet` 时走自己的子命令。
  `lolinet run`、`peers`、`core`、`cli` 都是换个 `argv[0]` exec 自己，参数原样交给 EasyTier。
- **构建**：lolinet 是 easytier 包的第三个 bin（`patches/easytier-v2.7.0-lolinet.patch` 加 `[[bin]]`，并把 cli 的 `main` 改成 `pub`），
  源码在这里，构建时拷到 `easytier/src/lolinet/`。和 `easytier-core` 同一份依赖、同一个 Cargo.lock（`--locked`）。
  本机：`sh lolinet/build.sh`，产物 `.build/out/lolinet-<os>-<arch>.tar.gz`；上游源码缓存在 `.build/easytier`（增量编译）。
  CI：公开仓库 prnake/lolinet 的 `build.yml` 四个平台和 easytier-core 一起编，并把 lolinet 链接成 `easytier-core` 名字再跑一遍 SNI 实测。
- **服务**：
  - macOS：LaunchDaemon `co.lo-li.lolinet`（`/Library/LaunchDaemons/co.lo-li.lolinet.plist`，RunAtLoad + KeepAlive）。
  - Linux：systemd `lolinet.service`（`Restart=always`）。没有 systemd 的（Docker、NAS）不管。
  - 都是 `/usr/local/bin/lolinet run`：`-c /etc/lolinet/loli.toml --rpc-portal 127.0.0.1:15888`，
    文件日志 `/var/log/lolinet/easytier.log`（10 MB × 5 份），launchd / journald 只收参数错误和崩溃输出（内核的终端日志关掉了：打洞超时这类 warn 很密，launchd 的 stderr 不轮转）。
- **接管 join 脚本以前装的东西**（`lolinet up` 自动做）：
  - 撤掉 `easytier-cli service install` 装的 `easytier` 服务（和 lolinet 抢同一张 TUN、同一条凭据）；
  - `/etc/lolinet/loli.toml` 还没有时，从 `/etc/easytier/loli.toml` 拷过来（旧文件留着）；
  - 老路径的 `easytier-core` / `easytier-cli`（macOS `/usr/local/libexec/easytier/`，Linux `/opt/easytier/`）换成指向 lolinet 的链接，
    菜单栏 App（no-tun 内核）、`pair_e2e_test.sh` 不用改；
  - 以前 install.sh 装的 python 版探针（`reporter.py` + `co.lo-li.mesh-probe` / `mesh-probe.service`）换成内置的，见下面「公开探针」。
- **一键安装 / 升级**（`join/install.sh`，准入站 `/i` 发）：lolinet 从 GitHub 最新的 Release 下载（跟 `releases/latest` 的跳转钉住 tag），
  连不上或太慢（20 秒不到 50 KB/s）就退到准入站 `dl/`；sha256 按那一版的 `lolinet.json` 校验，新二进制试跑 `--version` 对了才装。
  - `curl <准入站>/i | sudo sh -s <配对码>`：新设备。先下载校验好 lolinet 再兑换（下载失败不会白用掉配对码），`lolinet up -c <解密出的配置>`，
    配置里带了探针就一起装上（lolinet 内置，不要 python3）。
  - `curl <准入站>/i | sudo sh`：已入网的设备（`/etc/lolinet/loli.toml`，或者以前这个脚本装的 `/etc/easytier/loli.toml`）。
    本机已是最新、服务、自动升级和探针（配了的话）都在就什么都不动；否则 `lolinet up`（接管旧的 easytier 服务和 python 版探针）。
    relay（配置里有 `network_secret`）不碰。
  - `curl <准入站>/i | sh -s check`：不要 sudo，只下载校验，告诉你最新版、本机版本、要不要装。
  - 自动升级跟着这次下载成功的来源：GitHub 连得上就 `--upgrade-source github:<repo>`，否则 `base`（准入站 `dl/lolinet.json`）。
  - `LOLINET_GITHUB=<owner>/<repo>` 换 GitHub 仓库（默认 `prnake/lolinet`）。
  - 发包：四个平台的包由公开仓库的 `build.yml` 编、发 GitHub Release，私有仓库的 `dist.yml` 原样同步到准入站 `dl/`；
    `deploy.py install` 查到 `dl/` 的包或 `lolinet.json` 不全就不传新脚本。
- **凭证剩余**读配置末尾 `pair.py` 写的 `# credential_id` / `# credential_expiry_unix`；配置是 600，不加 sudo 看不到这一行。

## 自动升级

`lolinet up` 默认装一个定时任务（macOS launchd `co.lo-li.lolinet-upgrade`，Linux systemd `lolinet-upgrade.timer`），
每 6 小时跑一次 `lolinet upgrade --auto`。只有换了版本和出错时才写日志，`/var/log/lolinet/upgrade.log`（Linux 写 journald）。

- **发布清单 `lolinet.json`**：内容是 version 加上四个平台包的 file / sha256。由 [release.py](release.py) 生成，和各平台的包放在同一个目录。
  这里的 version 是「设备应该跑的版本」，lolinet 不比较新旧，只要和本机不一样就换。想回退，就把旧版本重新发布一次。
- **来源**：设置存在 `/etc/lolinet/lolinet.json`，由 `lolinet up` 写。
  - `--upgrade-source github:<owner>/<repo>`：用 GitHub Releases 最新那一版的 `lolinet.json`。
  - `--upgrade-source <清单 URL>`：直接用这个清单。
  - 不设，或者设成 `base`：用准入站的 `<base_url>/dl/lolinet.json`。`join/install.sh` 会把准入站地址经 `LOLINET_BASE_URL` 交给 `lolinet up`。
  - 什么都没有时，退到 `github:prnake/lolinet`。
- **换之前要过三关**：下载用系统 curl；sha256 要和清单一致；新二进制要在本机跑一次 `--version`，结果和清单的版本一致。
  三关都过了，才原子地换掉 `/usr/local/bin/lolinet` 并重启服务和探针（探针编在里面，跟着一起换）。
  每次跑（换没换都算）还会把探针理一遍：python 版接管过来，服务文件和这一版写的不一样就重装。
- **关掉**：`sudo lolinet up --auto-upgrade off`，会撤掉定时任务，`lolinet status` 的「升级」一行能看到当前状态。
- **发布**：
  - GitHub Release：在公开仓库 prnake/lolinet 推 `v*` tag，或者在 main 上手动运行 build 并勾上 release，就会发一版，里面是四个平台的包和 `lolinet.json`。
  - 准入站：把那一版的文件下载到一个目录（`gh release download`），`join/deploy.py release <目录>` 先传各个包，最后传清单。两个来源拿到的是同一份文件。

## 公开探针

准入站首页的设备卡片：只报硬件和位置（类型、型号、芯片、核数、内存、存储总量、系统、架构，加上配置写的城市或最近的接入点），
每 60 秒一次；不报主机名、地址、序列号、负载。以前是 `probe/reporter.py` 的设备模式（要 python3），现在编在 lolinet 里，
上报格式和签名不变，准入站不用改。

- **配置**：`scripts/pair.py` 签码时在配置末尾加一行 `# probe = {...}`（公开 id `d-xxxxxxxxxx`、这台设备的上报密钥、上报地址；
  `--no-probe` 不加）。`lolinet up -c` 把这一行存到 `/etc/lolinet/probe.json`（600），不进 EasyTier 配置。
  `-c` 的配置里没有这一行时，已经装着的探针不动。换了公开 id（设备改名后重新配对）会先把旧 id 从公开页面撤下。
- **服务**：macOS launchd `co.lo-li.lolinet-probe`（日志 `/var/log/lolinet/probe.log`），Linux systemd `lolinet-probe.service`（journald），
  都是 `/usr/local/bin/lolinet probe run`。以 root 启动、读完配置就降到 `nobody`；systemd 只给它降权要的两个能力，文件系统只读。
  硬件读 `system_profiler` / `sysctl` / `sw_vers`（macOS）或 `/proc`、`/sys/class/dmi`（Linux），最近的接入点问本机内核
  （`easytier-cli peer`，管理口 15888，菜单栏 App 的 15897 兜底），用系统 curl 发，参数和签名从 stdin 给，不出现在进程列表里。
  一直发不出去时错误一小时记一条。
- **接管 python 版**：`lolinet up` 和 `lolinet upgrade` 看到 `/etc/mesh-probe/reporter.json` 是设备模式的，就把它搬成
  `/etc/lolinet/probe.json`（公开 id 不变），撤掉 `co.lo-li.mesh-probe` / `mesh-probe.service` 和 `reporter.py`。
  relay 模式的配置（relay 上 `probe/deploy.py` 装的）不碰。所以已经装了 python 版的设备，升级到这一版后最多 6 小时（下一次自动升级检查）
  就换成内置的；想马上换就 `sudo lolinet up`。
- **看和关**：`lolinet` 的「探针」一行是服务状态（加 sudo 才显示公开 id）；`sudo lolinet probe` 打印这一轮要报的内容，
  `sudo lolinet probe send` 真发一次；`sudo lolinet probe off` 从公开页面撤下、卸载服务、删配置，以后要再装得重新配对。
- relay 的探针（负载、链路）和 Docker 里的设备（没有 systemd，装不了 lolinet 服务）还用 `probe/reporter.py`。

## 还没做

- `lolinet join <配对码>`：兑换和解密搬进 lolinet，install.sh 只剩下载这一步。
- relay 换成 lolinet 之后，relay 模式的探针（负载、链路）也编进来，`probe/reporter.py` 就可以删了。
- 菜单栏 App / 下一版 GUI 改成调 lolinet（`lolinet cli -o json …`），不再自己管内核子进程。
