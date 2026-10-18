//! Wire-Protokoll von Qross. Beschreibung: `docs/protokoll.md`.
//!
//! Kanäle einer QUIC-Verbindung:
//! - **Control** (bidirektionaler Stream, vom Client geöffnet): Hello/Welcome, Bildschirmänderungen,
//!   Rechte je Rechner, Bildschirmschoner
//! - **Input** (unidirektionaler Stream Server → Client): Tasten, Maustasten, Rad, Enter/Leave
//! - **Datagramme** (Server → Client): Mausposition bzw. im Spielemodus Mausbewegung,
//!   unzuverlässig, die neueste gewinnt
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

/// Version im `Hello`. Bleibt 3, solange das Protokoll nur abwärtskompatibel wächst, denn Server
/// bis Qross 0.3 lehnen jede andere ab.
pub const PROTOCOL_VERSION: u16 = 3;
/// Version im `Welcome`: welche Erweiterungen von Protokoll 3 der Server versteht. Clients bis
/// Qross 0.3 werten das Feld nicht aus.
/// - 3: Qross 0.2 und 0.3
/// - 4: `Control::Monitors` (seit Qross 0.5)
/// - 5: `Control::Game` und `Datagram::MouseDelta`, der Spielemodus (seit Qross 0.6)
/// - 6: `Control::TypesText`, `Input::Text` und `Input::Shortcut`: Zeichen statt Tastenposition
///   (seit Qross 0.7)
/// - 7: `Control::Sharing`, Rechte je Rechner (seit Qross 0.7)
/// - 11: `Control::FollowsScreenSaver` und `Control::ScreenSaver`, Bildschirmschoner abgleichen
///   (seit Qross 0.8)
pub const SERVER_VERSION: u16 = 11;
/// Ab dieser `Welcome`-Version darf der Client `Control::Monitors` senden. Ältere Server trennen
/// die Verbindung bei unbekannten Control-Nachrichten.
pub const MONITORS_SINCE: u16 = 4;
/// Ab dieser `Welcome`-Version darf der Client `Control::Game` senden. Ein Client, der sie
/// sendet, versteht damit auch `Datagram::MouseDelta`.
pub const GAME_MODE_SINCE: u16 = 5;
/// Ab dieser `Welcome`-Version darf der Client `Control::TypesText` senden. Nur wer das gesendet
/// hat, bekommt `Input::Text` und `Input::Shortcut`.
pub const TEXT_SINCE: u16 = 6;
/// Ab dieser `Welcome`-Version schickt der Server `Control::Sharing`: nach dem `Welcome` und bei
/// jeder Änderung der Rechte.
pub const SHARING_SINCE: u16 = 7;
/// Ab dieser `Welcome`-Version darf der Client `Control::FollowsScreenSaver` senden. Nur wer das
/// gesendet hat, bekommt `Control::ScreenSaver`.
pub const SCREEN_SAVER_SINCE: u16 = 11;
/// Höchstzahl Monitore, die der Server aus `Control::Monitors` übernimmt
pub const MAX_MONITORS: usize = 16;
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

/// Bildschirmfläche eines Rechners: Bounding-Box des virtuellen Desktops in physischen Pixeln
/// (macOS: Punkte). Die einzelnen Monitore meldet `Control::Monitors`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenInfo {
    pub bounds: Rect,
}

