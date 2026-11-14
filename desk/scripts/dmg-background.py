# /// script
# requires-python = ">=3.10,<3.14"
# dependencies = ["pillow==11.3.0"]
# ///
"""画 DMG 窗口的背景：desk/app/macos/dmg-background.png（680×420）和 @2x。
  uv run desk/scripts/dmg-background.py
只在 macOS 上跑（中文用系统的冠黑体 Hiragino Sans GB）。画好的图提交进仓库，打包时（bundle-dmg.sh）不再画。
布局和 desk/scripts/dmg-settings.py 对得上：Lolidesk.app 在 (340, 215)，Applications 在 (570, 215)，图标 112。
图标下面各垫一块中灰的圆角方块：Finder 的文件名浅色外观是黑字、深色外观是白字，这个灰上两种都看得清。
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "app" / "macos"
MASCOT = HERE.parent / "app" / "assets" / "mascot" / "hero.png"
FONT = "/System/Library/Fonts/Hiragino Sans GB.ttc"
W, H = 680, 420
APP, APPS = (340, 215), (570, 215)
INK, VOID, FG, FG3, EYE, SLOT, SLOT_LINE = "#0a0a0c", "#000000", "#f2f2f3", "#9a9aa0", "#ceff1c", "#707077", "#85858c"


def draw(scale: int) -> Image.Image:
    s = lambda v: round(v * scale)  # noqa: E731
    im = Image.new("RGBA", (s(W), s(H)), INK)
    # 箭头附近一点很淡的猫眼绿
    glow = Image.new("RGBA", im.size, (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse((s(330), s(110), s(580), s(320)), fill=(206, 255, 28, 22))
    im.alpha_composite(glow.filter(ImageFilter.GaussianBlur(s(60))))
    d = ImageDraw.Draw(im)
    bold = ImageFont.truetype(FONT, s(20), index=1)
    small = ImageFont.truetype(FONT, s(12), index=0)
    d.text((s(250), s(40)), "把 Lolidesk 拖到 Applications", font=bold, fill=FG)
    d.text((s(250), s(72)), "拖过去就装好了，以后从「应用程序」或启动台打开。", font=small, fill=FG3)
    for cx, cy in (APP, APPS):
        d.rounded_rectangle((s(cx - 78), s(cy - 82), s(cx + 78), s(cy + 90)), radius=s(22), fill=SLOT, outline=SLOT_LINE, width=s(1))
    # 箭头
    y, x0, x1 = s(APP[1]), s(432), s(478)
    d.line((x0, y, x1 - s(4), y), fill=EYE, width=s(4))
    d.polygon([(x1 + s(4), y), (x1 - s(8), y - s(9)), (x1 - s(8), y + s(9))], fill=EYE)
    d.text((s(250), s(344)), "第一次打开时 macOS 可能会拦住它（这一版还没有 Apple 的公证）：", font=small, fill=FG3)
    d.text((s(250), s(366)), "系统设置 → 隐私与安全性 → 往下找到 Lolidesk → 仍要打开", font=small, fill=FG)
    # 看板娘站在左边
    girl = Image.open(MASCOT).convert("RGBA")
    h = s(300)
    girl = girl.resize((round(girl.width * h / girl.height), h), Image.LANCZOS)
    im.alpha_composite(girl, (s(26), s(H - 12) - h))
    return im.convert("RGB")


OUT.mkdir(parents=True, exist_ok=True)
draw(1).save(OUT / "dmg-background.png", optimize=True)
draw(2).save(OUT / "dmg-background@2x.png", optimize=True)
print(OUT / "dmg-background.png")
