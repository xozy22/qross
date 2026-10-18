//! Zwischenablage für Linux und macOS über `arboard`: Text, HTML und Bilder.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::anyhow;
use qross_proto::ClipboardData;

use crate::clipformat::{png_to_rgba, rgba_to_png, Rgba};
use crate::Clipboard;

#[derive(Default)]
pub struct ArboardClipboard {
    /// Lebt so lange wie das Programm – unter X11 hält diese Instanz den Clipboard-Inhalt.
    inner: Option<arboard::Clipboard>,
}

impl ArboardClipboard {
    fn inner(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => self.inner = Some(clipboard),
                Err(e) => tracing::debug!("Clipboard nicht verfügbar: {e}"),
            }
        }
        self.inner.as_mut()
    }

    /// Rohinhalte ohne PNG-Kodierung (für `sequence`, das oft aufgerufen wird)
    fn raw(&mut self) -> Option<(Option<String>, Option<String>, Option<arboard::ImageData<'static>>)> {
        let cb = self.inner()?;
        let text = cb.get_text().ok().filter(|t| !t.is_empty());
        let html = cb.get().html().ok().filter(|h| !h.is_empty());
        let image = cb.get_image().ok();
        Some((text, html, image))
    }
}

impl Clipboard for ArboardClipboard {
    fn read(&mut self) -> Option<ClipboardData> {
        let (text, html, image) = self.raw()?;
        let png = image.and_then(|img| {
            let rgba = Rgba { width: img.width as u32, height: img.height as u32, pixels: img.bytes.into_owned() };
            rgba_to_png(&rgba).map_err(|e| tracing::debug!("Bild nicht kodierbar: {e:#}")).ok()
        });
        let data = ClipboardData { text, html, png };
        (!data.is_empty()).then_some(data)
    }

    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()> {
        let cb = self.inner().ok_or_else(|| anyhow!("Clipboard nicht verfügbar"))?;
        // arboard setzt jeweils eine Art. Ein Bild ohne Text ist ein Bild (Screenshot,
        // „Bild kopieren“); sonst gewinnt HTML mit Text als Alternative.
        let text_empty = data.text.as_deref().is_none_or(str::is_empty);
        let result = match (&data.png, &data.html, &data.text) {
            (Some(png), _, _) if text_empty => {
                let image = png_to_rgba(png)?;
                cb.set_image(arboard::ImageData {
                    width: image.width as usize,
                    height: image.height as usize,
                    bytes: image.pixels.into(),
                })
            }
            (_, Some(html), text) => cb.set_html(html.as_str(), text.as_deref()),
            (_, None, Some(text)) => cb.set_text(text.as_str()),
            _ => return Ok(()),
        };
        result.map_err(|e| anyhow!("Clipboard setzen: {e}"))
    }

    /// Ohne Änderungszähler im System: Hash des Inhalts – ändert sich mit jedem neuen Inhalt.
    fn sequence(&mut self) -> u64 {
        let mut hasher = DefaultHasher::new();
        if let Some((text, html, image)) = self.raw() {
            text.hash(&mut hasher);
            html.hash(&mut hasher);
            image.map(|i| (i.width, i.height, i.bytes.into_owned())).hash(&mut hasher);
        }
        hasher.finish()
    }
}
