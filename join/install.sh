#!/bin/sh
# lo-li mesh 一键安装 / 升级 lolinet（EasyTier 带 WSS SNI 补丁编在里面）：
#   curl -fsSL <准入站>/i | sudo sh -s <配对码>   新设备：兑换一次性加密配置、本地解密、装 lolinet、注册系统服务、装公开探针
#   curl -fsSL <准入站>/i | sudo sh               已入网的设备：用本机现有配置装上 / 升级到最新的 lolinet，已是最新就不动
#   curl -fsSL <准入站>/i | sh -s check           只看：最新版、本机版本、要不要装（下载校验一遍，不装，不要 sudo）
# lolinet 从 GitHub（${LOLINET_GITHUB}，默认 prnake/lolinet）最新的 Release 下载，连不上或太慢就退到准入站 dl/
# （dist.yml 从同一个 Release 原样同步）。sha256 按那一版的 lolinet.json 校验，新二进制试跑 --version 对了才装。
# 准入站地址：Worker 发这个脚本时把下面 BASE 的默认值换成自己的地址；直接跑仓库里这份要设 LOLINET_BASE_URL。
set -eu
BASE=${LOLINET_BASE_URL:-@LOLINET_BASE_URL@}
GH=${LOLINET_GITHUB:-prnake/lolinet}
BIN=/usr/local/bin/lolinet
case $BASE in https://*|http://*) ;; *) echo "没有准入站地址：sudo LOLINET_BASE_URL=https://<准入站> sh install.sh [配对码]"; exit 1 ;; esac
ARG=$(printf %s "${1:-}" | tr 'a-z' 'A-Z' | tr -d ' -')
CODE=
case $ARG in
  '' | UPDATE | UPGRADE | REPAIR) MODE=update ;;
  CHECK) MODE=check ;;
  *) MODE=pair; CODE=$ARG ;;
esac
[ $MODE = check ] || [ "$(id -u)" = 0 ] || { echo "需要 sudo：curl -fsSL $BASE/i | sudo sh${CODE:+ -s <配对码>}"; exit 1; }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d' ' -f1; else shasum -a 256 | cut -d' ' -f1; fi; }
case "$(uname -s)" in Darwin) OS=macos ;; Linux) OS=linux ;; *) echo "不支持的系统"; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) ARCH=aarch64 ;; x86_64|amd64) ARCH=x86_64 ;; *) echo "不支持的架构"; exit 1 ;; esac
# lolinet 的服务是 launchd / systemd；兑换之前先查，免得配对码白白用掉
SYSTEMD=yes; [ $OS = macos ] || [ -d /run/systemd/system ] || SYSTEMD=
[ $MODE = check ] || [ -n "$SYSTEMD" ] || { echo "只支持 systemd（容器、NAS 里用 Docker 版 EasyTier，配对时 pair.py --transport tcp --out）"; exit 1; }
HAVE_CONF=; { [ -s /etc/lolinet/loli.toml ] || [ -s /etc/easytier/loli.toml ]; } && HAVE_CONF=yes
[ $MODE != update ] || [ -n "$HAVE_CONF" ] || { echo "本机还没入网（没有 /etc/lolinet/loli.toml），要配对码：curl -fsSL $BASE/i | sudo sh -s <配对码>"; exit 1; }
# relay 的配置带 network_secret（设备是凭据入网，没有）：relay 有自己的 easytier 服务和参数，这个脚本不碰
if [ $MODE = update ] && grep -qs '^network_secret' /etc/lolinet/loli.toml /etc/easytier/loli.toml; then
  echo "这台是 relay（配置里有 network_secret），不用这个脚本装 lolinet"; exit 1
fi
CUR=
if [ -x "$BIN" ]; then CUR=$("$BIN" --version 2>/dev/null | sed -n 's/^lolinet //p') || CUR=; fi
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT; chmod 700 "$TMP"

# 下载：太慢（20 秒不到 50 KB/s）也算失败，好退到下一个来源
dl() { curl -fsSL --connect-timeout 10 --speed-limit 51200 --speed-time 20 "$@"; }

