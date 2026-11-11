#!/bin/sh
# 把编好的 lolidesk-app、lolidesk 和 libghostty-vt.dylib 装成 Lolidesk.app（ad-hoc 签名）。
#   sh desk/scripts/bundle-app.sh <lolidesk-app 可执行文件> <lolidesk 可执行文件> <libghostty-vt.dylib> <输出目录> [版本]
# 输出 <输出目录>/Lolidesk.app：
#   Contents/MacOS/Lolidesk                    界面
#   Contents/Helpers/lolidesk                  命令行（连别人、lolidesk host 让这台能被连），和包里单独那个一样。
#                                              不能放 MacOS/：APFS 默认不分大小写，lolidesk 会覆盖掉 Lolidesk
#   Contents/Frameworks/libghostty-vt.dylib    终端核心（两个程序都从这里载入）
#   Contents/Helpers/mihomo                    设了 MIHOMO=<mihomo 可执行文件> 才有（desk/scripts/fetch-mihomo.sh 取）：
#                                              独立的程序，lolinet mihomo on 用；旁边 Resources/mihomo-NOTICE.txt 写版本、许可和源码
#   Contents/Info.plist                        登记 lolidesk:// （分享链接）：点了链接，系统把它交给 app
set -eu

bin=${1:?要 lolidesk-app 可执行文件}
cli=${2:?要 lolidesk 可执行文件}
lib=${3:?要 libghostty-vt.dylib}
out=${4:?要输出目录}
version=${5:-0.2.0}

[ -f "$bin" ] || { echo "没有 $bin" >&2; exit 1; }
[ -f "$cli" ] || { echo "没有 $cli" >&2; exit 1; }
[ -f "$lib" ] || { echo "没有 ${lib}（先跑 desk/scripts/fetch-ghostty.sh）" >&2; exit 1; }
mihomo=${MIHOMO:-}
[ -z "$mihomo" ] || [ -f "$mihomo" ] || { echo "没有 ${mihomo}（先跑 desk/scripts/fetch-mihomo.sh）" >&2; exit 1; }
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

app="$out/Lolidesk.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Frameworks" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/Lolidesk"
cp "$cli" "$app/Contents/Helpers/lolidesk"
cp "$lib" "$app/Contents/Frameworks/libghostty-vt.dylib"
chmod 755 "$app/Contents/MacOS/Lolidesk" "$app/Contents/Helpers/lolidesk"
if [ -n "$mihomo" ]; then
  cp "$mihomo" "$app/Contents/Helpers/mihomo"
  chmod 755 "$app/Contents/Helpers/mihomo"
  # mihomo 是 GPL-3.0：原样带着（独立的程序，不链接），写清版本和对应的源码在哪
  python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1]))
print("Contents/Helpers/mihomo 是 mihomo %s（MetaCubeX/mihomo），按 %s 发布，没有改过。" % (m["version"], m["license"]))
print("对应的源码：%s" % m["source"])
print("许可证全文：https://www.gnu.org/licenses/gpl-3.0.html")
print("Lolidesk 本身不链接它，只在 lolinet mihomo on 时作为单独的程序运行。")
' "$here/../mihomo/mihomo.json" > "$app/Contents/Resources/mihomo-NOTICE.txt"
fi
# 签名前查一下两个程序都在：界面是 MacOS/Lolidesk，命令行是 Helpers/lolidesk
[ "$(cmp -s "$bin" "$app/Contents/MacOS/Lolidesk" && echo same)" = same ] || { echo "MacOS/Lolidesk 不是界面程序" >&2; exit 1; }

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Lolidesk</string>
  <key>CFBundleDisplayName</key><string>Lolidesk</string>
  <key>CFBundleExecutable</key><string>Lolidesk</string>
  <key>CFBundleIdentifier</key><string>co.lo-li.lolidesk.app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>co.lo-li.lolidesk.pair</string>
      <key>CFBundleURLSchemes</key><array><string>lolidesk</string></array>
    </dict>
  </array>
</dict>
</plist>
EOF

# 先签里面的库和命令行，再签整个 app（ad-hoc：第一次打开要右键 → 打开）
codesign --force --sign - "$app/Contents/Frameworks/libghostty-vt.dylib"
codesign --force --sign - --identifier co.lo-li.lolidesk.cli "$app/Contents/Helpers/lolidesk"
[ -z "$mihomo" ] || codesign --force --sign - --identifier co.lo-li.lolidesk.mihomo "$app/Contents/Helpers/mihomo"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
echo "$app"
