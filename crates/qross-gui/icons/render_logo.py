"""Erzeugt das Qross-Logo: SVG-Quellen und alle PNG/ICO-Größen aus derselben Geometrie.

Die Spur ist kein einfacher Strich, sondern eine Fläche, die sich zur Zeigerspitze hin verjüngt
und genau dort endet. SVG kennt keine variable Strichbreite, deshalb schreibt dieses Skript auch
die SVG-Dateien.

Aufruf (aus dem Workspace-Wurzelverzeichnis, braucht Pillow):
    python crates/qross-gui/icons/render_logo.py crates/qross-gui/icons docs/assets crates/qross-gui/ui
"""
import math
import os
import sys

from PIL import Image, ImageChops, ImageDraw

SS = 4          # Überabtastung
W = 512 * SS    # Arbeitsgröße in Pixeln

# ------------------------------------------------------------------ Geometrie (SVG-Koordinaten 0..512)

# Volles Logo: zwei Bildschirme, Spur vom linken zum rechten, Zeiger auf dem rechten
FULL_TIP = (318, 196)
FULL = {
    "cursor": [FULL_TIP, (318, 316), (346, 290), (366, 334), (386, 325), (366, 282), (404, 282)],
    "cursor_stroke": 10,
    "ink": "#12172a",
    # Bézier-Kurve: endet genau an der Zeigerspitze, kommt von oben links
    "trail": ((104, 268), (150, 150), (254, 148), FULL_TIP),
    "trail_width": 26,
    "trail_gradient": ((96, 300), FULL_TIP, [(0, "#8c8cff", 0), (0.4, "#8c8cff", 1), (1, "#3fe3c6", 1)]),
}

# Vereinfachte Variante für 16–64 px: nur Spur und Zeiger, kräftiger
SMALL_TIP = (296, 206)
SMALL = {
    "cursor": [SMALL_TIP, (296, 435.5), (348.7, 386.2), (386.1, 467.8), (425.2, 450.8), (387.8, 370.9), (459.2, 370.9)],
    "cursor_stroke": 20,
    "ink": "#151a31",
    "trail": ((72, 352), (92, 180), (208, 140), SMALL_TIP),
    "trail_width": 60,
    "trail_gradient": ((72, 352), SMALL_TIP, [(0, "#8c8cff", 0.25), (0.35, "#8c8cff", 1), (1, "#3fe3c6", 1)]),
}

TAPER_FROM = 0.5    # ab hier wird die Spur schmaler
TAPER_END = 0.3     # Breite an der Spitze relativ zur vollen Breite


def bezier_point(c, t):
    u = 1 - t
    return tuple(u**3 * c[0][k] + 3 * u * u * t * c[1][k] + 3 * u * t * t * c[2][k] + t**3 * c[3][k] for k in (0, 1))


def bezier_tangent(c, t):
    u = 1 - t
    return tuple(
        3 * u * u * (c[1][k] - c[0][k]) + 6 * u * t * (c[2][k] - c[1][k]) + 3 * t * t * (c[3][k] - c[2][k]) for k in (0, 1)
    )


def trail_width(width, t):
    if t <= TAPER_FROM:
        return width
    x = (t - TAPER_FROM) / (1 - TAPER_FROM)
    smooth = x * x * (3 - 2 * x)
    return width * (1 - (1 - TAPER_END) * smooth)


def trail_outline(curve, width, n=160, cap=20):
    """Umriss der Spur als Polygon: linke Kante, runde Kappe an der Spitze, rechte Kante, runde Kappe am Anfang."""
    left, right, normals = [], [], []
    for i in range(n + 1):
        t = i / n
        x, y = bezier_point(curve, t)
        dx, dy = bezier_tangent(curve, t)
        length = math.hypot(dx, dy)
        nx, ny = -dy / length, dx / length
        r = trail_width(width, t) / 2
        left.append((x + nx * r, y + ny * r))
        right.append((x - nx * r, y - ny * r))
        normals.append((nx, ny, r))

    def arc(center, nx, ny, r, start, sweep):
        a0 = math.atan2(ny, nx) + start
        return [(center[0] + r * math.cos(a0 - sweep * k / cap), center[1] + r * math.sin(a0 - sweep * k / cap)) for k in range(1, cap)]

    end, start = bezier_point(curve, 1), bezier_point(curve, 0)
    nx, ny, r = normals[-1]
    end_cap = arc(end, nx, ny, r, 0, math.pi)                 # von links über vorn nach rechts
    nx, ny, r = normals[0]
    start_cap = arc(start, nx, ny, r, math.pi, math.pi)       # von rechts über hinten nach links
    return left + end_cap + right[::-1] + start_cap


# ------------------------------------------------------------------ Raster-Ausgabe


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


