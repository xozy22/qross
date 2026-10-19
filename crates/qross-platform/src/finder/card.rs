//! Bild der Drag-Vorschau: ein Kärtchen rechts unter dem Cursor mit Symbol, Name und Gesamtgröße
//! der gezogenen Dateien, in den Farben des Cursor-Finders. Passt es dort nicht auf den Monitor,
//! steht es links bzw. über dem Cursor. Die Spitze des Cursors deckt es nie ab: Beim Ablegen sucht
//! Qross den Ordner im Fenster unter ihr, und dort soll nicht das Kärtchen liegen.

use std::f32::consts::SQRT_2;
use std::time::Duration;

use qross_core::finder::DragIcon;
use qross_proto::Rect;

use super::draw::{center, coverage, plate, rounded_box, smoothstep, span, Area, Canvas, Display, Frame, Text, TEAL, WHITE};

/// Schriftgröße des Namens und der Größe darunter (logische Pixel)
pub const TITLE_SIZE: f32 = 14.0;
pub const DETAIL_SIZE: f32 = 12.0;
/// Längere Namen werden gekürzt
pub const MAX_TEXT_WIDTH: f32 = 280.0;

// Maße in logischen Pixeln (bei 100 % Skalierung)
const PAD: f32 = 8.0;
const PAD_RIGHT: f32 = 12.0;
/// Fläche des Symbols
const ICON_WIDTH: f32 = 24.0;
const ICON_HEIGHT: f32 = 30.0;
const ICON_GAP: f32 = 9.0;
const RADIUS: f32 = 7.0;
const PAGE_RADIUS: f32 = 2.0;
/// Abstand zur Spitze des Cursors: rechts unterhalb am Pfeil vorbei, sonst links bzw. darüber
const RIGHT: f32 = 16.0;
const BELOW: f32 = 20.0;
const LEFT: f32 = 8.0;
const ABOVE: f32 = 8.0;
/// Abstand zum Rand des Monitors
const MARGIN: f32 = 4.0;
/// Die Größe steht etwas blasser unter dem Namen
const DETAIL_ALPHA: f32 = 0.7;
/// Reiter des Ordners, dunkler als der Ordner selbst
const TEAL_DARK: [f32; 3] = [TEAL[0] * 0.7, TEAL[1] * 0.7, TEAL[2] * 0.7];
const FADE_IN: Duration = Duration::from_millis(80);

/// Fertig gezeichnetes Kärtchen; wo es steht, entscheidet jedes Bild neu ([`Card::frame`])
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Größe in Koordinaten des Systems
    pub width: u32,
    pub height: u32,
    /// Das Bild in Bildpunkten, BGRA mit vormultipliziertem Alpha
    pixel_width: u32,
    pixel_height: u32,
    pixels: Vec<u8>,
}

impl Card {
    /// Das Bild für den Cursor an dieser Stelle (siehe [`place`]), mit der Deckkraft `alpha`
    pub fn frame(&self, cursor: (i32, i32), display: &Display, alpha: f32) -> Frame {
        let (x, y) = place(cursor, (self.width, self.height), display);
        let pixels = if alpha >= 1.0 {
            self.pixels.clone()
        } else {
            let alpha = alpha.max(0.0);
            self.pixels.iter().map(|&v| (v as f32 * alpha + 0.5) as u8).collect()
        };
        Frame { rect: Rect::new(x, y, self.width, self.height), width: self.pixel_width, height: self.pixel_height, pixels }
    }
}

/// Deckkraft beim Einblenden, `t` nach dem Beginn
pub fn alpha_at(t: Duration) -> f32 {
    smoothstep(t.as_secs_f32() / FADE_IN.as_secs_f32())
}

