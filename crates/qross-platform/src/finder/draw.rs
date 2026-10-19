//! Bild des Cursor-Finders: Ring und Namensschild mit Alphakanal, auf allen Systemen gleich
//! gezeichnet. Die Systeme legen das Bild nur noch in ein durchsichtiges Fenster.
//!
//! Der Ring zieht sich in gut einer Drittelsekunde um den Cursor zusammen und lenkt so den Blick
//! dorthin; das Schild mit dem Namen steht darunter (am unteren Rand darüber). Nach einer halben
//! Sekunde blendet alles sanft aus.

use std::ops::Range;
use std::time::Duration;

use qross_proto::{CursorFinder, Rect};

/// So lange ist der Cursor-Finder zu sehen
pub const DURATION: Duration = Duration::from_millis(1000);
/// Einblenden, Zusammenziehen des Rings, Beginn des Ausblendens (Sekunden)
const FADE_IN: f32 = 0.08;
const SHRINK: f32 = 0.35;
const FADE_OUT_FROM: f32 = 0.55;

// Maße in logischen Pixeln (bei 100 % Skalierung)
const RING_START: f32 = 72.0;
const RING_END: f32 = 30.0;
const RING_WIDTH: f32 = 4.0;
/// Dunkler Saum beiderseits des Rings, damit er auch auf hellem Grund deutlich bleibt
const HALO: f32 = 2.0;
/// Schriftgröße des Namens
pub const FONT_SIZE: f32 = 15.0;
const PAD_X: f32 = 11.0;
const PAD_Y: f32 = 5.0;
const BORDER: f32 = 1.0;
/// Abstand zwischen Ring und Schild
const GAP: f32 = 12.0;
/// Ohne Ring: so weit unter (bzw. über) der Spitze des Cursors steht das Schild
const CURSOR_CLEARANCE: f32 = 26.0;
/// Abstand des Schilds zum Rand des Monitors
const MARGIN: f32 = 8.0;
/// Längere Namen werden gekürzt
pub const MAX_LABEL_WIDTH: f32 = 360.0;

/// Farben der Oberfläche: Türkis für den Cursor, Mitternachtsblau für Saum und Schild
pub(super) const TEAL: [f32; 3] = [0x3f as f32 / 255.0, 0xe3 as f32 / 255.0, 0xc6 as f32 / 255.0];
const INK: [f32; 3] = [0x12 as f32 / 255.0, 0x17 as f32 / 255.0, 0x2a as f32 / 255.0];
pub(super) const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const HALO_ALPHA: f32 = 0.55;
const PLATE_ALPHA: f32 = 0.9;
const BORDER_ALPHA: f32 = 0.6;

/// Monitor unter dem Cursor
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    /// Fläche in den Koordinaten des Systems (Windows, X11: Pixel; macOS: Punkte)
    pub area: Rect,
    /// Koordinaten je logischem Pixel (Windows: DPI / 96; macOS: 1, denn Punkte sind schon logisch)
    pub scale: f32,
    /// Bildpunkte je Koordinate (macOS mit Retina: 2; sonst 1)
    pub density: f32,
}

impl Display {
    /// Bildpunkte je logischem Pixel
    pub fn pixel_scale(&self) -> f32 {
        self.scale * self.density
    }
}

/// Gerasterter Name: Deckung je Bildpunkt (0 bis 255), Zeile für Zeile
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

/// Ein fertiges Bild für das Fenster
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Lage und Größe des Fensters in den Koordinaten des Systems
    pub rect: Rect,
    /// Größe des Bildes in Bildpunkten
    pub width: u32,
    pub height: u32,
    /// BGRA mit vormultipliziertem Alpha, Zeile für Zeile von oben
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Farbe und Deckung an (x, y) in Bildpunkten: (B, G, R, A)
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }
}

/// Rechteck mit Bruchteilen, in Koordinaten des Systems
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Area {
    pub(super) fn right(&self) -> f32 {
        self.x + self.width
    }

    pub(super) fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

/// Ring um den Cursor: Mitte, Radius (Mitte der Linie) und Linienstärke, in Koordinaten
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub width: f32,
    pub halo: f32,
}

/// Was zum Zeitpunkt `t` wo zu sehen ist
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    pub ring: Option<Ring>,
    /// Schild mit dem Namen
    pub label: Option<Area>,
    /// Deckkraft von allem (Ein- und Ausblenden)
    pub alpha: f32,
    /// Fenster: alles zusammen, auf den Monitor beschnitten
    pub bounds: Rect,
}

