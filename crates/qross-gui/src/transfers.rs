//! Fortschrittsanzeige laufender Dateiübertragungen (Drag & Drop in beide Richtungen, kopierte
//! Dateien): Texte für das Fenster und die Systembenachrichtigung am Ende.

use std::time::Duration;

use qross_net::progress::{Direction, Kind, Phase, Progress};
use serde::Serialize;

use crate::clipboard_files::{names_text, size_text};

/// Lief eine Übertragung mindestens so lange, meldet sich ihr Ende auch als Systembenachrichtigung
/// (sofern das Fenster im Hintergrund ist). Gilt auch für „Kopierte Dateien sind bereit“.
pub const LONG_TRANSFER: Duration = Duration::from_secs(3);

/// Eine Zeile der Fortschrittsanzeige (Ereignis `transfer`)
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct TransferView {
    /// Eindeutig je Richtung und Übertragung, z. B. „send-17“; die Nummern selbst sind zu groß für
    /// JavaScript. Geht unverändert an `cancel_transfer`.
    pub id: String,
    /// `sending` oder `receiving`
    pub direction: &'static str,
    /// `drag` oder `clipboard`
    pub kind: &'static str,
    pub peer: String,
    /// „Bericht.pdf“, „a und b“, „„a“ und 3 weitere“
    pub name: String,
    /// „an „laptop““ bzw. „von „pc““
    pub route: String,
    /// 0 bis 100
    pub percent: f64,
    /// „1,2 GB von 3 GB“
    pub size: String,
    /// Läuft: „85 MB/s · noch 12 s“; danach z. B. „3 GB in 40 s“
    pub speed: String,
    /// `running`, `done`, `cancelled` oder `failed`
    pub state: &'static str,
    /// Ergebnis in Worten, sobald die Übertragung zu Ende ist
    pub outcome: Option<String>,
    pub elapsed_ms: u64,
}

/// Die Anzeige zu einer Meldung aus `qross_net`
pub fn view(p: &Progress) -> TransferView {
    let peer = &p.peer;
    let (prefix, route) = match p.direction {
        Direction::Sending => ("send", format!("an „{peer}“")),
        Direction::Receiving => ("recv", format!("von „{peer}“")),
    };
    let percent = if p.total == 0 { 100.0 } else { (p.bytes as f64 * 100.0 / p.total as f64).min(100.0) };
    let state = match p.phase {
        Phase::Running => "running",
        Phase::Done => "done",
        Phase::Cancelled { .. } => "cancelled",
        Phase::Failed(_) => "failed",
    };
    let speed = match p.phase {
        Phase::Running => running_speed(p),
        Phase::Done => format!("{} in {}", size_text(p.total), duration_text(p.elapsed)),
        _ => String::new(),
    };
    TransferView {
        id: format!("{prefix}-{}", p.transfer),
        direction: match p.direction {
            Direction::Sending => "sending",
            Direction::Receiving => "receiving",
        },
        kind: match p.kind {
            Kind::Drag => "drag",
            Kind::Clipboard => "clipboard",
        },
        peer: peer.clone(),
        name: names_text(&p.names),
        route,
        percent: (percent * 10.0).round() / 10.0,
        size: format!("{} von {}", size_text(p.bytes), size_text(p.total)),
        speed,
        state,
        outcome: outcome(p),
        elapsed_ms: p.elapsed.as_millis() as u64,
    }
}

/// „85 MB/s · noch 12 s“; ohne Schätzung nur die Rate, ohne Durchsatz „wartet …“
fn running_speed(p: &Progress) -> String {
    if p.bytes_per_s == 0 {
        return if p.elapsed >= Duration::from_secs(1) { "wartet …".into() } else { String::new() };
    }
    let rate = format!("{}/s", size_text(p.bytes_per_s));
    match p.remaining {
        Some(left) => format!("{rate} · {}", remaining_text(left)),
        None => rate,
    }
}

fn outcome(p: &Progress) -> Option<String> {
    let peer = &p.peer;
    Some(match &p.phase {
        Phase::Running => return None,
        Phase::Done => match p.direction {
            Direction::Sending => format!("Bei „{peer}“ angekommen"),
            Direction::Receiving => "Angekommen".into(),
        },
        Phase::Cancelled { by_peer: false } => "Abgebrochen".into(),
        Phase::Cancelled { by_peer: true } => format!("„{peer}“ hat abgebrochen"),
        Phase::Failed(reason) => format!("Fehlgeschlagen: {reason}"),
    })
}