/// Obere linke Ecke eines Kärtchens dieser Größe (in Koordinaten): rechts unter dem Cursor, wo es
/// dort nicht auf den Monitor passt links bzw. darüber. Waagerecht steht es immer neben der Spitze
/// des Cursors; senkrecht rückt es notfalls auf den Monitor.
pub fn place(cursor: (i32, i32), (width, height): (u32, u32), display: &Display) -> (i32, i32) {
    let px = |v: f32| (v * display.scale).round() as i32;
    let (area, (cx, cy)) = (display.area, cursor);
    let (width, height) = (width as i32, height as i32);
    let (left, top) = (area.x + px(MARGIN), area.y + px(MARGIN));
    let (right, bottom) = (area.right() + 1 - px(MARGIN), area.bottom() + 1 - px(MARGIN));
    let fits_right = cx + px(RIGHT) + width <= right;
    let fits_left = cx - px(LEFT) - width >= left;
    let x = if fits_right || !fits_left { cx + px(RIGHT) } else { cx - px(LEFT) - width };
    let y = if cy + px(BELOW) + height <= bottom { cy + px(BELOW) } else { (cy - px(ABOVE) - height).max(top) };
    (x, y)
}

/// Zeichnet das Kärtchen; `title` und `detail`: Name und Größe, gerastert zur Bildpunkt-Größe von
/// `display` (ohne Schrift bleibt es beim Symbol)
pub fn render(icon: DragIcon, title: Option<&Text>, detail: Option<&Text>, display: &Display) -> Card {
    let (scale, density) = (display.scale, display.density);
    let lines: Vec<&Text> = [title, detail].into_iter().flatten().collect();
    let text_width = lines.iter().map(|t| t.width as f32 / density).fold(0.0, f32::max);
    let text_height: f32 = lines.iter().map(|t| t.height as f32 / density).sum();
    let width = match text_width > 0.0 {
        true => (PAD + ICON_WIDTH + ICON_GAP + PAD_RIGHT) * scale + text_width,
        false => (2.0 * PAD + ICON_WIDTH) * scale,
    };
    // Ganze Koordinaten, damit das Bild beim Verschieben auf ganzen Bildpunkten bleibt
    let (width, height) = (width.ceil() as u32, (2.0 * PAD * scale + (ICON_HEIGHT * scale).max(text_height)).ceil() as u32);
    let to_px = |v: u32| ((v as f32 * density).round() as u32).max(1);
    let mut canvas = Canvas::new(to_px(width), to_px(height));
    let card = Area { x: 0.0, y: 0.0, width: width as f32, height: height as f32 };
    plate(&mut canvas, (0.0, 0.0), &card, RADIUS * scale, display);
    draw_icon(&mut canvas, icon, (PAD * scale, (height as f32 - ICON_HEIGHT * scale) / 2.0), display);
    // Name und Größe untereinander rechts neben dem Symbol, zusammen senkrecht mittig
    let left = ((PAD + ICON_WIDTH + ICON_GAP) * scale * density).round() as i64;
    let mut top = ((height as f32 - text_height) / 2.0 * density).round() as i64;
    for (text, alpha) in [(title, 1.0), (detail, DETAIL_ALPHA)] {
        if let Some(text) = text {
            canvas.text(text, left, top, WHITE, alpha);
            top += text.height as i64;
        }
    }
    let (pixel_width, pixel_height) = (canvas.width, canvas.height);
    Card { width, height, pixel_width, pixel_height, pixels: canvas.into_bgra(1.0) }
}

/// Teil eines Symbols, in logischen Pixeln ab der oberen linken Ecke des Symbols
enum Shape {
    /// Rechteck mit runden Ecken
    Rounded(Area, f32),
    /// Blatt, dessen Ecke oben rechts umgeknickt ist (Kantenlänge der Ecke)
    Page(Area, f32),
    /// Die umgeknickte Ecke selbst
    Fold(Area, f32),
}

