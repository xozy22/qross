//! Schrift für das Namensschild: die Systemschrift (Windows: Segoe UI, macOS: SF Pro, Linux: was
//! fontconfig für „sans-serif“ nennt), gerastert mit `ab_glyph`. Fehlt sie, bleibt es beim Ring.

use std::path::PathBuf;

use ab_glyph::{point, Font as _, FontVec, GlyphId, PxScale, ScaleFont, VariableFont as _};

use super::draw::Text;

/// Halbfett, wo die Schrift Gewichte kennt (variable Schriften wie SF Pro)
const WEIGHT: f32 = 600.0;

pub struct Font(FontVec);

impl Font {
    /// Die erste brauchbare Systemschrift
    pub fn system() -> Option<Self> {
        candidates().into_iter().find_map(|(path, index)| {
            let bytes = std::fs::read(&path).ok()?;
            let mut font = FontVec::try_from_vec_and_index(bytes, index).ok()?;
            font.set_variation(b"wght", WEIGHT);
            tracing::debug!("Cursor-Finder: Schrift {}", path.display());
            Some(Self(font))
        })
    }

    /// Rastert `text` in `px` Bildpunkten Schriftgröße, höchstens `max_width` breit (sonst mit „…“
    /// gekürzt). `None` bei leerem Text.
    pub fn render(&self, text: &str, px: f32, max_width: f32) -> Option<Text> {
        let scale = PxScale::from(px);
        let font = self.0.as_scaled(scale);
        let mut chars: Vec<char> = text.trim().chars().filter(|c| !c.is_control()).collect();
        if chars.is_empty() {
            return None;
        }
        let ellipsis = if font.glyph_id('…') == GlyphId(0) { "..." } else { "…" };
        let mut glyphs = layout(&font, chars.iter().copied());
        while glyphs.1 > max_width && chars.len() > 1 {
            chars.pop();
            glyphs = layout(&font, chars.iter().copied().chain(ellipsis.chars()));
        }
        let (positions, advance) = glyphs;
        let (ascent, descent) = (font.ascent(), font.descent());
        let width = advance.ceil().max(1.0) as u32;
        let height = (ascent - descent).ceil().max(1.0) as u32;
        let mut coverage = vec![0u8; (width * height) as usize];
        for (id, x) in positions {
            let Some(outline) = self.0.outline_glyph(id.with_scale_and_position(scale, point(x, ascent))) else { continue };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, c| {
                let (x, y) = (bounds.min.x as i64 + gx as i64, bounds.min.y as i64 + gy as i64);
                if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
                    return;
                }
                let cell = &mut coverage[(y as u32 * width + x as u32) as usize];
                *cell = cell.saturating_add((c.clamp(0.0, 1.0) * 255.0).round() as u8);
            });
        }
        Some(Text { width, height, coverage })
    }
}

/// Glyphen mit ihrer Position (Vorschub samt Unterschneidung) und die Gesamtbreite
fn layout<F: ab_glyph::Font>(font: &ab_glyph::PxScaleFont<&F>, chars: impl Iterator<Item = char>) -> (Vec<(GlyphId, f32)>, f32) {
    let mut x = 0.0;
    let mut previous = None;
    let mut glyphs = Vec::new();
    for c in chars {
        let id = font.glyph_id(c);
        if let Some(previous) = previous {
            x += font.kern(previous, id);
        }
        glyphs.push((id, x));
        x += font.h_advance(id);
        previous = Some(id);
    }
    (glyphs, x)
}

/// Schriftdateien in der Reihenfolge, in der sie versucht werden (Pfad, Index in einer Sammlung)
fn candidates() -> Vec<(PathBuf, u32)> {
    let mut out: Vec<(PathBuf, u32)> = Vec::new();
    #[cfg(windows)]
    {
        let fonts = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
        for name in ["seguisb.ttf", "segoeuib.ttf", "segoeui.ttf", "arialbd.ttf", "arial.ttf"] {
            out.push((fonts.join(name), 0));
        }
    }
    #[cfg(target_os = "macos")]
    for path in [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSText.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/Library/Fonts/Arial.ttf",
    ] {
        out.push((path.into(), 0));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(found) = fontconfig("sans-serif:semibold") {
            out.push(found);
        }
        for path in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/google-noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Bold.ttf",
        ] {
            out.push((path.into(), 0));
        }
    }
    out
}

/// Was fontconfig für `pattern` nimmt (Datei und Index), z. B. Cantarell unter GNOME
#[cfg(target_os = "linux")]
fn fontconfig(pattern: &str) -> Option<(PathBuf, u32)> {
    let output = std::process::Command::new("fc-match").args(["-f", "%{file}\n%{index}", pattern]).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines();
    let file = lines.next().filter(|f| !f.is_empty())?;
    let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
    Some((file.into(), index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_font_renders_names_and_shortens_long_ones() {
        let Some(font) = Font::system() else {
            eprintln!("keine Systemschrift gefunden – übersprungen");
            return;
        };
        let text = font.render("laptop", 15.0, 360.0).unwrap();
        assert!(text.width > 30 && text.width < 80, "{} Bildpunkte breit", text.width);
        assert!(text.height >= 15 && text.height < 30, "{} Bildpunkte hoch", text.height);
        assert!(text.coverage.contains(&255), "volle Deckung im Strich");
        assert!(text.coverage.iter().filter(|&&c| c == 0).count() > text.coverage.len() / 3, "Zwischenräume bleiben frei");
        // Doppelte Größe für 200 %
        let large = font.render("laptop", 30.0, 720.0).unwrap();
        assert!(large.width.abs_diff(text.width * 2) <= 3, "{} gegen {}", large.width, text.width);

        let long = font.render(&"sehr-langer-rechnername-".repeat(10), 15.0, 360.0).unwrap();
        assert!(long.width <= 360, "{}", long.width);
        assert!(font.render("  ", 15.0, 360.0).is_none());
        assert!(font.render("Büro-PC 😀", 15.0, 360.0).is_some(), "fehlende Zeichen stören nicht");
    }
}
