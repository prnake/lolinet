#!/bin/sh
# 下载 mihomo（MetaCubeX/mihomo 的 Release，版本和 sha256 钉在 desk/mihomo/mihomo.json），核对 .gz 的 sha256，
# 解压成 <目标目录>/mihomo（755）。目标目录里已经是这一版就不下。Lolidesk.app 带着它，lolinet mihomo on 用。
#
# 用法：desk/scripts/fetch-mihomo.sh <darwin-arm64|darwin-amd64|linux-amd64|linux-arm64> <目标目录>
# 成功时打印 mihomo 的路径；sha256 不对、下载失败都返回非零。
set -eu

usage() {
	echo "用法：${0} <darwin-arm64|darwin-amd64|linux-amd64|linux-arm64> <目标目录>" >&2
	exit 2
}

[ $# -eq 2 ] || usage
platform=$1
dest=$2
case $platform in
darwin-arm64 | darwin-amd64 | linux-amd64 | linux-arm64) ;;
*) usage ;;
esac

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
manifest=$here/../mihomo/mihomo.json
[ -f "$manifest" ] || {
	echo "找不到清单 ${manifest}" >&2
	exit 1
}

# 读清单：一行“版本 地址 sha256”。python3 优先，没有就用 jq。
if command -v python3 >/dev/null 2>&1; then
	entry=$(python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1]))
f = m["files"][sys.argv[2]]
print(m["version"], f["url"], f["sha256"])
' "$manifest" "$platform")
elif command -v jq >/dev/null 2>&1; then
	entry=$(jq -er --arg p "$platform" '"\(.version) \(.files[$p].url) \(.files[$p].sha256)"' "$manifest")
else
	echo "要 python3 或 jq 来读 ${manifest}" >&2
	exit 1
fi
# shellcheck disable=SC2086 # 三个字段都不含空格
set -- $entry
[ $# -eq 3 ] || {
	echo "清单里 ${platform} 那项不对：${entry}" >&2
	exit 1
}
version=$1
url=$2
want=$3

sha256() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	else
		shasum -a 256 "$1" | cut -d' ' -f1
	fi
}

mkdir -p "$dest"
out=$dest/mihomo
stamp=$dest/.mihomo.sha256
# 上次解出来的是这一版（记着 .gz 的 sha256）就不再下
if [ -x "$out" ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ]; then
	echo "$out"
	exit 0
fi

tmp=$(mktemp "$dest/.mihomo.gz.XXXXXX")
bin=$(mktemp "$dest/.mihomo.XXXXXX")
# 出错退出时也删掉临时文件，退出码不变。
trap 'st=$?; rm -f "$tmp" "$bin"; exit $st' EXIT
curl -fsSL --retry 3 -o "$tmp" "$url" || {
	echo "下载 ${url} 失败" >&2
	exit 1
}
got=$(sha256 "$tmp")
if [ "$got" != "$want" ]; then
	echo "mihomo ${version} 的 sha256 对不上：应该是 ${want}，下载到的是 ${got}（${url}）" >&2
	exit 1
fi
gzip -dc "$tmp" >"$bin"
chmod 755 "$bin"
mv -f "$bin" "$out"
rm -f "$tmp"
printf '%s\n' "$want" >"$stamp"
trap - EXIT
echo "$out"
