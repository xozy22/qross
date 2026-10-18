//! Text-Clipboard für Linux und macOS über `arboard`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::anyhow;

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
}

impl Clipboard for ArboardClipboard {
    fn text(&mut self) -> Option<String> {
        self.inner()?.get_text().ok()
    }

    fn set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.inner()
            .ok_or_else(|| anyhow!("Clipboard nicht verfügbar"))?
            .set_text(text.to_owned())
            .map_err(|e| anyhow!("Clipboard setzen: {e}"))
    }

    /// Ohne Änderungszähler im System: Hash des Inhalts – ändert sich mit jedem neuen Text.
    fn sequence(&mut self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.text().hash(&mut hasher);
        hasher.finish()
    }
}
