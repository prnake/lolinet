#!/bin/sh
# lolinet 一键接入：curl -fsSL <准入站>/i | sudo sh -s <配对码>
# 兑换一次性加密配置 -> 用配对码在本地解密 -> 装 lolinet（EasyTier 带 WSS SNI 补丁编在里面）-> lolinet up 注册系统服务
# -> 装公开探针（准入站首页的设备卡片，只报硬件和位置；配对时 pair.py --no-probe 就不装）。
# 准入站地址：Worker 发这个脚本时把下面 BASE 的默认值换成自己的地址；直接跑仓库里这份要设 LOLINET_BASE_URL。
set -eu
BASE=${LOLINET_BASE_URL:-@LOLINET_BASE_URL@}
case $BASE in https://*|http://*) ;; *) echo "没有准入站地址：sudo LOLINET_BASE_URL=https://<准入站> sh install.sh <配对码>"; exit 1 ;; esac
CODE=${1:-}
[ -n "$CODE" ] || { echo "用法: curl -fsSL $BASE/i | sudo sh -s <配对码>"; exit 1; }
[ "$(id -u)" = 0 ] || { echo "需要 sudo"; exit 1; }
CODE=$(printf %s "$CODE" | tr 'a-z' 'A-Z' | tr -d ' -')
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d' ' -f1; else shasum -a 256 | cut -d' ' -f1; fi; }
case "$(uname -s)" in Darwin) OS=macos ;; Linux) OS=linux ;; *) echo "不支持的系统"; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) ARCH=aarch64 ;; x86_64|amd64) ARCH=x86_64 ;; *) echo "不支持的架构"; exit 1 ;; esac
# lolinet 的服务是 launchd / systemd；兑换之前先查，免得配对码白白用掉
[ $OS = macos ] || [ -d /run/systemd/system ] || { echo "只支持 systemd（容器、NAS 里用 Docker 版 EasyTier，配对时 pair.py --transport tcp --out）"; exit 1; }
PKG=lolinet-$OS-$ARCH.tar.gz
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT; chmod 700 "$TMP"

ID=$(printf %s "$CODE" | sha | cut -c1-32)
curl -fsS "$BASE/v1/pair/$ID" -o "$TMP/blob" || { echo "配对码无效、已使用或已过期"; exit 1; }
K=$CODE openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -md sha256 -a -A -pass env:K -in "$TMP/blob" -out "$TMP/pair.toml" \
  || { echo "解密失败：配对码不对"; exit 1; }
# pair.py 把探针配置（公开 id + 上报密钥）写在末尾的 `# probe = {...}` 一行，取出来，不留在 EasyTier 配置里
PROBE=$(sed -n 's/^# probe = //p' "$TMP/pair.toml")
sed '/^# probe = /d' "$TMP/pair.toml" > "$TMP/loli.toml"

curl -fsSL "$BASE/dl/$PKG" -o "$TMP/lolinet.tgz"
WANT=$(curl -fsS "$BASE/dl/$PKG.sha256" | cut -d' ' -f1)
[ "$(sha < "$TMP/lolinet.tgz")" = "$WANT" ] || { echo "lolinet 校验失败"; exit 1; }
mkdir "$TMP/bin"; tar -xzf "$TMP/lolinet.tgz" -C "$TMP/bin"
# up：检查配置，把自己装到 /usr/local/bin/lolinet、配置放 /etc/lolinet/loli.toml（600），注册 launchd / systemd 服务
# （管理口只听 127.0.0.1:15888）。以前这个脚本装的 easytier 服务会被撤掉，老路径的 easytier-core / easytier-cli
# 换成指向 lolinet 的链接（探针靠它读 peer）。最后打印状态。
# 准入站地址经环境变量交给 lolinet：自动升级（默认开，每 6 小时）看 $BASE/dl/lolinet.json；老版本 lolinet 不认这个变量，照常装
LOLINET_BASE_URL="$BASE" "$TMP/bin/lolinet" up -c "$TMP/loli.toml"
echo "已接入 lolinet 网络。看状态：lolinet；日志：lolinet logs -f"