def draw_trail_and_cursor(img, spec):
    p0, p1, stops = spec["trail_gradient"]
    layer = linear(p0, p1, [(o, hexc(c, int(round(a * 255)))) for o, c, a in stops])
    img.alpha_composite(masked(layer, polygon(trail_outline(spec["trail"], spec["trail_width"]))))
    img.alpha_composite(solid(hexc("#3fe3c6"), polygon(spec["cursor"])))
    img.alpha_composite(solid(hexc(spec["ink"]), polyline(spec["cursor"], spec["cursor_stroke"], closed=True)))


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
    draw_trail_and_cursor(img, FULL)
    return img


def small():
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    bg = linear((0, 0), (0, 512), [(0, hexc("#2a3462")), (1, hexc("#151a31"))])
    img.alpha_composite(masked(bg, rrect(0, 0, 512, 512, 96)))
    draw_trail_and_cursor(img, SMALL)
    return img


def down(img, size):
    return img.resize((size, size), Image.LANCZOS)


# ------------------------------------------------------------------ SVG-Ausgabe


def fmt(v):
    return f"{v:.1f}".rstrip("0").rstrip(".")


def path(points):
    return "M" + " L".join(f"{fmt(x)} {fmt(y)}" for x, y in points) + " Z"


def svg_trail(spec):
    (x1, y1), (x2, y2), stops = spec["trail_gradient"]
    stop_tags = "\n".join(
        f'      <stop offset="{fmt(o)}" stop-color="{c}"' + ("" if a == 1 else f' stop-opacity="{fmt(a)}"') + "/>"
        for o, c, a in stops
    )
    gradient_tag = (
        f'    <linearGradient id="trail" gradientUnits="userSpaceOnUse" x1="{fmt(x1)}" y1="{fmt(y1)}" x2="{fmt(x2)}" y2="{fmt(y2)}">\n'
        f"{stop_tags}\n    </linearGradient>"
    )
    shapes = (
        f'  <path d="{path(trail_outline(spec["trail"], spec["trail_width"]))}" fill="url(#trail)"/>\n'
        f'  <path d="{path(spec["cursor"])}" fill="#3fe3c6" stroke="{spec["ink"]}" '
        f'stroke-width="{spec["cursor_stroke"]}" stroke-linejoin="round"/>'
    )
    return gradient_tag, shapes


def svg_full():
    gradient_tag, shapes = svg_trail(FULL)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <title>Qross</title>
  <!-- Erzeugt von crates/qross-gui/icons/render_logo.py -->
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#262f5a"/>
      <stop offset="1" stop-color="#12172a"/>
    </linearGradient>
{gradient_tag}
    <radialGradient id="glow" cx="340" cy="250" r="120" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#3fe3c6" stop-opacity=".35"/>
      <stop offset="1" stop-color="#3fe3c6" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect x="8" y="8" width="496" height="496" rx="108" fill="url(#bg)"/>
  <rect x="9.5" y="9.5" width="493" height="493" rx="106.5" fill="none" stroke="#5a66a0" stroke-opacity=".35" stroke-width="3"/>
  <g fill="none" stroke="#3a4470" stroke-width="12" stroke-linejoin="round">
    <rect x="62" y="176" width="172" height="120" rx="20"/>
    <rect x="278" y="176" width="172" height="120" rx="20"/>
  </g>
  <g fill="#3a4470">
    <rect x="128" y="306" width="40" height="12" rx="4"/>
    <rect x="344" y="306" width="40" height="12" rx="4"/>
  </g>
  <circle cx="340" cy="250" r="120" fill="url(#glow)"/>
{shapes}
</svg>
"""


def svg_small():
    gradient_tag, shapes = svg_trail(SMALL)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <title>Qross</title>
  <!-- Erzeugt von crates/qross-gui/icons/render_logo.py -->
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#2a3462"/>
      <stop offset="1" stop-color="#151a31"/>
    </linearGradient>
{gradient_tag}
  </defs>
  <rect x="0" y="0" width="512" height="512" rx="96" fill="url(#bg)"/>
{shapes}
</svg>
"""


def write(path_, text):
    with open(path_, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def main():
    out_icons, out_docs, out_ui = sys.argv[1:4]
    write(os.path.join(out_icons, "logo.svg"), svg_full())
    write(os.path.join(out_icons, "logo-small.svg"), svg_small())
    write(os.path.join(out_docs, "logo.svg"), svg_full())

    big, sm = full(), small()
    sizes = {n: down(sm, n) for n in (16, 20, 24, 32, 40, 48, 64)}
    sizes.update({n: down(big, n) for n in (128, 256, 512)})

    sizes[32].save(os.path.join(out_icons, "32x32.png"))      # auch Basis des Tray-Icons
    sizes[128].save(os.path.join(out_icons, "128x128.png"))
    sizes[512].save(os.path.join(out_icons, "icon.png"))
    sizes[256].save(os.path.join(out_docs, "logo.png"))
    sizes[128].save(os.path.join(out_ui, "icon.png"))         # Kopfzeile: volles Logo, scharf auch auf HiDPI
    ico = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    sizes[256].save(os.path.join(out_icons, "icon.ico"), sizes=[(n, n) for n in ico],
                    append_images=[sizes[n] for n in ico if n != 256])
    print("ok")


if __name__ == "__main__":
    main()