impl Shape {
    /// Vorzeichenbehafteter Abstand (negativ: innen), in logischen Pixeln
    fn distance(&self, x: f32, y: f32) -> f32 {
        match self {
            Shape::Rounded(area, radius) => rounded_box(x, y, area, *radius),
            Shape::Page(area, fold) => {
                // Abgeschnitten jenseits der Diagonale von (rechts − fold, oben) nach (rechts, oben + fold)
                let cut = ((x - (area.right() - fold)) - (y - area.y)) / SQRT_2;
                rounded_box(x, y, area, PAGE_RADIUS).max(cut)
            }
            Shape::Fold(area, fold) => {
                // Dreieck unter dieser Diagonale, in der Ecke des Blatts
                let (u, v) = (x - (area.right() - fold), y - area.y);
                (-u).max(v - fold).max((u - v) / SQRT_2)
            }
        }
    }
}

/// Formen eines Symbols mit Farbe und Deckkraft, von hinten nach vorn
fn shapes(icon: DragIcon) -> Vec<(Shape, [f32; 3], f32)> {
    let area = |x, y, width, height| Area { x, y, width, height };
    match icon {
        DragIcon::File => {
            let page = area(2.0, 1.0, 20.0, 28.0);
            vec![(Shape::Page(page, 7.0), WHITE, 0.95), (Shape::Fold(page, 7.0), TEAL, 1.0)]
        }
        DragIcon::Folder => vec![
            (Shape::Rounded(area(0.5, 5.0, 10.0, 7.0), 1.5), TEAL_DARK, 1.0),
            (Shape::Rounded(area(0.5, 8.0, 23.0, 17.0), 2.5), TEAL, 1.0),
        ],
        // Zwei Blätter übereinander, das hintere blasser
        DragIcon::Several => {
            let front = area(1.0, 5.0, 17.0, 24.0);
            vec![
                (Shape::Page(area(6.0, 0.5, 17.0, 24.0), 6.0), WHITE, 0.45),
                (Shape::Page(front, 6.0), WHITE, 0.95),
                (Shape::Fold(front, 6.0), TEAL, 1.0),
            ]
        }
    }
}

