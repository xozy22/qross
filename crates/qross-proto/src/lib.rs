//! Wire-Protokoll von Qross. Beschreibung: `docs/protokoll.md`.
//!
//! Kanäle einer QUIC-Verbindung:
//! - **Control** (bidirektionaler Stream, vom Client geöffnet): Hello/Welcome, Bildschirmänderungen
//! - **Input** (unidirektionaler Stream Server → Client): Tasten, Maustasten, Rad, Enter/Leave
//! - **Datagramme** (Server → Client): Mausposition, unzuverlässig, die neueste gewinnt
//! - **Clipboard** (je Übertragung ein eigener unidirektionaler Stream, beide Richtungen)
//! - **Datei** (je Datei ein eigener unidirektionaler Stream, niedrige Priorität): Drag & Drop
//!   (Server → Client) und kopierte Dateien aus der Zwischenablage (beide Richtungen)

mod frame;
mod geometry;
mod input;

use serde::{Deserialize, Serialize};

pub use frame::{decode_datagram, encode_datagram, read_frame, write_frame, FrameError, MAX_FRAME_LEN};
pub use geometry::{Edge, Rect};
pub use input::{KeyCode, MouseButton};

pub const PROTOCOL_VERSION: u16 = 3;
/// ALPN-Kennung im TLS-Handshake
pub const ALPN: &[u8] = b"qross/3";
/// UDP-Port (bewusst nicht Synergys 24800, damit beide parallel laufen können)
pub const DEFAULT_PORT: u16 = 24900;
/// UDP-Port der Server-Suche im LAN
pub const DISCOVERY_PORT: u16 = 24901;

/// Betriebssystem einer Gegenstelle (z. B. für den Strg/Cmd-Tausch zwischen PC und Mac).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
    Other,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// Bildschirmfläche eines Rechners: Bounding-Box des virtuellen Desktops in physischen Pixeln.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenInfo {
    pub bounds: Rect,
}

/// Nachrichten auf dem Control-Stream.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Client → Server, erste Nachricht
    Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
    /// Server → Client: angenommen
    Welcome { version: u16, server_name: String, platform: Platform },
    /// Server → Client: abgelehnt, danach wird die Verbindung geschlossen
    Reject { reason: String },
    /// Client → Server: Auflösung oder Monitor-Anordnung hat sich geändert
    ScreenChanged(ScreenInfo),
}

/// Zuverlässige, geordnete Eingabe-Ereignisse (Server → Client).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Der Cursor betritt den Client-Bildschirm. `epoch` kennzeichnet diesen Besuch;
    /// Maus-Datagramme mit anderer Epoche werden verworfen.
    Enter { epoch: u16, x: i32, y: i32 },
    /// Der Cursor verlässt den Client – alle gedrückten Tasten loslassen.
    Leave,
    Key { key: KeyCode, down: bool },
    /// Enthält die Position, damit der Klick auch dann an der richtigen Stelle landet,
    /// wenn das letzte Bewegungs-Datagramm verloren ging.
    Button { button: MouseButton, down: bool, x: i32, y: i32 },
    /// Raddrehung, 120 = eine Raste (wie Windows `WHEEL_DELTA`)
    Wheel { dx: i16, dy: i16 },
    /// Der Cursor zieht Dateien herein. Die Inhalte folgen auf eigenen Datei-Streams;
    /// der Client drückt dabei keine Maustaste.
    DragStart { transfer: u64, entries: Vec<FileEntry> },
    /// Maustaste losgelassen: Dateien an der Position (x, y) ablegen.
    Drop { transfer: u64, x: i32, y: i32 },
}

/// Eintrag einer Dateiübertragung. `path` ist relativ, mit `/` getrennt, und beginnt mit dem
/// Namen der gezogenen Datei bzw. des gezogenen Ordners.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Beginn eines Datei-Streams; danach folgen genau `size` Rohbytes und ein `FileTrailer`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileHeader {
    pub transfer: u64,
    /// Index in `Input::DragStart::entries` bzw. `Clipboard::Files::entries`
    pub index: u32,
    pub path: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileTrailer {
    pub sha256: [u8; 32],
}

/// Absolute Mausposition in Client-Koordinaten.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseMove {
    pub epoch: u16,
    pub seq: u32,
    pub x: i32,
    pub y: i32,
}

/// Alles, was als QUIC-Datagramm übertragen wird.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datagram {
    MouseMove(MouseMove),
}

/// Nachricht auf einem Clipboard-Stream.
///
/// Neue Varianten werden nur hinten angefügt: Eine ältere Gegenstelle kann sie nicht lesen,
/// verwirft dann nur diesen einen Stream und bleibt verbunden.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Clipboard {
    Data(ClipboardData),
    /// Kopierte Dateien und Ordner (seit Qross 0.3). Die Inhalte folgen auf Streams vom Typ
    /// `StreamKind::ClipboardFile` mit derselben `transfer`-Nummer. Der Empfänger setzt seine
    /// Zwischenablage erst, wenn alle Dateien geprüft angekommen sind – dann auf die Dateien und
    /// zusätzlich auf `data` (z. B. Dateinamen als Text), soweit sein System beides zulässt.
    Files { transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
}

