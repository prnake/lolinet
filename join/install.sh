#!/bin/sh
# lolinet 一键安装 / 升级（EasyTier 带 WSS SNI 补丁编在里面）：
#   curl -fsSL <准入站>/i | sudo sh -s <配对码>   新设备：兑换一次性加密配置、本地解密、装 lolinet、注册系统服务（含公开探针）
#   curl -fsSL <准入站>/i | sudo sh               已入网的设备：用本机现有配置装上 / 升级到最新的 lolinet，已是最新就不动
#   curl -fsSL <准入站>/i | sh -s check           只看：最新版、本机版本、要不要装（下载校验一遍，不装，不要 sudo）
# 三种都顺带管 lolidesk（网里的远程终端 / 远程桌面）：装上或换成发布的版本，不开主机服务，见 install_lolidesk。
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

# 清单 lolinet.json（lolinet/release.py 写的）：version 和各平台包的 file / sha256。只用 sed 读，不靠 python / jq。
# 第二个参数是哪个产品的清单：lolinet（默认）或 lolidesk（lolidesk.json，同样的格式）
manifest() {
  m=$(tr -d ' \t\r\n' < "$1")
  VER=$(printf %s "$m" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')
  e=$(printf %s "$m" | sed -n "s/.*\"$OS-$ARCH\":{\([^}]*\)}.*/\1/p")
  FILE=$(printf %s "$e" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')
  WANT=$(printf %s "$e" | sed -n 's/.*"sha256":"\([0-9a-f]\{64\}\)".*/\1/p')
  case $FILE in "${2:-lolinet}"-*.tar.gz) ;; *) return 1 ;; esac
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

# lolinet 的服务和自动升级都在、以前这个脚本装的 easytier 服务不在（自动升级关了的不算缺）；
# 配了公开探针的探针服务在，以前这个脚本装的 python 版探针不在（lolinet up 会把它换成内置的）
installed_ok() {
  auto_off=; grep -q '"auto_upgrade": *false' /etc/lolinet/lolinet.json 2>/dev/null && auto_off=yes
  if [ $OS = macos ]; then d=/Library/LaunchDaemons; svc=co.lo-li.lolinet.plist; old=easytier.plist; timer=co.lo-li.lolinet-upgrade.plist
    probe=co.lo-li.lolinet-probe.plist; old_probe=co.lo-li.mesh-probe.plist
  else d=/etc/systemd/system; svc=lolinet.service; old=easytier.service; timer=lolinet-upgrade.timer
    probe=lolinet-probe.service; old_probe=mesh-probe.service; fi
  [ -f $d/$svc ] && [ ! -f $d/$old ] && { [ -n "$auto_off" ] || [ -f $d/$timer ]; } \
    && [ ! -f $d/$old_probe ] && { [ ! -f /etc/lolinet/probe.json ] || [ -f $d/$probe ]; }
}

# up：检查配置，把自己装到 /usr/local/bin/lolinet、配置放 /etc/lolinet/loli.toml（600），注册 launchd / systemd 服务
# （管理口只听 127.0.0.1:15888）、自动升级（每 6 小时）和公开探针（配了的话，lolinet 内置）。以前这个脚本装的
# easytier 服务和 python 版探针会被撤掉（探针配置接管过来，公开 id 不变），老路径的 easytier-core / easytier-cli
# 换成指向 lolinet 的链接。最后打印状态。
# 自动升级跟着这次下载成功的来源：GitHub 连得上就看 GitHub 的 Release，连不上的网络看准入站 dl/（两边是同一版的文件）
up() {
  set -- up "$@"
  if "$NEW" up --help 2>/dev/null | grep -q -- --upgrade-source; then
    if [ $SRC = github ]; then set -- "$@" --upgrade-source "github:$GH"; else set -- "$@" --upgrade-source base; fi
  fi
  LOLINET_BASE_URL="$BASE" "$NEW" "$@"
}

# lolidesk：整个包（lolidesk + libghostty-vt）装进 /usr/local/lib/lolidesk/，/usr/local/bin/lolidesk 链接过去；
# 之后 lolinet upgrade 按 lolidesk.json 更新。不开主机服务。2026-11-10 以前的包是 /usr/local/bin 下的 lolidesk-host、lolidesk
# 两个程序：也换成指向新程序的链接（叫 lolidesk-host 时它就是 `lolidesk host`，老的服务照样起得来）。
# 清单 lolidesk.json：GitHub 上在最新的 lolidesk-* Release 里（它不占 releases/latest，要问 API），准入站上在 dl/。
# lolinet 这次从 GitHub 下的就先 GitHub 后准入站，从准入站下的就只用准入站。已经是清单上的版本就不动。
# 试跑过、跑得起来才装。出什么问题都只跳过，不影响入网。
desk_dir() {
  case $1 in
    github)
      # API 的列表不按发布时间排（排在前面的可能是旧版），按 published_at 取最新的，草稿、预发布不算。
      # 只用 grep / awk 读：每个 Release 里依次是 tag_name、draft、prerelease、published_at（assets 里没有这几个键）
      tag=$(curl -fsS --connect-timeout 10 --max-time 20 "https://api.github.com/repos/$GH/releases?per_page=100" 2>/dev/null \
        | grep -oE '"(tag_name|draft|prerelease|published_at)": *("[^"]*"|[a-z]+)' \
        | awk -F'"' '$2 == "tag_name" { t = $4; skip = 0 }
            ($2 == "draft" || $2 == "prerelease") && $3 ~ /true/ { skip = 1 }
            $2 == "published_at" { if (t ~ /^lolidesk-/ && !skip && $4 != "" && ($4 "") > (best "")) { best = $4; tag = t }; t = "" }
            END { print tag }')
      case $tag in lolidesk-*) ;; *) return 1 ;; esac
      case $tag in *[!A-Za-z0-9._-]*) return 1 ;; esac
      echo "https://github.com/$GH/releases/download/$tag" ;;
    base) echo "$BASE/dl" ;;
  esac
}

