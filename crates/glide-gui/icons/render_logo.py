"""Rendert das Glide-Logo. Geometrie identisch mit icons/logo.svg und icons/logo-small.svg.

Aufruf (aus dem Workspace-Wurzelverzeichnis, braucht Pillow):
    python crates/glide-gui/icons/render_logo.py crates/glide-gui/icons docs/assets crates/glide-gui/ui
"""
import os
import sys

from PIL import Image, ImageChops, ImageDraw

SS = 4          # Überabtastung
W = 512 * SS    # Arbeitsgröße in Pixeln


def hexc(h, a=255):
    h = h.lstrip("#")
    return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16), a)


def s(v):
    return v * SS


def stop_color(stops, t):
    t = max(0.0, min(1.0, t))
    for (o0, c0), (o1, c1) in zip(stops, stops[1:]):
        if t <= o1:
            u = 0 if o1 == o0 else (t - o0) / (o1 - o0)
            return tuple(int(round(c0[i] + (c1[i] - c0[i]) * u)) for i in range(4))
    return stops[-1][1]


def gradient(stops, value_at):
    """RGBA-Ebene; value_at(x, y) liefert die Position 0..1 im Verlauf (Arbeitspixel)."""
    lut = [stop_color(stops, i / 1023) for i in range(1024)]
    small = Image.new("RGBA", (W // 4, W // 4))
    px = small.load()
    for y in range(W // 4):
        for x in range(W // 4):
            t = value_at(x * 4 + 2, y * 4 + 2)
            px[x, y] = lut[int(max(0.0, min(1.0, t)) * 1023)]
    return small.resize((W, W), Image.BILINEAR)


def linear(p0, p1, stops):
    x0, y0, x1, y1 = s(p0[0]), s(p0[1]), s(p1[0]), s(p1[1])
    dx, dy = x1 - x0, y1 - y0
    length = dx * dx + dy * dy
    return gradient(stops, lambda x, y: ((x - x0) * dx + (y - y0) * dy) / length)


def radial(center, r, stops):
    cx, cy, rr = s(center[0]), s(center[1]), s(r)
    return gradient(stops, lambda x, y: ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5 / rr)


def masked(layer, mask):
    r, g, b, a = layer.split()
    return Image.merge("RGBA", (r, g, b, ImageChops.multiply(a, mask)))


def solid(color, mask):
    return masked(Image.new("RGBA", (W, W), color), mask)


def new_mask():
    m = Image.new("L", (W, W), 0)
    return m, ImageDraw.Draw(m)


def rrect(x, y, w, h, rx, outline=None):
    m, d = new_mask()
    if outline:
        # SVG-Kontur liegt mittig auf der Kante
        o = outline / 2
        box = (s(x - o), s(y - o), s(x + w + o), s(y + h + o))
        d.rounded_rectangle(box, radius=s(rx + o), outline=255, width=int(s(outline)))
    else:
        d.rounded_rectangle((s(x), s(y), s(x + w), s(y + h)), radius=s(rx), fill=255)
    return m


def brush(points, width):
    """Dicke Linie mit runden Enden: dicht gesetzte Kreise entlang der Punkte (glatter als ImageDraw.line)."""
    m, d = new_mask()
    r = s(width) / 2
    for x, y in points:
        x, y = s(x), s(y)
        d.ellipse((x - r, y - r, x + r, y + r), fill=255)
    return m


def polyline(points, width, closed=False):
    m, d = new_mask()
    pts = [(s(x), s(y)) for x, y in points]
    if closed:
        pts.append(pts[0])
    d.line(pts, fill=255, width=int(s(width)), joint="curve")
    r = s(width) / 2
    for x, y in (pts if closed else [pts[0], pts[-1]]):
        d.ellipse((x - r, y - r, x + r, y + r), fill=255)
    return m


def polygon(points):
    m, d = new_mask()
    d.polygon([(s(x), s(y)) for x, y in points], fill=255)
    return m


def bezier(p0, p1, p2, p3, n=2400):
    pts = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        pts.append(tuple(u**3 * p0[k] + 3 * u * u * t * p1[k] + 3 * u * t * t * p2[k] + t**3 * p3[k] for k in (0, 1)))
    return pts


def cursor(img, points, fill, stroke, width):
    img.alpha_composite(solid(hexc(fill), polygon(points)))
    img.alpha_composite(solid(hexc(stroke), polyline(points, width, closed=True)))


def full():
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    bg = linear((0, 8), (0, 504), [(0, hexc("#262f5a")), (1, hexc("#12172a"))])
    img.alpha_composite(masked(bg, rrect(8, 8, 496, 496, 108)))
    img.alpha_composite(solid(hexc("#5a66a0", 89), rrect(9.5, 9.5, 493, 493, 106.5, outline=3)))
    for x in (62, 278):
        img.alpha_composite(solid(hexc("#3a4470"), rrect(x, 176, 172, 120, 20, outline=12)))
    for x in (128, 344):
        img.alpha_composite(solid(hexc("#3a4470"), rrect(x, 306, 40, 12, 4)))
    img.alpha_composite(radial((340, 250), 120, [(0, hexc("#3fe3c6", 89)), (1, hexc("#3fe3c6", 0))]))
    trail = linear((96, 300), (330, 200), [(0, hexc("#8c8cff", 0)), (0.4, hexc("#8c8cff")), (1, hexc("#3fe3c6"))])
    img.alpha_composite(masked(trail, brush(bezier((104, 268), (150, 150), (270, 118), (322, 206)), 26)))
    cursor(img, [(318, 196), (318, 316), (346, 290), (366, 334), (386, 325), (366, 282), (404, 282)], "#3fe3c6", "#12172a", 10)
    return img


def small():
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    bg = linear((0, 0), (0, 512), [(0, hexc("#2a3462")), (1, hexc("#151a31"))])
    img.alpha_composite(masked(bg, rrect(0, 0, 512, 512, 96)))
    trail = linear((70, 390), (300, 170), [(0, hexc("#8c8cff", 64)), (0.35, hexc("#8c8cff")), (1, hexc("#3fe3c6"))])
    img.alpha_composite(masked(trail, brush(bezier((84, 392), (120, 210), (220, 140), (296, 190)), 64)))
    cursor(img, [(270, 150), (270, 420), (332, 362), (376, 458), (422, 438), (378, 344), (462, 344)], "#3fe3c6", "#151a31", 22)
    return img


def down(img, size):
    return img.resize((size, size), Image.LANCZOS)


def main():
    out_icons, out_docs, out_ui = sys.argv[1:4]
    big, sm = full(), small()
    sizes = {n: down(sm, n) for n in (16, 20, 24, 32, 40, 48, 64)}
    sizes.update({n: down(big, n) for n in (128, 256, 512)})

    sizes[32].save(os.path.join(out_icons, "32x32.png"))
    sizes[128].save(os.path.join(out_icons, "128x128.png"))
    sizes[512].save(os.path.join(out_icons, "icon.png"))
    sizes[256].save(os.path.join(out_docs, "logo.png"))
    sizes[64].save(os.path.join(out_ui, "icon.png"))  # klein dargestellt: vereinfachte Variante
    ico = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    sizes[256].save(os.path.join(out_icons, "icon.ico"), sizes=[(n, n) for n in ico],
                    append_images=[sizes[n] for n in ico if n != 256])

    print("ok")


if __name__ == "__main__":
    main()