/// Nachrichten auf dem Control-Stream.
///
/// Neue Varianten werden nur hinten angefügt. Anders als bei `Clipboard` trennen ältere Server die
/// Verbindung, wenn sie eine Nachricht nicht lesen können; der Client sendet neue Varianten deshalb
/// nur, wenn die Version im `Welcome` sie abdeckt (z. B. `MONITORS_SINCE`). In der Gegenrichtung
/// lesen Clients bis Qross 0.6 nach dem `Welcome` nichts mehr; was der Server danach schickt, bleibt
/// bei ihnen ungelesen liegen. Neuere Clients überspringen Nachrichten, die sie nicht kennen.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Client → Server, erste Nachricht
    Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
    /// Server → Client: angenommen. `version`: siehe `SERVER_VERSION`
    Welcome { version: u16, server_name: String, platform: Platform },
    /// Server → Client: abgelehnt, danach wird die Verbindung geschlossen
    Reject { reason: String },
    /// Client → Server: Auflösung oder Monitor-Anordnung hat sich geändert
    ScreenChanged(ScreenInfo),
    /// Client → Server (seit Qross 0.5, nur ab `MONITORS_SINCE`): die einzelnen Monitore in denselben
    /// Koordinaten wie `ScreenInfo::bounds`. Folgt direkt auf das `Welcome` und nach jeder Änderung.
    /// Der Server hält den Cursor dann auf den Monitorflächen und wechselt nur an Kantenstücken,
    /// an denen ein Monitor an der äußeren Kante liegt.
    Monitors(Vec<Rect>),
    /// Client → Server (seit Qross 0.6, nur ab `GAME_MODE_SINCE`): Läuft im Vordergrund des Clients
    /// ein Vollbildprogramm, bzw. hat ein Spiel den Zeiger eingefangen? Kommt bei jeder Änderung,
    /// nach dem `Welcome` nur, wenn schon etwas läuft. Steht der Cursor dort, hält der Server ihn
    /// fest und schickt solange `Datagram::MouseDelta` statt `Datagram::MouseMove`.
    Game(GameState),
    /// Client → Server (seit Qross 0.7, nur ab `TEXT_SINCE`): Der Client kann Zeichen eingeben,
    /// unabhängig von seinem Tastaturlayout. Folgt direkt auf das `Welcome`. Ohne diese Meldung
    /// bekommt er nur Tastenpositionen, auch wenn er auf dem Server auf „Zeichen“ steht.
    TypesText,
    /// Server → Client (seit Qross 0.7, ab `SHARING_SINCE`): Was der Server mit diesem Client teilt.
    /// Folgt direkt auf das `Welcome` und kommt nach jeder Änderung erneut. Der Client zeigt das an
    /// und versucht Verbotenes gar nicht erst; durchgesetzt wird es ohnehin auf dem Server, auch
    /// gegenüber älteren Clients, die die Nachricht nie lesen.
    Sharing(Sharing),
    /// Client → Server (seit Qross 0.8, nur ab `SCREEN_SAVER_SINCE`): Der Client kann seinen
    /// Bildschirmschoner auf Zuruf starten und beenden. Folgt direkt auf das `Welcome`.
    FollowsScreenSaver,
    /// Server → Client (seit Qross 0.8, nur an Clients, die `FollowsScreenSaver` gemeldet haben):
    /// Der Bildschirmschoner des Servers hat begonnen (`active`) bzw. geendet. Der Client startet
    /// bzw. beendet daraufhin seinen eigenen. Mit `lock` (nur beim Beginn) sperrt er sich stattdessen.
    ScreenSaver { active: bool, lock: bool },
}

/// Rechte je Rechner: In welche Richtung eine Art von Inhalten zwischen Server und Client wandern
/// darf. „Vom Client“ heißt: zum Server und von dort zu anderen Rechnern, soweit diese empfangen
/// dürfen. In Konfigurationsdateien: `"both"`, `"to-client"`, `"from-client"`, `"off"`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Share {
    /// In beide Richtungen (Standard, wie vor Qross 0.7)
    #[default]
    Both,
    /// Nur vom Server zum Client
    ToClient,
    /// Nur vom Client zum Server
    FromClient,
    /// Gar nicht
    Off,
}

impl Share {
    /// Darf der Client so etwas bekommen?
    pub const fn to_client(self) -> bool {
        matches!(self, Self::Both | Self::ToClient)
    }

    /// Darf der Client so etwas abgeben?
    pub const fn from_client(self) -> bool {
        matches!(self, Self::Both | Self::FromClient)
    }
}

/// Was der Server mit einem Client teilt (`Control::Sharing`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sharing {
    /// Zwischenablage: Text, HTML und Bild (`Clipboard::Data` und was `Clipboard::Files` an Text
    /// mitbringt)
    pub clipboard: Share,
    /// Kopierte Dateien: `Clipboard::Files` und Streams vom Typ `ClipboardFile`
    pub files: Share,
    /// Drag & Drop: `Input::DragStart`, Streams vom Typ `File` und `Drag`
    pub drag: Share,
}

impl Sharing {
    /// Alles in beide Richtungen
    pub const ALL: Self = Self { clipboard: Share::Both, files: Share::Both, drag: Share::Both };
}

/// Spielemodus: Was auf einem Rechner gerade im Vordergrund läuft.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GameState {
    /// Das Programm im Vordergrund füllt einen ganzen Monitor (Spiel, Video, Präsentation).
    pub fullscreen: bool,
    /// Ein Programm hat den Zeiger eingefangen (Windows: `ClipCursor` auf weniger als den
    /// Bildschirm), wie es Spiele mit Maussteuerung tun.
    pub captured: bool,
}