# 参数：check（只看）、update（不带配对码）、pair。子 shell：manifest 写的 VER / FILE / WANT 不覆盖 lolinet 的
install_lolidesk() (
  if [ $SRC = github ]; then order="github base"; else order=base; fi
  DREL=
  for S in $order; do
    d=$(desk_dir $S) || continue
    if dl "$d/lolidesk.json" -o "$TMP/lolidesk.json" 2>/dev/null && manifest "$TMP/lolidesk.json" lolidesk; then DREL=$d; break; fi
  done
  [ -n "$DREL" ] || { echo "跳过 lolidesk：没找到发布的 lolidesk.json"; return 0; }
  DD=/usr/local/lib/lolidesk
  LIB=libghostty-vt.so; [ $OS = macos ] && LIB=libghostty-vt.dylib
  NEED=
  [ "$("$DD/lolidesk" --version 2>/dev/null)" = "lolidesk $VER" ] || NEED=1
  for b in lolidesk-host lolidesk; do
    [ -f "/usr/local/bin/$b" ] && [ ! -L "/usr/local/bin/$b" ] && NEED=1
  done
  if [ $1 = check ]; then
    if [ -z "$NEED" ]; then echo "lolidesk  ${VER}，本机已是这一版"; else echo "lolidesk  ${VER}，本机要装或更新（curl -fsSL $BASE/i | sudo sh）"; fi
    return 0
  fi
  [ -n "$NEED" ] || { echo "lolidesk $VER 已是最新"; return 0; }
  dl "$DREL/$FILE" -o "$TMP/lolidesk.tgz" || { echo "跳过 lolidesk：下载 $FILE 失败或太慢"; return 0; }
  [ "$(sha < "$TMP/lolidesk.tgz")" = "$WANT" ] || { echo "跳过 lolidesk：$FILE 的 sha256 和 lolidesk.json 不一致"; return 0; }
  mkdir -p "$TMP/desk" && tar -xzf "$TMP/lolidesk.tgz" -C "$TMP/desk" lolidesk "$LIB" || { echo "跳过 lolidesk：$FILE 解压失败"; return 0; }
  [ "$("$TMP/desk/lolidesk" --version 2>/dev/null)" = "lolidesk $VER" ] || { echo "跳过 lolidesk：这台机器上跑不起来"; return 0; }
  # 先库后程序；/usr/local/bin 下的名字都换成链接（临时链接再改名）
  { install -d -m 755 "$DD" /usr/local/bin && install -m 644 "$TMP/desk/$LIB" "$DD/$LIB" && install -m 755 "$TMP/desk/lolidesk" "$DD/lolidesk"; } \
    || { echo "跳过 lolidesk：装不进 $DD"; return 0; }
  for b in lolidesk lolidesk-host; do
    if [ $b = lolidesk ] || [ -e "/usr/local/bin/$b" ]; then
      ln -sf "$DD/lolidesk" "/usr/local/bin/.$b.lolinet" && mv -f "/usr/local/bin/.$b.lolinet" "/usr/local/bin/$b"
    fi
  done
  echo "已装 lolidesk ${VER}（$DD，/usr/local/bin/lolidesk；之后 lolinet upgrade 跟着更新）"
  if [ $1 = update ]; then
    echo "  在跑的主机下次启动才用新版本"
    return 0
  fi
  echo "  连别的机器：lolidesk <地址> --token <token>"
  if [ $OS = macos ]; then
    echo "  让这台能被连（不用 sudo）：lolidesk host token 看 token，lolidesk host install 装成当前用户的常驻服务"
  else
    echo "  让这台能被连（不用 sudo）：lolidesk host token 看 token，lolidesk host run 起主机"
  fi
)

