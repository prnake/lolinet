#!/bin/sh
# Linux 的图形客户端放进包目录（包根目录已经有 lolidesk、libghostty-vt.so、BUILD-INFO）：
#   sh desk/scripts/bundle-linux.sh <lolidesk-app 可执行文件> <包目录>
# 加上：
#   lolidesk-app        界面（从包目录载入旁边的 libghostty-vt.so；旁边有 BUILD-INFO 和 lolidesk 才认这是包，
#                       网络页的「安装到命令行」把这个 lolidesk 链到 /usr/local/bin/lolidesk）
#   lolidesk.desktop    应用菜单的模板（desk/app/linux/lolidesk.desktop）；网络页的「加到应用菜单」
#                       把它写到 ~/.local/share/applications/，Exec、Icon 换成包里的绝对路径
#   lolidesk.svg        图标（docs/assets/logo.svg）
set -eu

bin=${1:?要 lolidesk-app 可执行文件}
out=${2:?要包目录}

[ -f "$bin" ] || { echo "没有 $bin" >&2; exit 1; }
[ -d "$out" ] || { echo "没有包目录 $out" >&2; exit 1; }
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

cp "$bin" "$out/lolidesk-app"
chmod 755 "$out/lolidesk-app"
cp "$here/../app/linux/lolidesk.desktop" "$out/lolidesk.desktop"
cp "$here/../../docs/assets/logo.svg" "$out/lolidesk.svg"
chmod 644 "$out/lolidesk.desktop" "$out/lolidesk.svg"
# 没有 BUILD-INFO 或 lolidesk 的话，app 不把这个目录当包（装不了命令行和应用菜单）
for f in lolidesk BUILD-INFO; do
  [ -f "$out/$f" ] || echo "注意：$out 里还没有 $f" >&2
done
echo "${out}：lolidesk-app、lolidesk.desktop、lolidesk.svg"