/// Symbol mit der oberen linken Ecke bei `at` (Koordinaten im Kärtchen)
fn draw_icon(canvas: &mut Canvas, icon: DragIcon, at: (f32, f32), display: &Display) {
    let (scale, density) = (display.scale, display.density);
    let bounds = Area { x: at.0, y: at.1, width: ICON_WIDTH * scale, height: ICON_HEIGHT * scale };
    let (xs, ys) = span(canvas, &bounds, (0.0, 0.0), density);
    for (shape, color, alpha) in shapes(icon) {
        for y in ys.clone() {
            for x in xs.clone() {
                let (px, py) = center((0.0, 0.0), density, x, y);
                let d = shape.distance((px - at.0) / scale, (py - at.1) / scale) * scale * density;
                canvas.blend(x, y, color, alpha * coverage(-d));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_HD: Display = Display { area: Rect::new(0, 0, 1920, 1080), scale: 1.0, density: 1.0 };

    /// Gerasterter Text dieser Größe (in logischen Pixeln), voll deckend
    fn text(width: f32, height: f32, display: &Display) -> Text {
        let (width, height) = ((width * display.pixel_scale()) as u32, (height * display.pixel_scale()) as u32);
        Text { width, height, coverage: vec![255; (width * height) as usize] }
    }

    /// Kärtchen mit einem Namen von 120 × 19 und einer Größe von 50 × 16 logischen Pixeln
    fn sample(icon: DragIcon, display: &Display) -> Card {
        render(icon, Some(&text(120.0, 19.0, display)), Some(&text(50.0, 16.0, display)), display)
    }

    fn contains(rect: Rect, (x, y): (i32, i32)) -> bool {
        x >= rect.x && x <= rect.right() && y >= rect.y && y <= rect.bottom()
    }

    /// Bildpunkt des Bildes an der Stelle (x, y) des Systems: (B, G, R, A)
    fn pixel_at(frame: &Frame, display: &Display, x: f32, y: f32) -> [u8; 4] {
        let px = ((x - frame.rect.x as f32) * display.density) as u32;
        let py = ((y - frame.rect.y as f32) * display.density) as u32;
        frame.pixel(px, py)
    }

    #[test]
    fn the_card_hangs_below_right_of_the_cursor() {
        let card = sample(DragIcon::File, &FULL_HD);
        // Symbol, Abstand, der längere Text und Rand; so hoch wie Name und Größe zusammen
        assert_eq!((card.width, card.height), (8 + 24 + 9 + 120 + 12, 8 + 35 + 8));
        let frame = card.frame((500, 400), &FULL_HD, 1.0);
        assert_eq!(frame.rect, Rect::new(516, 420, card.width, card.height));
        assert_eq!((frame.width, frame.height), (card.width, card.height));
    }

    #[test]
    fn at_the_edges_it_moves_aside_but_never_covers_the_tip() {
        let card = sample(DragIcon::File, &FULL_HD);
        let at = |cursor| card.frame(cursor, &FULL_HD, 1.0).rect;
        let corner = at((1919, 1079));
        assert_eq!((corner.right() + 1, corner.bottom() + 1), (1919 - 8, 1079 - 8), "links über dem Cursor");
        assert_eq!(at((1919, 300)).y, 320, "rechts am Rand: links daneben, aber weiter darunter");
        assert_eq!(at((300, 1079)).x, 316, "unten am Rand: darüber, aber weiter rechts");
        // Rechter von zwei Monitoren mit 150 %: Maße und Abstände wachsen mit
        let right = Display { area: Rect::new(1920, 0, 2560, 1440), scale: 1.5, density: 1.0 };
        let large = sample(DragIcon::File, &right);
        let rect = large.frame((1920, 0), &right, 1.0).rect;
        assert_eq!((rect.x, rect.y), (1920 + 24, 30));
        assert!(rect.width > card.width * 14 / 10, "{rect:?}");

        for cursor in [(0, 0), (1919, 0), (0, 1079), (1919, 1079), (960, 540), (1800, 1060), (100, 1050)] {
            let rect = at(cursor);
            assert!(!contains(rect, cursor), "{cursor:?} unter {rect:?}");
            assert!(rect.x >= 0 && rect.right() < 1920 && rect.y >= 0 && rect.bottom() < 1080, "{cursor:?}: {rect:?}");
        }
        // Ist der Monitor zu schmal, bleibt es trotzdem neben der Spitze
        let tiny = Display { area: Rect::new(0, 0, 150, 100), scale: 1.0, density: 1.0 };
        assert!(!contains(card.frame((75, 50), &tiny, 1.0).rect, (75, 50)));
    }

    #[test]
    fn it_shows_the_icon_the_name_and_the_size() {
        let card = sample(DragIcon::File, &FULL_HD);
        let f = card.frame((0, 0), &FULL_HD, 1.0);
        let (x, y) = (f.rect.x as f32, f.rect.y as f32);
        let plate = pixel_at(&f, &FULL_HD, x + 4.0, y + 4.0);
        assert!(plate[3] > 200 && plate[2] < 60, "dunkles Kärtchen: {plate:?}");
        let edge = pixel_at(&f, &FULL_HD, x + 40.0, y + 0.5);
        assert!(edge[1] > 120 && edge[2] < 80, "türkiser Rand (BGRA): {edge:?}");
        // Das Blatt links: weiß, die Ecke oben rechts umgeknickt (türkis), dahinter abgeschnitten
        let top = y + (card.height as f32 - ICON_HEIGHT) / 2.0;
        let page = pixel_at(&f, &FULL_HD, x + PAD + 8.0, top + 20.0);
        assert!(page.iter().all(|&c| c > 200), "weißes Blatt: {page:?}");
        let fold = pixel_at(&f, &FULL_HD, x + PAD + 17.5, top + 6.0);
        assert!(fold[1] > 180 && fold[2] < 120, "türkise Ecke (BGRA): {fold:?}");
        let cut = pixel_at(&f, &FULL_HD, x + PAD + 21.5, top + 1.5);
        assert!(cut[2] < 60, "abgeknickt, dort das dunkle Kärtchen: {cut:?}");
        // Name weiß, Größe darunter blasser
        let text_x = x + PAD + ICON_WIDTH + ICON_GAP + 10.0;
        let name = pixel_at(&f, &FULL_HD, text_x, y + 8.0 + 5.0);
        assert_eq!(name, [255, 255, 255, 255]);
        let size = pixel_at(&f, &FULL_HD, text_x, y + 8.0 + 19.0 + 5.0);
        assert!(size[3] > 240 && size[2] < 230 && size[2] > 150, "blasser: {size:?}");
        assert!(pixel_at(&f, &FULL_HD, text_x + 80.0, y + 8.0 + 19.0 + 5.0)[2] < 60, "kürzer als der Name");
    }

    #[test]
    fn folders_and_several_files_have_their_own_icons() {
        let icon_pixel = |icon, dx: f32, dy: f32| {
            let card = sample(icon, &FULL_HD);
            let f = card.frame((0, 0), &FULL_HD, 1.0);
            let top = f.rect.y as f32 + (card.height as f32 - ICON_HEIGHT) / 2.0;
            pixel_at(&f, &FULL_HD, f.rect.x as f32 + PAD + dx, top + dy)
        };
        let folder = icon_pixel(DragIcon::Folder, 12.0, 16.0);
        assert!(folder[1] > 180 && folder[2] < 120, "türkiser Ordner: {folder:?}");
        let tab = icon_pixel(DragIcon::Folder, 4.0, 6.5);
        assert!(tab[1] > 100 && tab[1] < folder[1], "dunklerer Reiter: {tab:?}");
        let front = icon_pixel(DragIcon::Several, 6.0, 20.0);
        assert!(front.iter().all(|&c| c > 200), "vorderes Blatt: {front:?}");
        let back = icon_pixel(DragIcon::Several, 21.0, 10.0);
        assert!(back[2] > 80 && back[2] < 200, "hinteres Blatt blasser: {back:?}");
    }

    #[test]
    fn retina_draws_twice_the_pixels_at_the_same_place() {
        let retina = Display { area: Rect::new(0, 0, 1512, 982), scale: 1.0, density: 2.0 };
        let card = sample(DragIcon::Several, &retina);
        assert_eq!((card.width, card.height), (173, 51), "in Punkten wie ohne Retina");
        let f = card.frame((700, 400), &retina, 1.0);
        assert_eq!((f.width, f.height), (f.rect.width * 2, f.rect.height * 2));
        assert_eq!(pixel_at(&f, &retina, f.rect.x as f32 + 60.0, f.rect.y as f32 + 13.0), [255, 255, 255, 255], "Name");
    }

    #[test]
    fn it_fades_in_and_works_without_a_font() {
        assert_eq!(alpha_at(Duration::ZERO), 0.0);
        assert!(alpha_at(Duration::from_millis(40)) > 0.2 && alpha_at(Duration::from_millis(40)) < 0.8);
        assert_eq!(alpha_at(FADE_IN), 1.0);
        let card = sample(DragIcon::File, &FULL_HD);
        let half = card.frame((0, 0), &FULL_HD, 0.5);
        let full = card.frame((0, 0), &FULL_HD, 1.0);
        assert_eq!(half.pixel(4, 4)[3], ((full.pixel(4, 4)[3] as f32) * 0.5 + 0.5) as u8);

        // Ohne Schrift: nur das Symbol
        let bare = render(DragIcon::Folder, None, None, &FULL_HD);
        assert_eq!((bare.width, bare.height), (8 + 24 + 8, 8 + 30 + 8));
    }
}
