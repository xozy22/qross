//! Hauptfenster nur bei Bedarf: Solange Qross im Tray läuft, gibt es kein Fenster und damit auch keine
//! Browser-Prozesse (WebView2 bzw. WebKit brauchen gut 80 MB, Qross selbst unter 10 MB). Angelegt
//! wird es beim Zeigen; im Tray versteckt endet es nach [`CLOSE_AFTER`].
//!
//! Reine Logik ohne Tauri: wann das Fenster angelegt und zerstört wird, welche Aufträge auf die Seite
//! warten, Lage und Größe über das Zerstören hinweg. Das Fenster selbst steuert `main_window.rs`.

use std::time::Duration;

use serde_json::Value;

/// Name des Hauptfensters in tauri.conf.json
pub const LABEL: &str = "main";

/// So lange bleibt ein im Tray verstecktes Fenster bestehen. Wird es vorher wieder gezeigt, bleibt es.
pub const CLOSE_AFTER: Duration = Duration::from_secs(30);

/// Was beim Programmstart mit dem Fenster geschieht
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtStart {
    /// Normal gestartet: anlegen und zeigen
    Show,
    /// Autostart ohne Tray: anlegen und minimiert zeigen, sonst wäre Qross nicht zu erreichen
    Minimized,
    /// Autostart mit Tray: kein Fenster, bis es jemand öffnet
    Tray,
}

pub fn at_start(autostarted: bool, tray: bool) -> AtStart {
    match (autostarted, tray) {
        (false, _) => AtStart::Show,
        (true, false) => AtStart::Minimized,
        (true, true) => AtStart::Tray,
    }
}

/// Auftrag an die Seite: ein Ereignis wie `show-settings` samt Inhalt. Die Seite bekommt es erst, wenn
/// sie geladen ist (Befehl `ui_ready`) und das Fenster zu sehen.
#[derive(Clone, Debug, PartialEq)]
pub struct UiAction {
    pub event: &'static str,
    pub payload: Value,
}

/// Wo das Hauptfenster gerade steht
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Keins da: Start im Tray oder zerstört
    #[default]
    Absent,
    /// Wird gerade angelegt
    Creating,
    /// Zu sehen, auch minimiert
    Shown,
    /// Im Tray versteckt; endet nach [`CLOSE_AFTER`]
    Hidden,
    /// Wird zerstört
    Closing,
}

/// Was beim Zeigen zu tun ist
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// Vorhandenes Fenster nach vorn holen
    Reveal,
    /// Neu anlegen und zeigen
    Create,
    /// Nichts: Es wird schon angelegt, oder es wird gerade zerstört und danach neu angelegt.
    Wait,
}

/// Lebenslauf des Hauptfensters und die Aufträge, die auf seine Seite warten
#[derive(Debug, Default)]
pub struct Life {
    phase: Phase,
    /// Die Seite hat sich nach dem Laden gemeldet
    ready: bool,
    pending: Vec<UiAction>,
    /// Zählt Zeigen und Verstecken; ein Auftrag zum Zerstören gilt nur für seinen Stand
    generation: u64,
    /// Während des Zerstörens wieder verlangt: danach gleich neu anlegen
    reopen: bool,
}