pub(super) fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn ease_out(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

/// Deckkraft zum Zeitpunkt `t` (Sekunden)
fn alpha_at(t: f32) -> f32 {
    let fade_out = (t - FADE_OUT_FROM) / (DURATION.as_secs_f32() - FADE_OUT_FROM);
    smoothstep(t / FADE_IN) * (1.0 - smoothstep(fade_out))
}

/// Größe des Schilds in Koordinaten für einen Namen dieser Größe (Bildpunkte)
pub fn label_size(text: &Text, display: &Display) -> (f32, f32) {
    let scale = display.scale;
    (text.width as f32 / display.density + 2.0 * PAD_X * scale, text.height as f32 / display.density + 2.0 * PAD_Y * scale)
}

/// Was zu sehen ist: `cursor` und `display` in Koordinaten des Systems, `label`: Größe des Schilds
/// (siehe [`label_size`]). `None`, wenn die Zeit um ist oder nichts zu zeigen ist.
pub fn scene(style: CursorFinder, cursor: (i32, i32), display: &Display, t: Duration, label: Option<(f32, f32)>) -> Option<Scene> {
    if t >= DURATION {
        return None;
    }
    let (secs, scale) = (t.as_secs_f32(), display.scale);
    let (cx, cy) = (cursor.0 as f32, cursor.1 as f32);
    let ring = style.ring.then(|| Ring {
        x: cx,
        y: cy,
        radius: (RING_END + (RING_START - RING_END) * (1.0 - ease_out(secs / SHRINK))) * scale,
        width: RING_WIDTH * scale,
        halo: HALO * scale,
    });
    let label = match (style.name, label) {
        (true, Some((width, height))) => Some(place_label(cx, cy, width, height, style.ring, display)),
        _ => None,
    };
    let area = display.area;
    let mut parts = Vec::new();
    if let Some(r) = ring {
        let reach = r.radius + r.width / 2.0 + r.halo + 1.0;
        parts.push(Area { x: cx - reach, y: cy - reach, width: 2.0 * reach, height: 2.0 * reach });
    }
    parts.extend(label);
    let left = parts.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor().max(area.x as f32);
    let top = parts.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor().max(area.y as f32);
    let right = parts.iter().map(Area::right).fold(f32::MIN, f32::max).ceil().min((area.right() + 1) as f32);
    let bottom = parts.iter().map(Area::bottom).fold(f32::MIN, f32::max).ceil().min((area.bottom() + 1) as f32);
    if parts.is_empty() || right <= left || bottom <= top {
        return None;
    }
    let bounds = Rect::new(left as i32, top as i32, (right - left) as u32, (bottom - top) as u32);
    Some(Scene { ring, label, alpha: alpha_at(secs), bounds })
}

/// Schild unter dem Cursor (unter dem Ring); passt es dort nicht auf den Monitor, darüber.
/// Seitlich bleibt es auf dem Monitor, auch wenn der Cursor am Rand steht.
fn place_label(cx: f32, cy: f32, width: f32, height: f32, ring: bool, display: &Display) -> Area {
    let scale = display.scale;
    let area = display.area;
    let (left, top) = (area.x as f32, area.y as f32);
    let (right, bottom) = ((area.right() + 1) as f32, (area.bottom() + 1) as f32);
    let margin = MARGIN * scale;
    let gap = if ring { (RING_END + RING_WIDTH / 2.0 + HALO + GAP) * scale } else { CURSOR_CLEARANCE * scale };
    let below = cy + gap;
    let y = if below + height <= bottom - margin { below } else { (cy - gap - height).max(top + margin) };
    let x = (cx - width / 2.0).min(right - margin - width).max(left + margin);
    Area { x, y, width, height }
}

/// Premultipliziertes RGBA in Gleitkomma, zum Übereinanderlegen
pub(super) struct Canvas {
    pub(super) width: u32,
    pub(super) height: u32,
    rgba: Vec<[f32; 4]>,
}

impl Canvas {
    pub(super) fn new(width: u32, height: u32) -> Self {
        Self { width, height, rgba: vec![[0.0; 4]; (width * height) as usize] }
    }

    /// `color` mit Deckung `alpha` über den Bildpunkt legen
    pub(super) fn blend(&mut self, x: u32, y: u32, color: [f32; 3], alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let px = &mut self.rgba[(y * self.width + x) as usize];
        let keep = 1.0 - alpha;
        for (value, c) in px.iter_mut().zip(color) {
            *value = c * alpha + *value * keep;
        }
        px[3] = alpha + px[3] * keep;
    }

    /// Gerasterten Text in `color` mit der Deckkraft `alpha` auftragen, obere linke Ecke bei
    /// (`left`, `top`) in Bildpunkten; was über den Rand ragt, fällt weg
    pub(super) fn text(&mut self, text: &Text, left: i64, top: i64, color: [f32; 3], alpha: f32) {
        for ty in 0..text.height {
            for tx in 0..text.width {
                let (x, y) = (left + tx as i64, top + ty as i64);
                if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
                    continue;
                }
                let c = text.coverage[(ty * text.width + tx) as usize] as f32 / 255.0;
                self.blend(x as u32, y as u32, color, alpha * c);
            }
        }
    }

    /// Als BGRA (vormultipliziert) mit der Deckkraft `alpha` für alles
    pub(super) fn into_bgra(self, alpha: f32) -> Vec<u8> {
        let to_byte = |v: f32| (v * alpha * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        self.rgba.iter().flat_map(|p| [to_byte(p[2]), to_byte(p[1]), to_byte(p[0]), to_byte(p[3])]).collect()
    }
}

/// Deckung eines Bildpunkts, dessen Mitte `distance` (in Bildpunkten) vor einer Kante liegt
pub(super) fn coverage(distance: f32) -> f32 {
    (distance + 0.5).clamp(0.0, 1.0)
}

/// Vorzeichenbehafteter Abstand zu einem Rechteck mit runden Ecken (negativ: innen)
pub(super) fn rounded_box(px: f32, py: f32, b: &Area, radius: f32) -> f32 {
    let (hx, hy) = (b.width / 2.0, b.height / 2.0);
    let (qx, qy) = ((px - b.x - hx).abs() - (hx - radius), (py - b.y - hy).abs() - (hy - radius));
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - radius
}

/// Mitte des Bildpunkts (x, y) in Koordinaten des Systems; `origin`: Koordinaten der oberen linken
/// Ecke des Bildes
pub(super) fn center(origin: (f32, f32), density: f32, x: u32, y: u32) -> (f32, f32) {
    (origin.0 + (x as f32 + 0.5) / density, origin.1 + (y as f32 + 0.5) / density)
}

/// Nur die Bildpunkte, die `area` (in Koordinaten) berühren
pub(super) fn span(canvas: &Canvas, area: &Area, origin: (f32, f32), density: f32) -> (Range<u32>, Range<u32>) {
    let to_px = |v: f32, origin: f32, limit: u32| (((v - origin) * density).floor().max(0.0) as u32).min(limit);
    let xs = to_px(area.x, origin.0, canvas.width)..to_px(area.right() + 1.0, origin.0, canvas.width);
    let ys = to_px(area.y, origin.1, canvas.height)..to_px(area.bottom() + 1.0, origin.1, canvas.height);
    (xs, ys)
}

/// Dunkles Schild mit feinem türkisem Rand, `plate` und `radius` in Koordinaten
pub(super) fn plate(canvas: &mut Canvas, origin: (f32, f32), plate: &Area, radius: f32, display: &Display) {
    let density = display.density;
    let border = BORDER * display.scale;
    let (xs, ys) = span(canvas, plate, origin, density);
    for y in ys {
        for x in xs.clone() {
            let (px, py) = center(origin, density, x, y);
            let d = rounded_box(px, py, plate, radius) * density;
            if d > 1.0 {
                continue;
            }
            canvas.blend(x, y, INK, PLATE_ALPHA * coverage(-d));
            // Feiner Rand: Abstand zur Mitte eines Streifens innen entlang der Kante
            let edge = (d + border * density / 2.0).abs() - border * density / 2.0;
            canvas.blend(x, y, TEAL, BORDER_ALPHA * coverage(-edge));
        }
    }
}

/// Zeichnet die Szene; `text`: der gerasterte Name (zur Dichte von `display`)
pub fn render(scene: &Scene, display: &Display, text: Option<&Text>) -> Frame {
    let density = display.density;
    let rect = scene.bounds;
    let (width, height) = (((rect.width as f32) * density).round() as u32, ((rect.height as f32) * density).round() as u32);
    let mut canvas = Canvas::new(width.max(1), height.max(1));
    let origin = (rect.x as f32, rect.y as f32);

    if let Some(ring) = scene.ring {
        let half = ring.width / 2.0 * density;
        let outer = half + ring.halo * density;
        let reach = ring.radius + ring.width / 2.0 + ring.halo + 1.0;
        let area = Area { x: ring.x - reach, y: ring.y - reach, width: 2.0 * reach, height: 2.0 * reach };
        let (xs, ys) = span(&canvas, &area, origin, density);
        for y in ys {
            for x in xs.clone() {
                let (px, py) = center(origin, density, x, y);
                let off = ((px - ring.x).hypot(py - ring.y) - ring.radius).abs() * density;
                if off > outer + 1.0 {
                    continue;
                }
                canvas.blend(x, y, INK, HALO_ALPHA * coverage(outer - off));
                canvas.blend(x, y, TEAL, coverage(half - off));
            }
        }
    }

    if let Some(label) = scene.label {
        plate(&mut canvas, origin, &label, label.height / 2.0, display);
        if let Some(text) = text {
            // Text mittig im Schild, auf ganze Bildpunkte gelegt
            let left = ((label.x - origin.0) * density + (label.width * density - text.width as f32) / 2.0).round() as i64;
            let top = ((label.y - origin.1) * density + (label.height * density - text.height as f32) / 2.0).round() as i64;
            canvas.text(text, left, top, WHITE, 1.0);
        }
    }

    Frame { rect, width: canvas.width, height: canvas.height, pixels: canvas.into_bgra(scene.alpha) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: CursorFinder = CursorFinder { ring: true, name: true };
    const RING: CursorFinder = CursorFinder { ring: true, name: false };
    const NAME: CursorFinder = CursorFinder { ring: false, name: true };
    const FULL_HD: Display = Display { area: Rect::new(0, 0, 1920, 1080), scale: 1.0, density: 1.0 };

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// Ein Name von 80 × 18 Bildpunkten, voll deckend
    fn text(display: &Display) -> Text {
        let (width, height) = ((80.0 * display.pixel_scale()) as u32, (18.0 * display.pixel_scale()) as u32);
        Text { width, height, coverage: vec![255; (width * height) as usize] }
    }

    fn frame(style: CursorFinder, cursor: (i32, i32), display: &Display, ms: u64) -> (Scene, Frame) {
        let text = text(display);
        let scene = scene(style, cursor, display, at(ms), Some(label_size(&text, display))).expect("sichtbar");
        let frame = render(&scene, display, Some(&text));
        (scene, frame)
    }

    /// Bildpunkt des Bildes, der an der Stelle (x, y) des Systems liegt
    fn pixel_at(frame: &Frame, display: &Display, x: i32, y: i32) -> [u8; 4] {
        let px = ((x - frame.rect.x) as f32 * display.density) as u32;
        let py = ((y - frame.rect.y) as f32 * display.density) as u32;
        frame.pixel(px, py)
    }

    #[test]
    fn the_ring_closes_in_on_the_cursor_and_fades_out() {
        let radius = |ms| scene(RING, (500, 500), &FULL_HD, at(ms), None).unwrap().ring.unwrap().radius;
        assert_eq!(radius(0), RING_START);
        assert!(radius(150) < RING_START && radius(150) > RING_END);
        assert!((radius(400) - RING_END).abs() < 0.01, "danach bleibt er klein");

        let alpha = |ms| scene(RING, (500, 500), &FULL_HD, at(ms), None).unwrap().alpha;
        assert!(alpha(0) < 0.01, "blendet ein");
        assert_eq!(alpha(300), 1.0);
        assert!(alpha(800) < 1.0 && alpha(800) > 0.0, "blendet sanft aus");
        assert!(alpha(995) < 0.01);
        assert!(scene(RING, (500, 500), &FULL_HD, DURATION, None).is_none(), "nach einer Sekunde ist er weg");
    }

    #[test]
    fn the_ring_surrounds_the_cursor_with_a_clear_middle() {
        let (scene, f) = frame(RING, (500, 400), &FULL_HD, 450);
        assert!(scene.label.is_none());
        let on_ring = pixel_at(&f, &FULL_HD, 500 + RING_END as i32, 400);
        assert_eq!(on_ring[3], 255, "deckend auf der Linie: {on_ring:?}");
        assert!(on_ring[1] > 200 && on_ring[0] > 150 && on_ring[2] < 100, "türkis (BGRA): {on_ring:?}");
        let halo = pixel_at(&f, &FULL_HD, 500 + RING_END as i32 + 3, 400);
        assert!(halo[3] > 0 && halo[1] < 100, "dunkler Saum außen: {halo:?}");
        assert_eq!(pixel_at(&f, &FULL_HD, 500, 400)[3], 0, "in der Mitte bleibt der Cursor frei");
        assert_eq!(pixel_at(&f, &FULL_HD, 500 + 15, 400 + 3)[3], 0);
        // Das Fenster umschließt nur den Ring
        let reach = (RING_END + RING_WIDTH / 2.0 + HALO + 1.0) as u32;
        assert!(scene.bounds.width >= 2 * reach && scene.bounds.width <= 2 * reach + 2, "{:?}", scene.bounds);
    }

    #[test]
    fn the_name_stands_below_the_ring_or_above_it_at_the_bottom() {
        let (scene, f) = frame(BOTH, (960, 500), &FULL_HD, 450);
        let label = scene.label.unwrap();
        assert!(label.y > 500.0 + RING_END + RING_WIDTH, "unter dem Ring: {label:?}");
        assert!((label.x + label.width / 2.0 - 960.0).abs() < 1.0, "mittig unter dem Cursor");
        let middle = pixel_at(&f, &FULL_HD, (label.x + label.width / 2.0) as i32, (label.y + label.height / 2.0) as i32);
        assert_eq!(middle, [255, 255, 255, 255], "weißer Text");
        let plate = pixel_at(&f, &FULL_HD, (label.x + 4.0) as i32, (label.y + label.height / 2.0) as i32);
        assert!(plate[3] > 200 && plate[2] < 60, "dunkles Schild: {plate:?}");

        // Am unteren Rand steht es über dem Cursor, ganz auf dem Monitor
        let (scene, _) = frame(BOTH, (960, 1079), &FULL_HD, 450);
        let label = scene.label.unwrap();
        assert!(label.bottom() < 1079.0 - RING_END, "{label:?}");
    }

    #[test]
    fn everything_stays_on_the_monitor_of_the_cursor() {
        // Rechter von zwei Monitoren, Cursor kommt links unten an
        let right = Display { area: Rect::new(1920, 0, 2560, 1440), scale: 1.0, density: 1.0 };
        let (scene, f) = frame(BOTH, (1920, 1439), &right, 200);
        assert!(scene.bounds.x >= 1920 && scene.bounds.bottom() <= 1439, "{:?}", scene.bounds);
        let label = scene.label.unwrap();
        assert!(label.x >= 1920.0 + MARGIN && label.bottom() <= 1440.0, "{label:?}");
        assert_eq!((f.width, f.height), (scene.bounds.width, scene.bounds.height));
        // Am rechten Rand mit langem Namen: bündig am Rand statt abgeschnitten
        let (scene, _) = frame(NAME, (4479, 300), &right, 200);
        let label = scene.label.unwrap();
        assert!((label.right() - (4480.0 - MARGIN)).abs() < 0.01, "{label:?}");
        assert!(scene.ring.is_none());
        assert!(label.y >= 300.0 + CURSOR_CLEARANCE - 0.01, "ohne Ring dicht unter dem Cursor: {label:?}");
    }

    #[test]
    fn higher_scaling_draws_everything_larger() {
        let hidpi = Display { area: Rect::new(0, 0, 3840, 2160), scale: 1.5, density: 1.0 };
        let (scene, _) = frame(BOTH, (1000, 1000), &hidpi, 450);
        assert!((scene.ring.unwrap().radius - RING_END * 1.5).abs() < 0.01);
        assert!((scene.label.unwrap().height - (18.0 * 1.5 + 2.0 * PAD_Y * 1.5)).abs() < 1.0);

        // Retina (macOS): Koordinaten in Punkten, das Bild hat doppelt so viele Bildpunkte
        let retina = Display { area: Rect::new(0, 0, 1512, 982), scale: 1.0, density: 2.0 };
        let (scene, f) = frame(BOTH, (700, 400), &retina, 450);
        assert_eq!((f.width, f.height), (scene.bounds.width * 2, scene.bounds.height * 2));
        assert_eq!(pixel_at(&f, &retina, 700 + RING_END as i32, 400)[3], 255);
    }

    #[test]
    fn nothing_without_ring_or_name() {
        assert!(scene(CursorFinder::OFF, (5, 5), &FULL_HD, at(100), Some((50.0, 20.0))).is_none());
        // Name gewünscht, aber keine Schrift gefunden: dann eben nichts
        assert!(scene(NAME, (5, 5), &FULL_HD, at(100), None).is_none());
        assert!(scene(BOTH, (5, 5), &FULL_HD, at(100), None).unwrap().label.is_none());
    }
}
