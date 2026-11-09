# lolinet

lo-li mesh 的命令行。EasyTier（[easytier/build.env](../easytier/build.env) 的版本 + [patches/](../patches/)，含 WSS SNI 补丁）
整个编进同一个二进制，负责后台进程和看状态。

```
lolinet                   # = lolinet status：服务、本机、凭证剩余、接入点和设备（直连 / 中转、延迟、丢包）
lolinet peers | routes    # easytier-cli peer / route
sudo lolinet up [-c 配置]  # 装成系统服务并启动；-c 换配置（配对得到的 loli.toml）
sudo lolinet down         # 停掉并卸载服务，配置留着
sudo lolinet restart
lolinet logs [-f] [-n 50] # 内核日志 /var/log/lolinet/easytier.log
sudo lolinet upgrade      # 按发布清单升级（--check 只看、--dry-run 下载校验不换、--source 换来源）
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
  CI：`easytier.yml` 四个平台和 easytier-core 一起编，并把 lolinet 链接成 `easytier-core` 名字再跑一遍 SNI 实测。
- **服务**：
  - macOS：LaunchDaemon `co.lo-li.lolinet`（`/Library/LaunchDaemons/co.lo-li.lolinet.plist`，RunAtLoad + KeepAlive）。
  - Linux：systemd `lolinet.service`（`Restart=always`）。没有 systemd 的（Docker、NAS）不管。
  - 都是 `/usr/local/bin/lolinet run`：`-c /etc/lolinet/loli.toml --rpc-portal 127.0.0.1:15888`，
    文件日志 `/var/log/lolinet/easytier.log`（10 MB × 5 份），launchd / journald 只收参数错误和崩溃输出（内核的终端日志关掉了：打洞超时这类 warn 很密，launchd 的 stderr 不轮转）。
- **接管 join 脚本以前装的东西**（`lolinet up` 自动做）：
  - 撤掉 `easytier-cli service install` 装的 `easytier` 服务（和 lolinet 抢同一张 TUN、同一条凭据）；
  - `/etc/lolinet/loli.toml` 还没有时，从 `/etc/easytier/loli.toml` 拷过来（旧文件留着）；
  - 老路径的 `easytier-core` / `easytier-cli`（macOS `/usr/local/libexec/easytier/`，Linux `/opt/easytier/`）换成指向 lolinet 的链接，
    探针（`reporter.py` 找 `easytier-cli`）、菜单栏 App（no-tun 内核）、`pair_e2e_test.sh` 不用改。
- **配对码安装**（`curl <准入站>/i | sudo sh -s <配对码>`）：`join/install.sh` 从准入站 `dl/lolinet-<os>-<arch>.tar.gz` 下载、校验 sha256，再 `lolinet up -c <解密出的配置>`。四个平台的包由 `easytier.yml` 编，手动 publish 才发到 `dl/`；`deploy.py install` 查到包不全就不传新脚本。
- **凭证剩余**读配置末尾 `pair.py` 写的 `# credential_id` / `# credential_expiry_unix`；配置是 600，不加 sudo 看不到这一行。

## 自动升级

`lolinet up` 默认装一个定时任务（macOS launchd `co.lo-li.lolinet-upgrade`，Linux systemd `lolinet-upgrade.timer`），
每 6 小时跑一次 `lolinet upgrade --auto`。只有换了版本和出错时才写日志，`/var/log/lolinet/upgrade.log`（Linux 写 journald）。

- **发布清单 `lolinet.json`**：内容是 version 加上四个平台包的 file / sha256，探针包放在旁边。由 [release.py](release.py) 生成，和各平台的包放在同一个目录。
  这里的 version 是「设备应该跑的版本」，lolinet 不比较新旧，只要和本机不一样就换。想回退，就把旧版本重新发布一次。
- **来源**：设置存在 `/etc/lolinet/lolinet.json`，由 `lolinet up` 写。
  - `--upgrade-source github:<owner>/<repo>`：用 GitHub Releases 最新那一版的 `lolinet.json`。
  - `--upgrade-source <清单 URL>`：直接用这个清单。
  - 不设，或者设成 `base`：用准入站的 `<base_url>/dl/lolinet.json`。`join/install.sh` 会把准入站地址经 `LOLINET_BASE_URL` 交给 `lolinet up`。
  - 什么都没有时，退到 `github:prnake/lolinet`。
- **换之前要过三关**：下载用系统 curl；sha256 要和清单一致；新二进制要在本机跑一次 `--version`，结果和清单的版本一致。
  三关都过了，才原子地换掉 `/usr/local/bin/lolinet` 并重启服务。装了探针的话，`reporter.py` 不一样也一起换，然后重启探针。
- **关掉**：`sudo lolinet up --auto-upgrade off`，会撤掉定时任务，`lolinet status` 的「升级」一行能看到当前状态。
- **发布**：
  - 准入站：Actions → easytier → publish，由 `join/deploy.py release` 先传各个包，最后传清单。
  - 公开仓库：推 `v*` tag，或者手动运行 build 并勾上 release，就会发一个 GitHub Release，里面带 `lolinet.json`。

## 还没做

- `lolinet join <配对码>`：兑换和解密搬进 lolinet，install.sh 只剩下载这一步；探针也可以改成 lolinet 内置，不再依赖 python3。
- 菜单栏 App / 下一版 GUI 改成调 lolinet（`lolinet cli -o json …`），不再自己管内核子进程。
