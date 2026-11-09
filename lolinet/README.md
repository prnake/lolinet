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
    文件日志 `/var/log/lolinet/easytier.log`（10 MB × 5 份），launchd / journald 只收 warn 以上。
- **接管 join 脚本以前装的东西**（`lolinet up` 自动做）：
  - 撤掉 `easytier-cli service install` 装的 `easytier` 服务（和 lolinet 抢同一张 TUN、同一条凭据）；
  - `/etc/lolinet/loli.toml` 还没有时，从 `/etc/easytier/loli.toml` 拷过来（旧文件留着）；
  - 老路径的 `easytier-core` / `easytier-cli`（macOS `/usr/local/libexec/easytier/`，Linux `/opt/easytier/`）换成指向 lolinet 的链接，
    探针（`reporter.py` 找 `easytier-cli`）、菜单栏 App（no-tun 内核）、`pair_e2e_test.sh` 不用改。
- **凭证剩余**读配置末尾 `pair.py` 写的 `# credential_id` / `# credential_expiry_unix`；配置是 600，不加 sudo 看不到这一行。

## 还没做

- `join/install.sh` 还在装 easytier-core / easytier-cli：等四个平台的 lolinet 发到 `dl/` 以后，
  install.sh 改成下载 lolinet 再 `lolinet up -c`。
- `lolinet join <配对码>`：兑换和解密搬进 lolinet，install.sh 只剩下载这一步；探针也可以改成 lolinet 内置，不再依赖 python3。
- 菜单栏 App / 下一版 GUI 改成调 lolinet（`lolinet cli -o json …`），不再自己管内核子进程。