if [ $MODE = check ]; then
  if [ "$NEW" = "$BIN" ]; then got="本机已是这一版，没下载"; else got="下载和 sha256 都对"; fi
  echo "最新      lolinet ${VER}（${FROM}，${got}）"
  if [ -n "$CUR" ]; then echo "本机      lolinet $CUR"; else echo "本机      没装 lolinet"; fi
  if [ -z "$HAVE_CONF" ]; then echo "入网      没看到 /etc/lolinet/loli.toml（以前的装法要 sudo 才看得到）"; fi
  if [ "$VER" != "$CUR" ]; then echo "要装：     curl -fsSL $BASE/i | sudo sh"
  elif ! installed_ok; then echo "版本是最新，但服务、自动升级或公开探针没装好：curl -fsSL $BASE/i | sudo sh"
  else echo "已是最新，服务、自动升级和公开探针（配了的话）都在，不用动"; fi
  install_lolidesk check
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
  install_lolidesk update
  exit 0
fi

# 配对：lolinet 已经下好、校验过，再兑换（下载失败不会白用掉配对码）
ID=$(printf %s "$CODE" | sha | cut -c1-32)
curl -fsS "$BASE/v1/pair/$ID" -o "$TMP/blob" || { echo "配对码无效、已使用或已过期"; exit 1; }
K=$CODE openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -md sha256 -a -A -pass env:K -in "$TMP/blob" -out "$TMP/pair.toml" \
  || { echo "解密失败：配对码不对"; exit 1; }
# pair.py 把探针配置（公开 id + 上报密钥）写在末尾的 `# probe = {...}` 一行：整份交给 lolinet up，它把这一行存到
# /etc/lolinet/probe.json、装成系统服务（lolinet 内置，不要 python3），不留在 EasyTier 配置里
up -c "$TMP/pair.toml"
echo "已接入 lolinet 网络（lolinet ${VER}，${FROM}）。看状态：lolinet；日志：lolinet logs -f"
if ! grep -q '^# probe = ' "$TMP/pair.toml"; then
  echo "这台不装公开探针（配对时选了不装）。"
elif ! "$NEW" probe --help >/dev/null 2>&1; then
  # 下到的 lolinet 比这个脚本旧（比如准入站 dl/ 还没同步）：探针配置先放好，自动升级到带探针的版本后自己装上
  (umask 077; sed -n 's/^# probe = //p' "$TMP/pair.toml" > /etc/lolinet/probe.json)
  echo "这一版 lolinet（${VER}）还没有内置探针：探针配置已存好，自动升级后自己装上。"
fi

install_lolidesk pair