impl GameState {
    /// Hält der Spielemodus den Cursor auf diesem Rechner?
    pub const fn is_active(self) -> bool {
        self.fullscreen || self.captured
    }
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
    /// Zeichenmodus (seit Qross 0.7, nur an Clients, die `Control::TypesText` gesendet haben): Die
    /// Taste `key` ergab auf dem Server `text`, z. B. „@“ mit AltGr+Q. Der Client gibt den Text ein,
    /// egal welches Tastaturlayout er hat, und hält das letzte Zeichen, bis `Key { key, down: false }`
    /// kommt. Eine Wiederholung (Taste gehalten) kommt als weiteres `Text` mit derselben Taste.
    Text { key: KeyCode, text: String },
    /// Zeichenmodus: Kürzel wie Strg+Z. Der Client drückt die Taste, die bei ihm `ch` ergibt (ohne
    /// Zusatztasten), und lässt sie bei `Key { key, down: false }` los. Kennt sein Layout das
    /// Zeichen nicht, nimmt er die Taste `key`.
    Shortcut { key: KeyCode, ch: char },
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

/// Relative Mausbewegung in Pixeln des Servers (Spielemodus). Teilt sich `epoch` und `seq` mit
/// `MouseMove`: Auch hier gilt nur, was neuer ist als das zuletzt übernommene Datagramm.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseDelta {
    pub epoch: u16,
    pub seq: u32,
    pub dx: i32,
    pub dy: i32,
}