impl Life {
    pub const fn new() -> Self {
        Self { phase: Phase::Absent, ready: false, pending: Vec::new(), generation: 0, reopen: false }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Zeigen verlangt (Tray, zweiter Programmstart, Dock). `exists`: Tauri kennt ein Fenster.
    pub fn show(&mut self, exists: bool) -> Show {
        match (self.phase, exists) {
            (Phase::Creating, _) => Show::Wait,
            (Phase::Closing, true) => {
                self.reopen = true;
                Show::Wait
            }
            (_, true) => {
                self.phase = Phase::Shown;
                self.generation += 1;
                Show::Reveal
            }
            (_, false) => {
                self.phase = Phase::Creating;
                self.ready = false;
                self.reopen = false;
                Show::Create
            }
        }
    }

    /// Angelegt; gleich zu sehen
    pub fn created(&mut self) {
        self.phase = Phase::Shown;
        self.generation += 1;
    }

    /// Anlegen gescheitert; das nächste Zeigen versucht es neu.
    pub fn create_failed(&mut self) {
        self.phase = Phase::Absent;
    }

    /// In den Tray versteckt. Liefert den Auftrag, das Fenster nach [`CLOSE_AFTER`] zu zerstören.
    pub fn hidden(&mut self) -> Option<u64> {
        if self.phase != Phase::Shown {
            return None;
        }
        self.phase = Phase::Hidden;
        self.generation += 1;
        Some(self.generation)
    }

    /// Die Frist ist um: zerstören, wenn das Fenster seitdem versteckt blieb.
    pub fn close_due(&mut self, ticket: u64) -> bool {
        let due = self.phase == Phase::Hidden && self.generation == ticket;
        if due {
            self.phase = Phase::Closing;
        }
        due
    }

    /// Zerstören gescheitert: Das Fenster bleibt versteckt.
    pub fn close_failed(&mut self) {
        if self.phase == Phase::Closing {
            self.phase = Phase::Hidden;
        }
    }

    /// Das Fenster ist weg. `true`: Inzwischen wurde es wieder verlangt, also neu anlegen.
    pub fn destroyed(&mut self) -> bool {
        if self.phase == Phase::Creating {
            // Das alte Fenster; ein neues entsteht schon
            return false;
        }
        self.phase = Phase::Absent;
        self.ready = false;
        std::mem::take(&mut self.reopen)
    }

    /// Die Seite lädt (neu): Aufträge warten, bis sie sich wieder meldet.
    pub fn page_loading(&mut self) {
        self.ready = false;
    }

    /// Die Seite ist bereit. Liefert, was jetzt an sie geht.
    pub fn page_ready(&mut self) -> Vec<UiAction> {
        self.ready = true;
        self.deliverable()
    }

    /// Neuer Auftrag an die Seite; liefert, was jetzt an sie geht. Gleiche Ereignisse warten nur
    /// einmal, mit dem neuesten Inhalt.
    pub fn action(&mut self, action: UiAction) -> Vec<UiAction> {
        self.pending.retain(|a| a.event != action.event);
        self.pending.push(action);
        self.deliverable()
    }

    /// Was jetzt an die Seite gehen kann: alles Wartende, sobald sie bereit und das Fenster zu sehen ist
    pub fn deliverable(&mut self) -> Vec<UiAction> {
        if self.ready && self.phase == Phase::Shown {
            std::mem::take(&mut self.pending)
        } else {
            Vec::new()
        }
    }
}

/// Rechteck in den Einheiten des Desktops: Pixel unter Windows und Linux, Punkte unter macOS. In diesen
/// Einheiten liegen alle Bildschirme in einem gemeinsamen Koordinatensystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Bounds {
    /// Breite und Höhe der Schnittfläche
    fn overlap(&self, other: &Bounds) -> (i64, i64) {
        let end = |start: i32, len: u32| i64::from(start) + i64::from(len);
        let width = end(self.x, self.width).min(end(other.x, other.width)) - i64::from(self.x.max(other.x));
        let height = end(self.y, self.height).min(end(other.y, other.height)) - i64::from(self.y.max(other.y));
        (width.max(0), height.max(0))
    }
}

/// Oberer Rand des Fensters, an dem es sich ziehen lässt (Kopfzeile der Seite)
const GRIP_HEIGHT: u32 = 40;
/// So viel davon muss auf einem Bildschirm liegen, damit die gemerkte Lage gilt
const GRIP_VISIBLE: (u32, u32) = (120, 24);

/// Lage, Größe und Maximiert-Zustand des Fensters über das Zerstören hinweg (nur im Speicher)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    /// Lage und Größe im normalen Zustand, weder maximiert noch minimiert
    normal: Option<Bounds>,
    maximized: bool,
}

/// Wie ein neu angelegtes Fenster erscheint
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restore {
    /// `None`: wie beim ersten Mal zentriert in Standardgröße
    pub bounds: Option<Bounds>,
    pub maximized: bool,
}

impl Placement {
    pub const fn new() -> Self {
        Self { normal: None, maximized: false }
    }