/// „noch 42 s“, „noch 12 min“, „noch 2 h 5 min“ (aufgerundet, die Schätzung schwankt ohnehin)
pub fn remaining_text(left: Duration) -> String {
    let secs = left.as_secs_f64().ceil().max(1.0) as u64;
    if secs < 60 {
        format!("noch {secs} s")
    } else if secs < 3600 {
        format!("noch {} min", secs.div_ceil(60))
    } else {
        let minutes = secs.div_ceil(60);
        match minutes % 60 {
            0 => format!("noch {} h", minutes / 60),
            rest => format!("noch {} h {rest} min", minutes / 60),
        }
    }
}

/// „0,4 s“, „40 s“, „2 min 5 s“, „1 h 3 min“
pub fn duration_text(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 10 {
        format!("{:.1} s", d.as_secs_f64()).replace('.', ",")
    } else if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        match secs % 60 {
            0 => format!("{} min", secs / 60),
            rest => format!("{} min {rest} s", secs / 60),
        }
    } else {
        format!("{} h {} min", secs / 3600, secs % 3600 / 60)
    }
}

/// Titel und Text der Systembenachrichtigung am Ende einer längeren Übertragung, sonst `None`.
/// Kein Hinweis für einen Abbruch auf diesem Rechner (der Nutzer weiß es) und für angekommene
/// kopierte Dateien: Für die meldet sich „Kopierte Dateien sind bereit“ mit dem Einfügen-Hinweis.
pub fn end_notice(p: &Progress) -> Option<(String, String)> {
    if p.elapsed < LONG_TRANSFER {
        return None;
    }
    let (name, peer) = (names_text(&p.names), &p.peer);
    match (&p.phase, p.direction) {
        (Phase::Running | Phase::Cancelled { by_peer: false }, _) => None,
        (Phase::Done, Direction::Receiving) if p.kind == Kind::Clipboard => None,
        (Phase::Done, Direction::Sending) => {
            Some(("Übertragung fertig".into(), format!("{name} ist bei „{peer}“ angekommen ({})", size_text(p.total))))
        }
        (Phase::Done, Direction::Receiving) => {
            Some(("Dateien angekommen".into(), format!("{name} von „{peer}“ ist da ({})", size_text(p.total))))
        }
        (Phase::Cancelled { by_peer: true }, _) => {
            Some(("Übertragung abgebrochen".into(), format!("„{peer}“ hat die Übertragung von {name} abgebrochen")))
        }
        (Phase::Failed(reason), _) => Some(("Übertragung fehlgeschlagen".into(), format!("{name}: {reason}"))),
    }
}

