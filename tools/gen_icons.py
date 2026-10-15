"""Generate the PWA icons into assets/. Run: python tools/gen_icons.py (needs Pillow)."""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent.parent / "assets"
BIG = 1024
A1, A2 = (137, 180, 250), (203, 166, 247)  # Catppuccin blue -> mauve (Mocha)
BASE = (30, 30, 46)  # Catppuccin Mocha base, used for the glyph


def gradient(size):
    # diagonal gradient from the top-right (light) to the bottom-left (dark)
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            t = ((size - x) + y) / (2 * size)
            px[x, y] = tuple(round(a + (b - a) * t) for a, b in zip(A1, A2))
    return img


def glyph(size):
    """Mouse outline (Catppuccin base color) inside the central ~56% (maskable safe zone)."""
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    w, h = size * 0.34, size * 0.52
    x0, y0 = (size - w) / 2, (size - h) / 2
    x1, y1 = x0 + w, y0 + h
    stroke = round(size * 0.035)
    d.rounded_rectangle((x0, y0, x1, y1), radius=w / 2, outline=BASE, width=stroke)
    mid = (x0 + x1) / 2
    split = y0 + h * 0.42
    d.line((x0 + stroke / 2, split, x1 - stroke / 2, split), fill=BASE, width=stroke)
    d.line((mid, y0 + stroke, mid, split), fill=BASE, width=stroke)
    ww, wh = w * 0.10, h * 0.13  # scroll wheel
    d.rounded_rectangle((mid - ww, y0 + h * 0.12, mid + ww, y0 + h * 0.12 + wh), radius=ww, fill=BASE)
    shadow = layer.filter(ImageFilter.GaussianBlur(size * 0.012))
    return shadow, layer


def build():
    base = gradient(BIG).convert("RGBA")
    glow = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse((-BIG * 0.2, BIG * 0.5, BIG * 0.7, BIG * 1.3), fill=(255, 255, 255, 60))
    base.alpha_composite(glow.filter(ImageFilter.GaussianBlur(BIG * 0.08)))
    _, g = glyph(BIG)
    base.alpha_composite(g)
    return base.convert("RGB")


if __name__ == "__main__":
    big = build()
    # raw RGBA (32x32) for the tray icon, included with include_bytes!
    (OUT / "tray-32.rgba").write_bytes(big.resize((32, 32), Image.LANCZOS).convert("RGBA").tobytes())
    print("wrote", OUT / "tray-32.rgba")
    for n in (180, 192, 512):
        big.resize((n, n), Image.LANCZOS).save(OUT / f"icon-{n}.png", optimize=True)
        print("wrote", OUT / f"icon-{n}.png")