    /// Nach Verschieben und Größenänderung und vor dem Zerstören. Maximiert zählt nur der Zustand, die
    /// normale Lage bleibt die von vorher; minimiert zählt nichts.
    pub fn track(&mut self, bounds: Option<Bounds>, maximized: bool, minimized: bool) {
        if minimized {
            return;
        }
        self.maximized = maximized;
        if let Some(bounds) = bounds.filter(|_| !maximized) {
            self.normal = Some(bounds);
        }
    }

    /// Beim Neuanlegen. `screens`: Arbeitsbereiche der Bildschirme (ohne Taskleiste bzw. Dock). Liegt
    /// die Kopfzeile auf keinem mehr (Bildschirm abgezogen), zentriert wie beim ersten Mal.
    pub fn restore(&self, screens: &[Bounds]) -> Restore {
        Restore { bounds: self.normal.and_then(|bounds| fit(bounds, screens)), maximized: self.maximized }
    }
}

/// Die gemerkte Lage auf den Bildschirmen von jetzt: Sie gilt, wenn genug von der Kopfzeile auf einem
/// liegt, und wird nicht größer als dieser.
fn fit(bounds: Bounds, screens: &[Bounds]) -> Option<Bounds> {
    let grip = Bounds { height: bounds.height.min(GRIP_HEIGHT), ..bounds };
    let need = (i64::from(grip.width.min(GRIP_VISIBLE.0)), i64::from(grip.height.min(GRIP_VISIBLE.1)));
    let screen = screens.iter().find(|screen| {
        let (width, height) = grip.overlap(screen);
        width >= need.0 && height >= need.1
    })?;
    Some(Bounds { width: bounds.width.min(screen.width), height: bounds.height.min(screen.height), ..bounds })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(event: &'static str, payload: Value) -> UiAction {
        UiAction { event, payload }
    }

    /// Angelegt und zu sehen, Seite geladen
    fn open_window() -> Life {
        let mut life = Life::new();
        assert_eq!(life.show(false), Show::Create);
        life.created();
        assert!(life.page_ready().is_empty());
        life
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Bounds {
        Bounds { x, y, width, height }
    }

    #[test]
    fn autostart_with_tray_starts_without_a_window() {
        assert_eq!(at_start(false, true), AtStart::Show);
        assert_eq!(at_start(false, false), AtStart::Show);
        assert_eq!(at_start(true, true), AtStart::Tray);
        assert_eq!(at_start(true, false), AtStart::Minimized, "ohne Tray wäre Qross sonst unerreichbar");
    }

    #[test]
    fn hidden_window_closes_after_the_delay() {
        let mut life = open_window();
        let ticket = life.hidden().expect("versteckt: Auftrag zum Zerstören");
        assert_eq!(life.phase(), Phase::Hidden);
        assert!(life.close_due(ticket));
        assert_eq!(life.phase(), Phase::Closing);
        assert!(!life.close_due(ticket), "nur einmal");
        assert!(!life.destroyed(), "niemand hat es inzwischen verlangt");
        assert_eq!(life.phase(), Phase::Absent);
        assert_eq!(life.show(false), Show::Create, "Zeigen legt es neu an");
    }

    #[test]
    fn showing_again_in_time_keeps_the_window() {
        let mut life = open_window();
        let first = life.hidden().unwrap();
        assert_eq!(life.show(true), Show::Reveal);
        assert!(!life.close_due(first), "wieder gezeigt: bleibt");
        // Erneut versteckt: Es zählt nur die Frist ab dem letzten Verstecken
        let second = life.hidden().unwrap();
        assert!(!life.close_due(first));
        assert!(life.close_due(second));
    }

    #[test]
    fn only_a_shown_window_starts_the_delay() {
        let mut life = Life::new();
        assert_eq!(life.hidden(), None, "kein Fenster");
        assert_eq!(life.show(false), Show::Create);
        assert_eq!(life.hidden(), None, "wird noch angelegt");
        life.created();
        let ticket = life.hidden().unwrap();
        assert_eq!(life.hidden(), None, "schon versteckt, die erste Frist gilt");
        assert!(life.close_due(ticket));
    }

    #[test]
    fn showing_while_creating_or_closing_waits() {
        let mut life = Life::new();
        assert_eq!(life.show(false), Show::Create);
        assert_eq!(life.show(false), Show::Wait, "nicht zweimal anlegen");
        assert_eq!(life.show(true), Show::Wait);
        life.created();
        assert_eq!(life.show(true), Show::Reveal);

        // Während des Zerstörens verlangt: danach gleich neu anlegen
        let ticket = life.hidden().unwrap();
        assert!(life.close_due(ticket));
        assert_eq!(life.show(true), Show::Wait);
        assert!(life.destroyed());
        assert_eq!(life.show(false), Show::Create);
        life.created();
        let ticket = life.hidden().unwrap();
        assert!(life.close_due(ticket));
        assert!(!life.destroyed(), "nur das eine Mal neu anlegen");
    }

    #[test]
    fn window_gone_before_its_destroyed_event_is_created_anew() {
        let mut life = open_window();
        let ticket = life.hidden().unwrap();
        assert!(life.close_due(ticket));
        // Tauri kennt das Fenster schon nicht mehr, das Ereignis dazu kommt erst noch
        assert_eq!(life.show(false), Show::Create);
        assert!(!life.destroyed(), "gilt dem alten Fenster");
        assert_eq!(life.phase(), Phase::Creating);
        life.created();
        assert_eq!(life.phase(), Phase::Shown);
    }

    #[test]
    fn failures_leave_a_usable_state() {
        let mut life = Life::new();
        assert_eq!(life.show(false), Show::Create);
        life.create_failed();
        assert_eq!(life.phase(), Phase::Absent);
        assert_eq!(life.show(false), Show::Create, "neuer Versuch");
        life.created();
        let ticket = life.hidden().unwrap();
        assert!(life.close_due(ticket));
        life.close_failed();
        assert_eq!(life.phase(), Phase::Hidden);
        assert_eq!(life.show(true), Show::Reveal);
        // Fenster unerwartet weg: Zeigen legt es neu an
        assert_eq!(life.show(false), Show::Create);
    }

    #[test]
    fn actions_wait_for_the_page_and_a_visible_window() {
        let news = action("show-news", Value::Null);
        let start = action("start-from-tray", Value::Null);
        let mut life = Life::new();
        assert!(life.action(news.clone()).is_empty(), "noch kein Fenster");
        assert_eq!(life.show(false), Show::Create);
        life.created();
        assert!(life.deliverable().is_empty(), "die Seite lädt noch");
        assert_eq!(life.page_ready(), vec![news.clone()]);
        assert!(life.deliverable().is_empty(), "nur einmal");

        assert_eq!(life.action(news.clone()), vec![news.clone()], "bereit und zu sehen: sofort");

        // Im Tray versteckt: wartet aufs nächste Zeigen
        life.hidden().unwrap();
        assert!(life.action(start.clone()).is_empty());
        assert_eq!(life.show(true), Show::Reveal);
        assert_eq!(life.deliverable(), vec![start]);

        // Neu laden (Sprachwechsel): wartet, bis sich die Seite wieder meldet
        life.page_loading();
        assert!(life.action(news.clone()).is_empty());
        assert_eq!(life.page_ready(), vec![news.clone()]);

        // Zerstört: Die nächste Seite meldet sich erst nach dem Neuanlegen
        let ticket = life.hidden().unwrap();
        assert!(life.close_due(ticket));
        life.destroyed();
        assert!(life.action(news.clone()).is_empty());
        assert_eq!(life.show(false), Show::Create);
        life.created();
        assert!(life.deliverable().is_empty(), "alte Seite zählt nicht");
        assert_eq!(life.page_ready(), vec![news]);
    }

    #[test]
    fn same_actions_wait_once_with_the_latest_payload() {
        let mut life = Life::new();
        life.action(action("show-settings", Value::Null));
        life.action(action("show-news", Value::Null));
        life.action(action("show-settings", Value::from("set-profiles")));
        life.action(action("start-from-tray", Value::Null));
        life.action(action("start-from-tray", Value::Null));
        life.show(false);
        life.created();
        assert_eq!(
            life.page_ready(),
            vec![
                action("show-news", Value::Null),
                action("show-settings", Value::from("set-profiles")),
                action("start-from-tray", Value::Null),
            ]
        );
    }

    #[test]
    fn first_window_is_centered() {
        let screens = [rect(0, 0, 1920, 1040)];
        assert_eq!(Placement::new().restore(&screens), Restore { bounds: None, maximized: false });
        assert_eq!(Placement::default(), Placement::new());
    }

    #[test]
    fn placement_keeps_the_normal_bounds_and_the_maximized_state() {
        let screens = [rect(0, 0, 1920, 1040)];
        let normal = rect(200, 100, 1080, 760);
        let mut placement = Placement::new();
        placement.track(Some(normal), false, false);
        assert_eq!(placement.restore(&screens), Restore { bounds: Some(normal), maximized: false });

        // Maximiert: Die normale Lage bleibt, dazu der Zustand
        placement.track(Some(rect(-8, -8, 1936, 1056)), true, false);
        assert_eq!(placement.restore(&screens), Restore { bounds: Some(normal), maximized: true });

        // Minimiert (Windows meldet -32000) zählt nicht
        placement.track(Some(rect(-32000, -32000, 0, 0)), false, true);
        assert_eq!(placement.restore(&screens), Restore { bounds: Some(normal), maximized: true });

        // Wiederhergestellt und verschoben
        let moved = rect(300, 120, 900, 700);
        placement.track(Some(moved), false, false);
        assert_eq!(placement.restore(&screens), Restore { bounds: Some(moved), maximized: false });

        // Lage nicht lesbar: Zustand zählt trotzdem
        placement.track(None, true, false);
        assert_eq!(placement.restore(&screens), Restore { bounds: Some(moved), maximized: true });
    }

    #[test]
    fn lost_screens_fall_back_to_the_centered_default() {
        let laptop = rect(0, 0, 1920, 1040);
        let monitor = rect(1920, 0, 2560, 1400);
        let on_monitor = rect(2400, 200, 1080, 760);
        let mut placement = Placement::new();
        placement.track(Some(on_monitor), false, false);
        assert_eq!(placement.restore(&[laptop, monitor]).bounds, Some(on_monitor));
        assert_eq!(placement.restore(&[laptop]).bounds, None, "Monitor abgezogen");
        assert_eq!(placement.restore(&[]).bounds, None, "keine Bildschirme gemeldet");

        placement.track(Some(on_monitor), true, false);
        assert_eq!(placement.restore(&[laptop]), Restore { bounds: None, maximized: true }, "maximiert auf dem verbliebenen");
    }

    #[test]
    fn window_partly_off_screen_stays_if_its_top_can_be_grabbed() {
        let screens = [rect(0, 0, 1920, 1040)];
        let mut placement = Placement::new();
        let left = rect(-900, 300, 1080, 760);
        placement.track(Some(left), false, false);
        assert_eq!(placement.restore(&screens).bounds, Some(left), "180 Pixel der Kopfzeile sichtbar");

        placement.track(Some(rect(-1000, 300, 1080, 760)), false, false);
        assert_eq!(placement.restore(&screens).bounds, None, "nur 80 Pixel sichtbar");

        placement.track(Some(rect(400, 1030, 1080, 760)), false, false);
        assert_eq!(placement.restore(&screens).bounds, None, "Kopfzeile unter der Taskleiste");

        placement.track(Some(rect(400, -30, 1080, 760)), false, false);
        assert_eq!(placement.restore(&screens).bounds, None, "Kopfzeile über dem Bildschirm");

        let low = rect(400, 1000, 1080, 760);
        placement.track(Some(low), false, false);
        assert_eq!(placement.restore(&screens).bounds, Some(low), "Kopfzeile ganz unten noch greifbar");
    }

    #[test]
    fn size_is_capped_to_the_screen() {
        let mut placement = Placement::new();
        placement.track(Some(rect(0, 0, 2500, 1400)), false, false);
        assert_eq!(placement.restore(&[rect(0, 0, 1920, 1040)]).bounds, Some(rect(0, 0, 1920, 1040)));
        // Auf einem zweiten Bildschirm zählt dessen Größe
        placement.track(Some(rect(2000, 100, 2500, 1400)), false, false);
        let screens = [rect(0, 0, 1920, 1040), rect(1920, 0, 2560, 1400)];
        assert_eq!(placement.restore(&screens).bounds, Some(rect(2000, 100, 2500, 1400)));
    }
}
