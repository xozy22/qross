use std::path::PathBuf;

use qross_proto::{Edge, KeyCode, MouseButton};

/// Was die Eingabe-Erfassung des Servers meldet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvent {
    /// Lokaler Modus: Der Cursor hat eine Kante des eigenen Bildschirms erreicht.
    EdgeHit { edge: Edge, x: i32, y: i32, buttons_down: bool },
    /// Remote-Modus: relative Mausbewegung (lokal abgefangen)
    MouseDelta { dx: i32, dy: i32 },
    Button { button: MouseButton, down: bool },
    Wheel { dx: i16, dy: i16 },
    Key { key: KeyCode, down: bool },
    /// Notfall-Hotkey: Die Erfassung hat sich bereits selbst auf lokal zurückgeschaltet.
    PanicReturn,
    /// Rollen-Taste: Cursor auf dem aktuellen Bildschirm festhalten bzw. wieder freigeben.
    ToggleLock,
    /// Antwort auf `Action::ProbeDrag`: die gezogenen Dateien, leer wenn kein Datei-Drag lief.
    DragProbeResult { files: Vec<PathBuf> },
    /// Nach `Action::AwaitLocalDrop`: Maustaste hier losgelassen – Dateien vom Client ablegen.
    LocalDrop { x: i32, y: i32 },
}

/// Was der Client in sein Betriebssystem einspeisen soll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inject {
    MoveTo { x: i32, y: i32 },
    /// Relativ bewegen (Spielemodus), so wie eine echte Maus: Spiele mit eingefangenem Zeiger
    /// lesen die Bewegung, nicht die Position.
    MoveBy { dx: i32, dy: i32 },
    Button { button: MouseButton, down: bool },
    Wheel { dx: i16, dy: i16 },
    Key { key: KeyCode, down: bool },
}
