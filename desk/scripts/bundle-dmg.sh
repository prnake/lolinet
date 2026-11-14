#!/bin/sh
# 把 Lolidesk.app 做成 DMG：打开是一个窗口，左边 Lolidesk，右边 Applications，拖过去就装好了。
#   sh desk/scripts/bundle-dmg.sh <Lolidesk.app> <输出的 .dmg>
# Finder 的窗口布局和背景由 dmgbuild 直接写进 .DS_Store（不开 Finder，CI 上也能做）。dmgbuild 装在单独的虚拟环境里，
# 版本和 sha256 钉在 desk/scripts/dmgbuild.txt；要 uv（uv 自己取 Python 3.12）。DMGBUILD_VENV 可以指定虚拟环境放哪。
# app 是 ad-hoc 签名，DMG 不另签：从浏览器下载的，第一次打开要到「系统设置 → 隐私与安全性」点「仍要打开」（背景图上写着）。
set -eu
app=${1:?要 Lolidesk.app}
out=${2:?要输出的 .dmg}
[ -x "$app/Contents/MacOS/Lolidesk" ] || { echo "没有 ${app}（先跑 desk/scripts/bundle-app.sh）" >&2; exit 1; }
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
res="$here/../app/macos"
key=$(shasum -a 256 "$here/dmgbuild.txt" | cut -c1-12)
venv=${DMGBUILD_VENV:-${TMPDIR:-/tmp}/lolidesk-dmgbuild-$key}
if [ ! -x "$venv/bin/dmgbuild" ]; then
  command -v uv >/dev/null || { echo "要 uv（https://docs.astral.sh/uv/）" >&2; exit 1; }
  uv venv -q --python 3.12 "$venv"
  uv pip install -q --python "$venv/bin/python" --require-hashes -r "$here/dmgbuild.txt"
fi
mkdir -p "$(dirname -- "$out")"
rm -f "$out"
"$venv/bin/dmgbuild" -s "$here/dmg-settings.py" -D app="$app" -D background="$res/dmg-background.png" \
  -D icon="$res/AppIcon.icns" Lolidesk "$out"
hdiutil verify -quiet "$out"
echo "$out"
