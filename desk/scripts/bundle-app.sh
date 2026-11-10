#!/bin/sh
# 把编好的 lolidesk-app、lolidesk 和 libghostty-vt.dylib 装成 Lolidesk.app（ad-hoc 签名）。
#   sh desk/scripts/bundle-app.sh <lolidesk-app 可执行文件> <lolidesk 可执行文件> <libghostty-vt.dylib> <输出目录> [版本]
# 输出 <输出目录>/Lolidesk.app：
#   Contents/MacOS/Lolidesk                    界面
#   Contents/Helpers/lolidesk                  命令行（连别人、lolidesk host 让这台能被连），和包里单独那个一样。
#                                              不能放 MacOS/：APFS 默认不分大小写，lolidesk 会覆盖掉 Lolidesk
#   Contents/Frameworks/libghostty-vt.dylib    终端核心（两个程序都从这里载入）
#   Contents/Info.plist
set -eu

bin=${1:?要 lolidesk-app 可执行文件}
cli=${2:?要 lolidesk 可执行文件}
lib=${3:?要 libghostty-vt.dylib}
out=${4:?要输出目录}
version=${5:-0.2.0}

[ -f "$bin" ] || { echo "没有 $bin" >&2; exit 1; }
[ -f "$cli" ] || { echo "没有 $cli" >&2; exit 1; }
[ -f "$lib" ] || { echo "没有 $lib（先跑 desk/scripts/fetch-ghostty.sh）" >&2; exit 1; }

app="$out/Lolidesk.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Frameworks" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/Lolidesk"
cp "$cli" "$app/Contents/Helpers/lolidesk"
cp "$lib" "$app/Contents/Frameworks/libghostty-vt.dylib"
chmod 755 "$app/Contents/MacOS/Lolidesk" "$app/Contents/Helpers/lolidesk"
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
</dict>
</plist>
EOF

# 先签里面的库和命令行，再签整个 app（ad-hoc：第一次打开要右键 → 打开）
codesign --force --sign - "$app/Contents/Frameworks/libghostty-vt.dylib"
codesign --force --sign - --identifier co.lo-li.lolidesk.cli "$app/Contents/Helpers/lolidesk"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
echo "$app"
