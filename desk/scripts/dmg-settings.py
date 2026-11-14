# dmgbuild 的设置：desk/scripts/bundle-dmg.sh 传 -D app=<Lolidesk.app> -D background=<png> -D icon=<icns>。
# 坐标要和背景图 desk/app/macos/dmg-background.png（desk/scripts/dmg-background.py 画的）对得上：
# 窗口 680×420，Lolidesk.app 在 (340, 215)，Applications 在 (570, 215)，图标 112。
import os.path

app = defines["app"]  # noqa: F821（dmgbuild 执行这个文件时给的）
name = os.path.basename(app)

format = "ULMO"  # lzma，macOS 10.15 起都能开（app 要 13）
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines["icon"]  # noqa: F821
background = defines["background"]  # noqa: F821（旁边的 @2x 会被合成一张 Retina 的 TIFF）

window_rect = ((200, 160), (680, 420))
default_view = "icon-view"
icon_size = 112
text_size = 13
icon_locations = {name: (340, 215), "Applications": (570, 215)}
# 不设 hide_extensions：它在 app 上写 FinderInfo，拖出去的 app 过不了 codesign --verify --strict（Finder 本来就不显示 .app）
show_icon_preview = False
