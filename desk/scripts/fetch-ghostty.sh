#!/bin/sh
# 下载 libghostty-vt（mygo 预编译的 Ghostty 终端核心，见 desk/ghostty/libghostty-vt.json），
# 核对 sha256，放进目标目录。目标目录里已经有对的就不下。
#
# 用法：desk/scripts/fetch-ghostty.sh <darwin-arm64|darwin-amd64|linux-amd64|linux-arm64> <目标目录>
# 成功时打印库的路径；sha256 不对、下载失败都返回非零。
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
manifest=$here/../ghostty/libghostty-vt.json
[ -f "$manifest" ] || {
	echo "找不到清单 ${manifest}" >&2
	exit 1
}

# 读清单：一行“文件名 地址 sha256”。python3 优先，没有就用 jq。
if command -v python3 >/dev/null 2>&1; then
	entry=$(python3 -I -c '
import json, sys
f = json.load(open(sys.argv[1]))["files"][sys.argv[2]]
print(f["name"], f["url"], f["sha256"])
' "$manifest" "$platform")
elif command -v jq >/dev/null 2>&1; then
	entry=$(jq -er --arg p "$platform" '.files[$p] | "\(.name) \(.url) \(.sha256)"' "$manifest")
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
name=$1
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
out=$dest/$name
if [ -f "$out" ] && [ "$(sha256 "$out")" = "$want" ]; then
	echo "$out"
	exit 0
fi

tmp=$(mktemp "$dest/.$name.XXXXXX")
# 出错退出时也删掉临时文件，退出码不变。
trap 'st=$?; rm -f "$tmp"; exit $st' EXIT
curl -fsSL --retry 3 -o "$tmp" "$url" || {
	echo "下载 ${url} 失败" >&2
	exit 1
}
got=$(sha256 "$tmp")
if [ "$got" != "$want" ]; then
	echo "${name} 的 sha256 对不上：应该是 ${want}，下载到的是 ${got}（${url}）" >&2
	exit 1
fi
chmod 644 "$tmp"
mv -f "$tmp" "$out"
trap - EXIT
echo "$out"