probe_macos() {
  install -d -m 755 /usr/local/libexec/mesh-probe /etc/mesh-probe
  install -m 644 "$TMP/probe/reporter.py" /usr/local/libexec/mesh-probe/reporter.py
  (umask 077; printf '%s\n' "$PROBE" > /etc/mesh-probe/reporter.json)
  chown nobody /etc/mesh-probe/reporter.json
  : > /var/log/mesh-probe.log; chown nobody /var/log/mesh-probe.log
  # 以前用 probe/device.py install-mac 手工装的用户级 LaunchAgent 撤掉，免得同一台设备报两份
  UH=$(dscl . -read "/Users/${SUDO_USER:-root}" NFSHomeDirectory 2>/dev/null | sed 's/^NFSHomeDirectory: //')
  if [ -n "${SUDO_USER:-}" ] && [ -f "$UH/Library/LaunchAgents/co.lo-li.mesh-probe.plist" ]; then
    launchctl bootout "gui/$(id -u "$SUDO_USER")/co.lo-li.mesh-probe" 2>/dev/null || true
    rm -rf "$UH/Library/LaunchAgents/co.lo-li.mesh-probe.plist" "$UH/Library/Application Support/lo-li mesh/probe"
  fi
  install -m 644 "$TMP/probe/co.lo-li.mesh-probe.plist" /Library/LaunchDaemons/co.lo-li.mesh-probe.plist
  launchctl bootout system/co.lo-li.mesh-probe 2>/dev/null || true
  # bootout 是异步的：重装时紧接着 bootstrap 会报 5: Input/output error，重试几次（和 lolinet 的 launchd 服务一样）
  i=0
  until launchctl bootstrap system /Library/LaunchDaemons/co.lo-li.mesh-probe.plist 2>/dev/null; do
    i=$((i + 1)); [ $i -lt 10 ] || { echo "探针服务注册失败：launchctl bootstrap system /Library/LaunchDaemons/co.lo-li.mesh-probe.plist"; return 1; }
    sleep 1
  done
}

probe_linux() {
  install -d -m 755 /opt/mesh-probe
  install -d -m 700 /etc/mesh-probe
  install -m 644 "$TMP/probe/reporter.py" /opt/mesh-probe/reporter.py
  (umask 077; printf '%s\n' "$PROBE" > /etc/mesh-probe/reporter.json)
  install -m 644 "$TMP/probe/mesh-probe.service" /etc/systemd/system/mesh-probe.service
  systemctl daemon-reload
  systemctl enable mesh-probe >/dev/null 2>&1
  systemctl restart mesh-probe
}

# 公开探针：reporter.py 只用标准库，macOS 用命令行工具自带的 /usr/bin/python3，Linux 用 systemd（和 relay 同一个单元文件）。
# 缺依赖就跳过，不影响入网；之后装好依赖再用新配对码重跑即可。
if [ -z "$PROBE" ]; then
  echo "这台不装公开探针。"
elif [ $OS = macos ] && ! xcode-select -p >/dev/null 2>&1; then
  echo "跳过公开探针：没有 /usr/bin/python3（先 xcode-select --install）。"
elif [ $OS = linux ] && { ! command -v python3 >/dev/null 2>&1 || [ ! -d /run/systemd/system ]; }; then
  echo "跳过公开探针：需要 python3 和 systemd。"
else
  curl -fsSL "$BASE/dl/mesh-probe.tar.gz" -o "$TMP/probe.tgz"
  WANT=$(curl -fsS "$BASE/dl/mesh-probe.tar.gz.sha256" | cut -d' ' -f1)
  [ "$(sha < "$TMP/probe.tgz")" = "$WANT" ] || { echo "探针校验失败"; exit 1; }
  mkdir "$TMP/probe"; tar -xzf "$TMP/probe.tgz" -C "$TMP/probe"
  probe_$OS
  echo "公开探针已启动（只报硬件和位置），设备 id $(printf %s "$PROBE" | sed -n 's/.*"name":"\(d-[0-9a-f]*\)".*/\1/p')：$BASE"
fi