/// Alles, was als QUIC-Datagramm übertragen wird.
///
/// Neue Varianten werden nur hinten angefügt. Ein älterer Client verwirft ein Datagramm, das er
/// nicht lesen kann (und bleibt verbunden); der Server schickt sie trotzdem nur an Clients, die
/// sie kennen.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datagram {
    MouseMove(MouseMove),
    /// Seit Qross 0.6, nur an Clients, die `Control::Game` gesendet haben
    MouseDelta(MouseDelta),
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

    /// So sah `Control` bis Qross 0.3 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum ControlV03 {
        Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
        Welcome { version: u16, server_name: String, platform: Platform },
        Reject { reason: String },
        ScreenChanged(ScreenInfo),
    }

    #[test]
    fn monitors_reach_only_servers_that_understand_them() {
        // Hello und ScreenChanged lesen ältere Server unverändert; Hello meldet weiter Version 3.
        let screen = ScreenInfo { bounds: Rect::new(0, -400, 5256, 1800) };
        let hello = Control::Hello { version: PROTOCOL_VERSION, name: "mac".into(), screen, platform: Platform::MacOs };
        let bytes = postcard::to_stdvec(&hello).unwrap();
        assert_eq!(
            postcard::from_bytes::<ControlV03>(&bytes).unwrap(),
            ControlV03::Hello { version: 3, name: "mac".into(), screen, platform: Platform::MacOs }
        );
        let bytes = postcard::to_stdvec(&Control::ScreenChanged(screen)).unwrap();
        assert_eq!(postcard::from_bytes::<ControlV03>(&bytes).unwrap(), ControlV03::ScreenChanged(screen));

        // Ältere Clients lesen das Welcome eines neuen Servers (die Version werten sie nicht aus).
        let welcome = Control::Welcome { version: SERVER_VERSION, server_name: "pc".into(), platform: Platform::Windows };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(postcard::from_bytes::<ControlV03>(&bytes).unwrap(), ControlV03::Welcome { version: SERVER_VERSION, .. }));

        // Monitors kann ein älterer Server nicht lesen (er würde trennen). Er meldet im Welcome
        // Version 3 und bekommt die Nachricht deshalb nie.
        const { assert!(PROTOCOL_VERSION < MONITORS_SINCE && MONITORS_SINCE <= SERVER_VERSION) };
        let monitors = Control::Monitors(vec![Rect::new(0, 418, 1512, 982), Rect::new(1512, -400, 3744, 1800)]);
        let bytes = postcard::to_stdvec(&monitors).unwrap();
        assert!(postcard::from_bytes::<ControlV03>(&bytes).is_err(), "ältere Version kann Monitors nicht lesen");
        assert_eq!(postcard::from_bytes::<Control>(&bytes).unwrap(), monitors);
    }

    /// So sah `Control` in Qross 0.5 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum ControlV05 {
        Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
        Welcome { version: u16, server_name: String, platform: Platform },
        Reject { reason: String },
        ScreenChanged(ScreenInfo),
        Monitors(Vec<Rect>),
    }

    /// So sah `Datagram` bis Qross 0.5 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum DatagramV05 {
        MouseMove(MouseMove),
    }

    #[test]
    fn game_mode_reaches_only_peers_that_understand_it() {
        // Ein Server aus 0.5 liest das Welcome-Format unverändert und meldet Version 4: Der Client
        // schickt ihm kein `Game`, das er nicht lesen könnte.
        const { assert!(MONITORS_SINCE < GAME_MODE_SINCE && GAME_MODE_SINCE <= SERVER_VERSION) };
        let welcome = Control::Welcome { version: SERVER_VERSION, server_name: "pc".into(), platform: Platform::Windows };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(postcard::from_bytes::<ControlV05>(&bytes).unwrap(), ControlV05::Welcome { version: SERVER_VERSION, .. }));
        let game = Control::Game(GameState { fullscreen: true, captured: false });
        let bytes = postcard::to_stdvec(&game).unwrap();
        assert!(postcard::from_bytes::<ControlV05>(&bytes).is_err(), "ein Server aus 0.5 kann Game nicht lesen");
        assert_eq!(postcard::from_bytes::<Control>(&bytes).unwrap(), game);

        // Ein älterer Client liest absolute Positionen wie bisher; ein Delta kann er nicht lesen
        // (er verwirft nur dieses Datagramm), bekommt aber auch keins.
        let mv = MouseMove { epoch: 2, seq: 9, x: 100, y: 200 };
        let bytes = crate::encode_datagram(&Datagram::MouseMove(mv));
        assert_eq!(postcard::from_bytes::<DatagramV05>(&bytes).unwrap(), DatagramV05::MouseMove(mv));
        let delta = Datagram::MouseDelta(MouseDelta { epoch: 2, seq: 10, dx: -3, dy: 40_000 });
        let bytes = crate::encode_datagram(&delta);
        assert!(postcard::from_bytes::<DatagramV05>(&bytes).is_err());
        assert_eq!(crate::decode_datagram(&bytes).unwrap(), delta);
        assert!(bytes.len() <= 12, "{} Bytes", bytes.len());

        assert!(!GameState::default().is_active());
        assert!(GameState { fullscreen: false, captured: true }.is_active());
    }

    /// So sah `Control` in Qross 0.6 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum ControlV06 {
        Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
        Welcome { version: u16, server_name: String, platform: Platform },
        Reject { reason: String },
        ScreenChanged(ScreenInfo),
        Monitors(Vec<Rect>),
        Game(GameState),
    }

    #[test]
    fn sharing_reaches_clients_that_read_it() {
        // Ein Client aus 0.6 liest das Welcome eines neuen Servers wie bisher; danach liest er den
        // Control-Stream nicht mehr. Ein neuer Client liest die Rechte.
        const { assert!(GAME_MODE_SINCE < SHARING_SINCE && SHARING_SINCE <= SERVER_VERSION) };
        let welcome = Control::Welcome { version: SERVER_VERSION, server_name: "pc".into(), platform: Platform::Linux };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(postcard::from_bytes::<ControlV06>(&bytes).unwrap(), ControlV06::Welcome { version: SERVER_VERSION, .. }));

        let sharing = Sharing { clipboard: Share::ToClient, files: Share::Off, drag: Share::FromClient };
        let bytes = postcard::to_stdvec(&Control::Sharing(sharing)).unwrap();
        assert!(postcard::from_bytes::<ControlV06>(&bytes).is_err(), "ein Client aus 0.6 könnte sie nicht lesen");
        assert_eq!(postcard::from_bytes::<Control>(&bytes).unwrap(), Control::Sharing(sharing));
        assert!(bytes.len() <= 4, "{} Bytes", bytes.len());
    }

    #[test]
    fn shares_name_their_directions() {
        let all = [Share::Both, Share::ToClient, Share::FromClient, Share::Off];
        let to: Vec<bool> = all.iter().map(|s| s.to_client()).collect();
        let from: Vec<bool> = all.iter().map(|s| s.from_client()).collect();
        assert_eq!(to, [true, true, false, false]);
        assert_eq!(from, [true, false, true, false]);
        assert_eq!(Sharing::default(), Sharing::ALL, "ohne Angabe ist alles erlaubt (wie vor Qross 0.7)");
    }

    /// So sah `Input` bis Qross 0.6 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum InputV06 {
        Enter { epoch: u16, x: i32, y: i32 },
        Leave,
        Key { key: KeyCode, down: bool },
        Button { button: MouseButton, down: bool, x: i32, y: i32 },
        Wheel { dx: i16, dy: i16 },
        DragStart { transfer: u64, entries: Vec<FileEntry> },
        Drop { transfer: u64, x: i32, y: i32 },
    }

    #[test]
    fn typed_text_reaches_only_peers_that_understand_it() {
        // Ein Server aus 0.6 meldet Version 5: Der Client schickt ihm kein `TypesText`.
        const { assert!(GAME_MODE_SINCE < TEXT_SINCE && TEXT_SINCE <= SERVER_VERSION) };
        let welcome = Control::Welcome { version: SERVER_VERSION, server_name: "pc".into(), platform: Platform::Windows };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(postcard::from_bytes::<ControlV06>(&bytes).unwrap(), ControlV06::Welcome { version: SERVER_VERSION, .. }));
        let bytes = postcard::to_stdvec(&Control::TypesText).unwrap();
        assert!(postcard::from_bytes::<ControlV06>(&bytes).is_err(), "ein Server aus 0.6 kann TypesText nicht lesen");
        assert_eq!(postcard::from_bytes::<Control>(&bytes).unwrap(), Control::TypesText);

        // Ein Client aus 0.6 liest Tasten wie bisher; Text und Kürzel bekommt er nie, denn er
        // könnte sie nicht lesen (der ganze Input-Stream bräche ab).
        let key = Input::Key { key: KeyCode::A, down: false };
        let bytes = postcard::to_stdvec(&key).unwrap();
        assert_eq!(postcard::from_bytes::<InputV06>(&bytes).unwrap(), InputV06::Key { key: KeyCode::A, down: false });
        for input in [Input::Text { key: KeyCode(0x14), text: "@".into() }, Input::Shortcut { key: KeyCode(0x1C), ch: 'z' }] {
            let bytes = postcard::to_stdvec(&input).unwrap();
            assert!(postcard::from_bytes::<InputV06>(&bytes).is_err(), "{input:?}");
            assert_eq!(postcard::from_bytes::<Input>(&bytes).unwrap(), input);
        }
        let emoji = Input::Text { key: KeyCode(0x08), text: "é😀".into() };
        assert_eq!(postcard::from_bytes::<Input>(&postcard::to_stdvec(&emoji).unwrap()).unwrap(), emoji);
    }

    /// So sah `Control` in Qross 0.7 aus.
    #[derive(Deserialize, Debug, PartialEq)]
    enum ControlV07 {
        Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
        Welcome { version: u16, server_name: String, platform: Platform },
        Reject { reason: String },
        ScreenChanged(ScreenInfo),
        Monitors(Vec<Rect>),
        Game(GameState),
        TypesText,
        Sharing(Sharing),
    }

    #[test]
    fn screen_saver_reaches_only_peers_that_understand_it() {
        // Ein Server aus 0.7 meldet Version 7: Der Client schickt ihm kein `FollowsScreenSaver`,
        // das er nicht lesen könnte (er würde trennen).
        const { assert!(SHARING_SINCE < SCREEN_SAVER_SINCE && SCREEN_SAVER_SINCE <= SERVER_VERSION) };
        let welcome = Control::Welcome { version: SERVER_VERSION, server_name: "pc".into(), platform: Platform::Windows };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(postcard::from_bytes::<ControlV07>(&bytes).unwrap(), ControlV07::Welcome { version: SERVER_VERSION, .. }));
        let bytes = postcard::to_stdvec(&Control::FollowsScreenSaver).unwrap();
        assert!(postcard::from_bytes::<ControlV07>(&bytes).is_err(), "ein Server aus 0.7 kann die Meldung nicht lesen");
        assert_eq!(postcard::from_bytes::<Control>(&bytes).unwrap(), Control::FollowsScreenSaver);

        // Ein Client aus 0.7 überspringt, was er nicht lesen kann; er bekommt die Nachricht aber
        // ohnehin nie, denn er meldet sich nicht dafür.
        for msg in [Control::ScreenSaver { active: true, lock: false }, Control::ScreenSaver { active: false, lock: false }] {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            assert!(postcard::from_bytes::<ControlV07>(&bytes).is_err(), "{msg:?}");
            assert_eq!(postcard::from_bytes::<Control>(&bytes).unwrap(), msg);
            assert!(bytes.len() <= 3, "{} Bytes", bytes.len());
        }
        // Rechte liest ein Client aus 0.7 weiter wie bisher
        let bytes = postcard::to_stdvec(&Control::Sharing(Sharing::ALL)).unwrap();
        assert_eq!(postcard::from_bytes::<ControlV07>(&bytes).unwrap(), ControlV07::Sharing(Sharing::ALL));
    }
}