# 那一版文件所在的目录。GitHub：跟 releases/latest 的跳转拿到 tag，后面的文件都钉在这一版（下到一半发了新版也不会混）
release_dir() {
  case $1 in
    github)
      loc=$(curl -fsS -o /dev/null --connect-timeout 10 --max-time 20 -w '%{redirect_url}' "https://github.com/$GH/releases/latest" 2>/dev/null) || return 1
      tag=${loc##*/releases/tag/}
      [ -n "$tag" ] && [ "$tag" != "$loc" ] || return 1
      echo "https://github.com/$GH/releases/download/$tag" ;;
    base) echo "$BASE/dl" ;;
  esac
}

# 清单 lolinet.json（lolinet/release.py 写的）：version 和各平台包的 file / sha256。只用 sed 读，不靠 python / jq
manifest() {
  m=$(tr -d ' \t\r\n' < "$1")
  VER=$(printf %s "$m" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')
  e=$(printf %s "$m" | sed -n "s/.*\"$OS-$ARCH\":{\([^}]*\)}.*/\1/p")
  FILE=$(printf %s "$e" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')
  WANT=$(printf %s "$e" | sed -n 's/.*"sha256":"\([0-9a-f]\{64\}\)".*/\1/p')
  case $FILE in lolinet-*.tar.gz) ;; *) return 1 ;; esac
  case $FILE in */*) return 1 ;; esac
  [ -n "$VER" ] && [ -n "$WANT" ]
}

# GitHub 优先，不行用准入站。本机已经是清单上的版本就不下载
SRC= NEW=
for S in github base; do
  if ! REL=$(release_dir $S); then echo "连不上 GitHub（$GH 的 Release），改用准入站 $BASE/dl/"; continue; fi
  if ! { dl "$REL/lolinet.json" -o "$TMP/lolinet.json" && manifest "$TMP/lolinet.json"; }; then echo "读不到 $REL/lolinet.json"; continue; fi
  if [ "$VER" = "$CUR" ]; then SRC=$S; NEW=$BIN; break; fi
  if ! dl "$REL/$FILE" -o "$TMP/lolinet.tgz"; then echo "从 $REL 下载 $FILE 失败或太慢"; continue; fi
  [ "$(sha < "$TMP/lolinet.tgz")" = "$WANT" ] || { echo "$FILE 校验失败（${REL}）：sha256 和 lolinet.json 不一致"; exit 1; }
  mkdir -p "$TMP/bin"; tar -xzf "$TMP/lolinet.tgz" -C "$TMP/bin" lolinet
  [ "$("$TMP/bin/lolinet" --version 2>/dev/null)" = "lolinet $VER" ] || { echo "下载的 lolinet 在本机试跑不对，应该是 lolinet $VER"; exit 1; }
  SRC=$S; NEW=$TMP/bin/lolinet; break
done
[ -n "$SRC" ] || { echo "GitHub 和准入站都下载不到 lolinet"; exit 1; }
if [ $SRC = github ]; then FROM="GitHub $GH"; else FROM="准入站 $BASE/dl/"; fi

# lolinet 的服务和自动升级都在、以前这个脚本装的 easytier 服务不在（自动升级关了的不算缺）
installed_ok() {
  auto_off=; grep -q '"auto_upgrade": *false' /etc/lolinet/lolinet.json 2>/dev/null && auto_off=yes
  if [ $OS = macos ]; then d=/Library/LaunchDaemons; svc=co.lo-li.lolinet.plist; old=easytier.plist; timer=co.lo-li.lolinet-upgrade.plist
  else d=/etc/systemd/system; svc=lolinet.service; old=easytier.service; timer=lolinet-upgrade.timer; fi
  [ -f $d/$svc ] && [ ! -f $d/$old ] && { [ -n "$auto_off" ] || [ -f $d/$timer ]; }
}

# up：检查配置，把自己装到 /usr/local/bin/lolinet、配置放 /etc/lolinet/loli.toml（600），注册 launchd / systemd 服务
# （管理口只听 127.0.0.1:15888）和自动升级（每 6 小时）。以前这个脚本装的 easytier 服务会被撤掉，老路径的
# easytier-core / easytier-cli 换成指向 lolinet 的链接（探针靠它读 peer）。最后打印状态。
# 自动升级跟着这次下载成功的来源：GitHub 连得上就看 GitHub 的 Release，连不上的网络看准入站 dl/（两边是同一版的文件）
up() {
  set -- up "$@"
  if "$NEW" up --help 2>/dev/null | grep -q -- --upgrade-source; then
    if [ $SRC = github ]; then set -- "$@" --upgrade-source "github:$GH"; else set -- "$@" --upgrade-source base; fi
  fi
  LOLINET_BASE_URL="$BASE" "$NEW" "$@"
}

fetch_probe() {
  dl "$REL/mesh-probe.tar.gz" -o "$TMP/probe.tgz" || return 1
  want=$(dl "$REL/mesh-probe.tar.gz.sha256" | cut -d' ' -f1) || return 1
  [ "$(sha < "$TMP/probe.tgz")" = "$want" ] || return 1
  mkdir -p "$TMP/probe"; tar -xzf "$TMP/probe.tgz" -C "$TMP/probe"
}

if [ $MODE = check ]; then
  if [ "$NEW" = "$BIN" ]; then got="本机已是这一版，没下载"; else got="下载和 sha256 都对"; fi
  echo "最新      lolinet ${VER}（${FROM}，${got}）"
  if [ -n "$CUR" ]; then echo "本机      lolinet $CUR"; else echo "本机      没装 lolinet"; fi
  if [ -z "$HAVE_CONF" ]; then echo "入网      没看到 /etc/lolinet/loli.toml（以前的装法要 sudo 才看得到）"; fi
  if [ "$VER" != "$CUR" ]; then echo "要装：     curl -fsSL $BASE/i | sudo sh"
  elif ! installed_ok; then echo "版本是最新，但服务或自动升级没装好：curl -fsSL $BASE/i | sudo sh"
  else echo "已是最新，服务和自动升级都在，不用动"; fi
  exit 0
fi

if [ $MODE = update ]; then
  if [ "$NEW" = "$BIN" ] && installed_ok; then
    echo "lolinet $VER 已是最新（${FROM}），服务和自动升级都在，不用动"
    "$BIN" || true
  else
    up
    echo "lolinet ${CUR:-（没装）} -> ${VER}（${FROM}）。看状态：lolinet；日志：lolinet logs -f"
  fi
  # 装过公开探针的（配对时装的）：reporter.py 和这一版不一样就换掉、重启探针；没装过的不装（探针配置在配对包里）
  PROBE_PY=/usr/local/libexec/mesh-probe/reporter.py; [ $OS = macos ] || PROBE_PY=/opt/mesh-probe/reporter.py
  if [ -f /etc/mesh-probe/reporter.json ] && [ -f "$PROBE_PY" ]; then
    if ! fetch_probe; then echo "探针包下载或校验失败，探针没动"
    elif ! cmp -s "$TMP/probe/reporter.py" "$PROBE_PY"; then
      install -m 644 "$TMP/probe/reporter.py" "$PROBE_PY"
      if [ $OS = macos ]; then launchctl kickstart -k system/co.lo-li.mesh-probe 2>/dev/null || true; else systemctl try-restart mesh-probe || true; fi
      echo "公开探针 reporter.py 已更新"
    fi
  fi
  exit 0
fi

# 配对：lolinet 已经下好、校验过，再兑换（下载失败不会白用掉配对码）
ID=$(printf %s "$CODE" | sha | cut -c1-32)
curl -fsS "$BASE/v1/pair/$ID" -o "$TMP/blob" || { echo "配对码无效、已使用或已过期"; exit 1; }
K=$CODE openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -md sha256 -a -A -pass env:K -in "$TMP/blob" -out "$TMP/pair.toml" \
  || { echo "解密失败：配对码不对"; exit 1; }
# pair.py 把探针配置（公开 id + 上报密钥）写在末尾的 `# probe = {...}` 一行，取出来，不留在 EasyTier 配置里
PROBE=$(sed -n 's/^# probe = //p' "$TMP/pair.toml")
sed '/^# probe = /d' "$TMP/pair.toml" > "$TMP/loli.toml"
up -c "$TMP/loli.toml"
echo "已接入 lo-li mesh（lolinet ${VER}，${FROM}）。看状态：lolinet；日志：lolinet logs -f"

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
  fetch_probe || { echo "探针包下载或校验失败"; exit 1; }
  probe_$OS
  echo "公开探针已启动（只报硬件和位置），设备 id $(printf %s "$PROBE" | sed -n 's/.*"name":"\(d-[0-9a-f]*\)".*/\1/p')：$BASE"
fi
