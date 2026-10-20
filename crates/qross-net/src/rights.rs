//! Rechte je Rechner in der Netzschicht: Was von einem Zwischenablage-Inhalt in eine Richtung
//! gehen darf. Text, HTML und Bild zählen zur Zwischenablage, kopierte Dateien sind eine eigene
//! Art; was `Clipboard::Files` an Text mitbringt (etwa die Dateinamen), zählt zur Zwischenablage.

use qross_proto::{Clipboard, ClipboardData};

use crate::clipboard::Content;

/// Was in eine Richtung erlaubt ist (aus `Share::to_client` bzw. `Share::from_client`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Allowed {
    /// Text, HTML, Bild
    pub clipboard: bool,
    /// Kopierte Dateien
    pub files: bool,
}

impl Allowed {
    pub fn any(self) -> bool {
        self.clipboard || self.files
    }
}

/// Was von `content` gesendet werden darf (`None`: nichts).
pub(crate) fn content(content: Content, allowed: Allowed) -> Option<Content> {
    match content {
        Content::Data(data) => allowed.clipboard.then_some(Content::Data(data)),
        Content::Files { paths, data } if allowed.files => {
            Some(Content::Files { paths, data: if allowed.clipboard { data } else { ClipboardData::default() } })
        }
        Content::Files { data, .. } => (allowed.clipboard && !data.is_empty()).then_some(Content::Data(data)),
    }
}

/// Was von einer angekommenen Nachricht übernommen werden darf (`None`: nichts). Kopierte
/// Dateien ohne Recht darauf werden zu ihrem Text; ihre Streams lehnt der Empfänger ab, Dateien auf
/// Abruf holt er nicht.
pub(crate) fn message(message: Clipboard, allowed: Allowed) -> Option<Clipboard> {
    match message {
        Clipboard::Data(data) => allowed.clipboard.then_some(Clipboard::Data(data)),
        Clipboard::Files { transfer, entries, data } if allowed.files => Some(Clipboard::Files {
            transfer,
            entries,
            data: if allowed.clipboard { data } else { ClipboardData::default() },
        }),
        Clipboard::Files { data, .. } => (allowed.clipboard && !data.is_empty()).then_some(Clipboard::Data(data)),
        Clipboard::FilesOnDemand { transfer, entries, data } if allowed.files => Some(Clipboard::FilesOnDemand {
            transfer,
            entries,
            data: if allowed.clipboard { data } else { ClipboardData::default() },
        }),
        Clipboard::FilesOnDemand { data, .. } => (allowed.clipboard && !data.is_empty()).then_some(Clipboard::Data(data)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qross_proto::FileEntry;
    use std::path::PathBuf;

    const ALL: Allowed = Allowed { clipboard: true, files: true };
    const TEXT: Allowed = Allowed { clipboard: true, files: false };
    const FILES: Allowed = Allowed { clipboard: false, files: true };
    const NOTHING: Allowed = Allowed { clipboard: false, files: false };

    fn files(text: &str) -> Content {
        Content::Files { paths: vec![PathBuf::from("Bericht.pdf")], data: ClipboardData::from_text(text) }
    }

    #[test]
    fn local_content_is_cut_down_to_what_may_go() {
        let text = Content::Data(ClipboardData::from_text("geheim"));
        assert_eq!(content(text.clone(), ALL), Some(text.clone()));
        assert_eq!(content(text.clone(), FILES), None, "Text ist Zwischenablage");
        assert_eq!(content(text, NOTHING), None);

        assert_eq!(content(files("Bericht.pdf"), ALL), Some(files("Bericht.pdf")));
        assert_eq!(content(files("Bericht.pdf"), TEXT), Some(Content::Data(ClipboardData::from_text("Bericht.pdf"))), "nur die Namen");
        let Some(Content::Files { data, .. }) = content(files("Bericht.pdf"), FILES) else { panic!("Dateien dürfen") };
        assert!(data.is_empty(), "ohne den Text");
        let bare = Content::Files { paths: vec![PathBuf::from("Bild.png")], data: ClipboardData::default() };
        assert_eq!(content(bare, TEXT), None, "ohne Text bleibt nichts");
        assert!(!NOTHING.any() && TEXT.any() && FILES.any());
    }

    #[test]
    fn arriving_messages_are_cut_down_to_what_is_allowed() {
        let data = Clipboard::Data(ClipboardData::from_text("x"));
        assert_eq!(message(data.clone(), TEXT), Some(data.clone()));
        assert_eq!(message(data, FILES), None);

        let entries = vec![FileEntry { path: "a.txt".into(), size: 1, is_dir: false }];
        let files = Clipboard::Files { transfer: 3, entries: entries.clone(), data: ClipboardData::from_text("a.txt") };
        assert_eq!(message(files.clone(), ALL), Some(files.clone()));
        assert_eq!(message(files.clone(), TEXT), Some(Clipboard::Data(ClipboardData::from_text("a.txt"))));
        assert_eq!(
            message(files.clone(), FILES),
            Some(Clipboard::Files { transfer: 3, entries, data: ClipboardData::default() })
        );
        assert_eq!(message(files, NOTHING), None);

        let entries = vec![FileEntry { path: "Film.mov".into(), size: 4_000_000_000, is_dir: false }];
        let on_demand = Clipboard::FilesOnDemand { transfer: 4, entries: entries.clone(), data: ClipboardData::from_text("Film.mov") };
        assert_eq!(message(on_demand.clone(), ALL), Some(on_demand.clone()));
        assert_eq!(message(on_demand.clone(), TEXT), Some(Clipboard::Data(ClipboardData::from_text("Film.mov"))), "nur der Name");
        assert_eq!(
            message(on_demand.clone(), FILES),
            Some(Clipboard::FilesOnDemand { transfer: 4, entries, data: ClipboardData::default() })
        );
        assert_eq!(message(on_demand, NOTHING), None);
    }
}