/// Inhalt der Zwischenablage. Oft liegen mehrere Darstellungen gleichzeitig vor (z. B. Text und
/// HTML aus dem Browser); übertragen werden alle, der Empfänger stellt alle bereit.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardData {
    pub text: Option<String>,
    /// HTML-Ausschnitt (ohne Windows-CF_HTML-Kopf)
    pub html: Option<String>,
    /// Bild, PNG-kodiert
    pub png: Option<Vec<u8>>,
}

impl ClipboardData {
    pub fn from_text(text: impl Into<String>) -> Self {
        Self { text: Some(text.into()), ..Self::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.png.is_none()
    }

    /// Kurzbeschreibung fürs Log, z. B. „Text, Bild (1,2 MB)“
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.text.is_some() {
            parts.push("Text".to_owned());
        }
        if self.html.is_some() {
            parts.push("HTML".to_owned());
        }
        if let Some(png) = &self.png {
            parts.push(format!("Bild ({:.1} MB)", png.len() as f64 / 1_000_000.0).replace('.', ","));
        }
        if parts.is_empty() {
            "leer".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

/// Erstes Byte jedes unidirektionalen Streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamKind {
    Input = 1,
    Clipboard = 2,
    /// Eine Datei für Drag & Drop: `FileHeader`-Frame, Rohbytes, `FileTrailer`-Frame
    File = 3,
    /// Eine Datei aus der Zwischenablage (`Clipboard::Files`), aufgebaut wie `File`.
    /// Ältere Gegenstellen kennen den Typ nicht und brechen nur diesen Stream ab.
    ClipboardFile = 4,
    /// Drag & Drop vom Client zum Server: eine `DragMessage` (seit Qross 0.5)
    Drag = 5,
}

/// Nachricht auf einem Stream vom Typ `StreamKind::Drag`: Drag & Drop vom Client zum Server.
///
/// Steht der Cursor auf einem Client mit gedrückter Taste an der Kante zum Server, fragt der
/// Server nach. Zieht der Client gerade Dateien, bricht er seinen Drag ab, bietet sie an und
/// schickt sie auf Streams vom Typ `File`. Ein älterer Client antwortet nie; der Server wechselt
/// dann mit gedrückter Taste nicht.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum DragMessage {
    /// Server → Client: Werden gerade Dateien gezogen?
    Query { probe: u32 },
    /// Client → Server: Antwort. Leere `entries` = kein Datei-Drag.
    Offer { probe: u32, transfer: u64, entries: Vec<FileEntry> },
}

impl StreamKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Input),
            2 => Some(Self::Clipboard),
            3 => Some(Self::File),
            4 => Some(Self::ClipboardFile),
            5 => Some(Self::Drag),
            _ => None,
        }
    }
}

/// Server-Suche im LAN per UDP-Broadcast an `DISCOVERY_PORT`.
/// Die Antwort ist nur ein Hinweis – vertraut wird erst nach dem Fingerprint-Abgleich beim Verbinden.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    Probe { version: u16 },
    Announce { version: u16, name: String, port: u16, fingerprint: String, platform: Platform },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// So sah `Clipboard` bis Qross 0.2 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum ClipboardV02 {
        Data(ClipboardData),
    }

    #[test]
    fn older_peers_read_text_and_reject_files_cleanly() {
        let data = ClipboardData::from_text("hallo");
        let bytes = postcard::to_stdvec(&Clipboard::Data(data.clone())).unwrap();
        assert_eq!(postcard::from_bytes::<ClipboardV02>(&bytes).unwrap(), ClipboardV02::Data(data));

        let files = Clipboard::Files {
            transfer: 7,
            entries: vec![FileEntry { path: "a.txt".into(), size: 3, is_dir: false }],
            data: ClipboardData::default(),
        };
        let bytes = postcard::to_stdvec(&files).unwrap();
        assert!(postcard::from_bytes::<ClipboardV02>(&bytes).is_err(), "ältere Version muss sauber ablehnen");
        assert_eq!(postcard::from_bytes::<Clipboard>(&bytes).unwrap(), files);
        assert_eq!(StreamKind::from_u8(StreamKind::ClipboardFile as u8), Some(StreamKind::ClipboardFile));
    }

    #[test]
    fn drag_messages_roundtrip() {
        for msg in [
            DragMessage::Query { probe: 3 },
            DragMessage::Offer { probe: 3, transfer: 9, entries: vec![FileEntry { path: "a".into(), size: 1, is_dir: false }] },
        ] {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            assert_eq!(postcard::from_bytes::<DragMessage>(&bytes).unwrap(), msg);
        }
        assert_eq!(StreamKind::from_u8(5), Some(StreamKind::Drag));
        assert_eq!(StreamKind::from_u8(6), None);
    }
}
