//! Wire-Protokoll von Qross. Beschreibung: `docs/protokoll.md`.
//!
//! Kanäle einer QUIC-Verbindung:
//! - **Control** (bidirektionaler Stream, vom Client geöffnet): Hello/Welcome, Bildschirmänderungen
//! - **Input** (unidirektionaler Stream Server → Client): Tasten, Maustasten, Rad, Enter/Leave
//! - **Datagramme** (Server → Client): Mausposition, unzuverlässig, die neueste gewinnt
//! - **Clipboard** (je Übertragung ein eigener unidirektionaler Stream, beide Richtungen)
//! - **Datei** (je Datei ein eigener unidirektionaler Stream, niedrige Priorität)

mod frame;
mod geometry;
mod input;

use serde::{Deserialize, Serialize};

pub use frame::{decode_datagram, encode_datagram, read_frame, write_frame, FrameError, MAX_FRAME_LEN};
pub use geometry::{Edge, Rect};
pub use input::{KeyCode, MouseButton};

pub const PROTOCOL_VERSION: u16 = 2;
/// ALPN-Kennung im TLS-Handshake
pub const ALPN: &[u8] = b"qross/2";
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
    /// Index in `DragStart::entries`
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Clipboard {
    Text(String),
}

/// Erstes Byte jedes unidirektionalen Streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamKind {
    Input = 1,
    Clipboard = 2,
    /// Eine Datei: `FileHeader`-Frame, Rohbytes, `FileTrailer`-Frame
    File = 3,
}

impl StreamKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Input),
            2 => Some(Self::Clipboard),
            3 => Some(Self::File),
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