/// Zurück aus der `id` einer [`TransferView`]: Richtung und Nummer
pub fn parse_id(id: &str) -> Option<(Direction, u64)> {
    let (prefix, number) = id.split_once('-')?;
    let direction = match prefix {
        "send" => Direction::Sending,
        "recv" => Direction::Receiving,
        _ => return None,
    };
    Some((direction, number.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(direction: Direction, phase: Phase) -> Progress {
        Progress {
            transfer: 17_234_567_890_123_456_789,
            direction,
            kind: Kind::Drag,
            peer: "laptop".into(),
            names: vec!["Film.mkv".into()],
            total: 3_000_000_000,
            bytes: 1_260_000_000,
            bytes_per_s: 85_000_000,
            remaining: Some(Duration::from_millis(20_400)),
            elapsed: Duration::from_secs(15),
            phase,
        }
    }

    #[test]
    fn running_transfers_show_size_rate_and_remaining_time() {
        let v = view(&progress(Direction::Sending, Phase::Running));
        assert_eq!(v.id, "send-17234567890123456789");
        assert_eq!((v.direction, v.kind, v.state), ("sending", "drag", "running"));
        assert_eq!((v.name.as_str(), v.route.as_str()), ("Film.mkv", "an „laptop“"));
        assert_eq!(v.percent, 42.0);
        assert_eq!(v.size, "1,3 GB von 3 GB");
        assert_eq!(v.speed, "85 MB/s · noch 21 s");
        assert_eq!(v.outcome, None);

        let early = Progress { remaining: None, ..progress(Direction::Receiving, Phase::Running) };
        assert_eq!(view(&early).speed, "85 MB/s", "ohne Schätzung nur die Rate");
        assert_eq!(view(&early).route, "von „laptop“");
        let stalled = Progress { bytes_per_s: 0, remaining: None, ..early };
        assert_eq!(view(&stalled).speed, "wartet …");
    }

    #[test]
    fn the_end_is_described() {
        let done = view(&progress(Direction::Sending, Phase::Done));
        assert_eq!((done.state, done.outcome.as_deref()), ("done", Some("Bei „laptop“ angekommen")));
        assert_eq!(done.speed, "3 GB in 15 s");
        let cancelled = view(&progress(Direction::Receiving, Phase::Cancelled { by_peer: true }));
        assert_eq!((cancelled.state, cancelled.outcome.as_deref()), ("cancelled", Some("„laptop“ hat abgebrochen")));
        assert_eq!(cancelled.speed, "");
        let failed = view(&progress(Direction::Receiving, Phase::Failed("Verbindung getrennt".into())));
        assert_eq!(failed.outcome.as_deref(), Some("Fehlgeschlagen: Verbindung getrennt"));
    }

    #[test]
    fn empty_folders_count_as_complete() {
        let folders = Progress { total: 0, bytes: 0, ..progress(Direction::Receiving, Phase::Running) };
        assert_eq!(view(&folders).percent, 100.0);
    }

    #[test]
    fn times_are_short_and_german() {
        assert_eq!(remaining_text(Duration::from_millis(300)), "noch 1 s");
        assert_eq!(remaining_text(Duration::from_secs(42)), "noch 42 s");
        assert_eq!(remaining_text(Duration::from_secs(61)), "noch 2 min");
        assert_eq!(remaining_text(Duration::from_secs(3600)), "noch 1 h");
        assert_eq!(remaining_text(Duration::from_secs(7500)), "noch 2 h 5 min");
        assert_eq!(duration_text(Duration::from_millis(2400)), "2,4 s");
        assert_eq!(duration_text(Duration::from_secs(40)), "40 s");
        assert_eq!(duration_text(Duration::from_secs(125)), "2 min 5 s");
        assert_eq!(duration_text(Duration::from_secs(180)), "3 min");
        assert_eq!(duration_text(Duration::from_secs(3780)), "1 h 3 min");
    }

    #[test]
    fn system_notices_only_for_long_transfers_not_cancelled_here() {
        let short = Progress { elapsed: Duration::from_secs(2), ..progress(Direction::Sending, Phase::Done) };
        assert_eq!(end_notice(&short), None);
        assert_eq!(end_notice(&progress(Direction::Sending, Phase::Running)), None);
        assert_eq!(end_notice(&progress(Direction::Sending, Phase::Cancelled { by_peer: false })), None);
        let (title, text) = end_notice(&progress(Direction::Sending, Phase::Done)).unwrap();
        assert_eq!((title.as_str(), text.as_str()), ("Übertragung fertig", "Film.mkv ist bei „laptop“ angekommen (3 GB)"));
        let (title, _) = end_notice(&progress(Direction::Receiving, Phase::Cancelled { by_peer: true })).unwrap();
        assert_eq!(title, "Übertragung abgebrochen");
        let copied = Progress { kind: Kind::Clipboard, ..progress(Direction::Receiving, Phase::Done) };
        assert_eq!(end_notice(&copied), None, "dafür meldet sich „Kopierte Dateien sind bereit“");
        let dropped = progress(Direction::Receiving, Phase::Done);
        assert_eq!(end_notice(&dropped).map(|n| n.1), Some("Film.mkv von „laptop“ ist da (3 GB)".into()));
    }

    #[test]
    fn ids_round_trip() {
        let v = view(&progress(Direction::Receiving, Phase::Running));
        assert_eq!(parse_id(&v.id), Some((Direction::Receiving, 17_234_567_890_123_456_789)));
        assert_eq!(parse_id("send-x"), None);
        assert_eq!(parse_id("other-1"), None);
    }
}
