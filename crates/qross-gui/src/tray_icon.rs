//! Tray-Symbol zeichnen: Statuspunkt unten links, bei laufenden Übertragungen stattdessen ein
//! Fortschrittsbalken über die ganze Breite. Gezeichnet wird in RGBA-Pixel des 32 × 32 großen
//! Basis-Symbols.

/// Spalten im Inneren des Fortschrittsbalkens (Symbol 32 × 32)
pub const BAR_COLUMNS: u32 = 28;

/// Gefüllte Spalten des Balkens; das Symbol ändert sich so nur, wenn man es sieht
pub fn bar_fill(percent: u8) -> u8 {
    ((u32::from(percent.min(100)) * BAR_COLUMNS + 50) / 100) as u8
}

/// Unten links, damit der Mauszeiger im Logo frei bleibt
pub fn draw_dot(rgba: &mut [u8], w: u32, h: u32, [r, g, b]: [u8; 3]) {
    let (cx, cy, radius) = (8.0, h as f32 - 8.0, 7.5f32);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let i = ((y * w + x) * 4) as usize;
            if d <= radius - 1.5 {
                rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            } else if d <= radius {
                // weißer Rand, damit der Punkt auf jedem Hintergrund sichtbar ist
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
}

/// Balken über die ganze Breite am unteren Rand: weißer Rahmen wie beim Punkt, dunkle Spur,
/// gefüllt in Türkis wie die Fortschrittsanzeige im Fenster. Die Ecken bleiben frei (abgerundet).
pub fn draw_bar(rgba: &mut [u8], w: u32, h: u32, filled: u8) {
    const FRAME: [u8; 4] = [255, 255, 255, 255];
    const TRACK: [u8; 4] = [30, 34, 56, 255];
    const FILL: [u8; 4] = [63, 227, 198, 255];
    let (x0, x1, y0, y1) = (1, w - 1, h - 9, h - 1);
    for y in y0..y1 {
        for x in x0..x1 {
            let (edge_x, edge_y) = (x == x0 || x == x1 - 1, y == y0 || y == y1 - 1);
            if edge_x && edge_y {
                continue;
            }
            let color = if edge_x || edge_y {
                FRAME
            } else if x - (x0 + 1) < u32::from(filled) {
                FILL
            } else {
                TRACK
            };
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_changes_only_per_visible_column() {
        assert_eq!(bar_fill(0), 0);
        assert_eq!(bar_fill(1), 0);
        assert_eq!(bar_fill(2), 1);
        assert_eq!(bar_fill(50), 14);
        assert_eq!(bar_fill(100), 28);
        assert_eq!(bar_fill(250), 28);
    }

    #[test]
    fn the_bar_fills_from_the_left_inside_a_white_frame() {
        let (w, h) = (32, 32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        draw_bar(&mut rgba, w, h, 14);
        let at = |x: u32, y: u32| rgba[((y * w + x) * 4) as usize..][..4].to_vec();
        let middle = h - 5;
        assert_eq!(at(1, middle), [255, 255, 255, 255], "Rahmen links");
        assert_eq!(at(2, middle), [63, 227, 198, 255], "erste Spalte gefüllt");
        assert_eq!(at(15, middle), [63, 227, 198, 255], "14. Spalte gefüllt");
        assert_eq!(at(16, middle), [30, 34, 56, 255], "danach die Spur");
        assert_eq!(at(29, middle), [30, 34, 56, 255], "letzte Spalte leer");
        assert_eq!(at(30, middle), [255, 255, 255, 255], "Rahmen rechts");
        assert_eq!(at(1, h - 9), [0, 0, 0, 0], "Ecke bleibt frei");
        assert_eq!(at(16, h - 10), [0, 0, 0, 0], "darüber bleibt das Logo");
        assert_eq!(at(16, h - 1), [0, 0, 0, 0], "unterste Zeile bleibt frei");
    }

    #[test]
    fn a_full_bar_reaches_the_right_frame() {
        let (w, h) = (32, 32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        draw_bar(&mut rgba, w, h, bar_fill(100));
        let at = |x: u32, y: u32| rgba[((y * w + x) * 4) as usize..][..4].to_vec();
        assert_eq!(at(29, h - 5), [63, 227, 198, 255]);
        assert_eq!(at(30, h - 5), [255, 255, 255, 255]);
    }
}
