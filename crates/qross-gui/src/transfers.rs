//! Fortschrittsanzeige laufender Dateiübertragungen (Drag & Drop in beide Richtungen, kopierte
//! Dateien): Texte für das Fenster, den Gesamtstand für Tray und Taskleiste und die
//! Systembenachrichtigung am Ende.

use std::collections::HashMap;
use std::time::Duration;

use qross_core::text::size_text_in;
use qross_net::progress::{Direction, Kind, Phase, Progress};
use serde::Serialize;

use crate::clipboard_files::names_text;
use crate::i18n::Lang;

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

/// „an „laptop““ bzw. „von „pc““
fn route(direction: Direction, peer: &str, lang: Lang) -> String {
    match (direction, lang) {
        (Direction::Sending, Lang::De) => format!("an „{peer}“"),
        (Direction::Sending, Lang::En) => format!("to “{peer}”"),
        (Direction::Receiving, Lang::De) => format!("von „{peer}“"),
        (Direction::Receiving, Lang::En) => format!("from “{peer}”"),
    }
}

/// Die Anzeige zu einer Meldung aus `qross_net`, Texte in `lang`
pub fn view(p: &Progress, lang: Lang) -> TransferView {
    let peer = &p.peer;
    let size = |bytes| size_text_in(bytes, lang);
    let prefix = match p.direction {
        Direction::Sending => "send",
        Direction::Receiving => "recv",
    };
    let percent = if p.total == 0 { 100.0 } else { (p.bytes as f64 * 100.0 / p.total as f64).min(100.0) };
    let state = match p.phase {
        Phase::Running => "running",
        Phase::Done => "done",
        Phase::Cancelled { .. } => "cancelled",
        Phase::Failed(_) => "failed",
    };
    let speed = match p.phase {
        Phase::Running => running_speed(p, lang),
        Phase::Done => format!("{} in {}", size(p.total), duration_text(p.elapsed, lang)),
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
        name: names_text(&p.names, lang),
        route: route(p.direction, peer, lang),
        percent: (percent * 10.0).round() / 10.0,
        size: match lang {
            Lang::De => format!("{} von {}", size(p.bytes), size(p.total)),
            Lang::En => format!("{} of {}", size(p.bytes), size(p.total)),
        },
        speed,
        state,
        outcome: outcome(p, lang),
        elapsed_ms: p.elapsed.as_millis() as u64,
    }
}

/// „85 MB/s · noch 12 s“; ohne Schätzung nur die Rate, ohne Durchsatz „wartet …“
fn running_speed(p: &Progress, lang: Lang) -> String {
    if p.bytes_per_s == 0 {
        if p.elapsed < Duration::from_secs(1) {
            return String::new();
        }
        return match lang {
            Lang::De => "wartet …".into(),
            Lang::En => "waiting …".into(),
        };
    }
    let rate = format!("{}/s", size_text_in(p.bytes_per_s, lang));
    match p.remaining {
        Some(left) => format!("{rate} · {}", remaining_text(left, lang)),
        None => rate,
    }
}

fn outcome(p: &Progress, lang: Lang) -> Option<String> {
    let peer = &p.peer;
    Some(match (&p.phase, lang) {
        (Phase::Running, _) => return None,
        (Phase::Done, Lang::De) => match p.direction {
            Direction::Sending => format!("Bei „{peer}“ angekommen"),
            Direction::Receiving => "Angekommen".into(),
        },
        (Phase::Done, Lang::En) => match p.direction {
            Direction::Sending => format!("Arrived at “{peer}”"),
            Direction::Receiving => "Arrived".into(),
        },
        (Phase::Cancelled { by_peer: false }, Lang::De) => "Abgebrochen".into(),
        (Phase::Cancelled { by_peer: false }, Lang::En) => "Canceled".into(),
        (Phase::Cancelled { by_peer: true }, Lang::De) => format!("„{peer}“ hat abgebrochen"),
        (Phase::Cancelled { by_peer: true }, Lang::En) => format!("“{peer}” canceled"),
        (Phase::Failed(reason), Lang::De) => format!("Fehlgeschlagen: {reason}"),
        (Phase::Failed(reason), Lang::En) => format!("Failed: {reason}"),
    })
}

/// „noch 42 s“, „noch 12 min“, „noch 2 h 5 min“ (aufgerundet, die Schätzung schwankt ohnehin);
/// englisch „42 s left“
pub fn remaining_text(left: Duration, lang: Lang) -> String {
    let secs = left.as_secs_f64().ceil().max(1.0) as u64;
    let time = if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs.div_ceil(60))
    } else {
        let minutes = secs.div_ceil(60);
        match minutes % 60 {
            0 => format!("{} h", minutes / 60),
            rest => format!("{} h {rest} min", minutes / 60),
        }
    };
    match lang {
        Lang::De => format!("noch {time}"),
        Lang::En => format!("{time} left"),
    }
}

/// „0,4 s“, „40 s“, „2 min 5 s“, „1 h 3 min“ (englisch „0.4 s“)
pub fn duration_text(d: Duration, lang: Lang) -> String {
    let secs = d.as_secs();
    if secs < 10 {
        let text = format!("{:.1} s", d.as_secs_f64());
        match lang {
            Lang::De => text.replace('.', ","),
            Lang::En => text,
        }
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
pub fn end_notice(p: &Progress, lang: Lang) -> Option<(String, String)> {
    if p.elapsed < LONG_TRANSFER {
        return None;
    }
    let (name, peer, size) = (names_text(&p.names, lang), &p.peer, size_text_in(p.total, lang));
    match (&p.phase, p.direction) {
        (Phase::Running | Phase::Cancelled { by_peer: false }, _) => None,
        (Phase::Done, Direction::Receiving) if p.kind == Kind::Clipboard => None,
        (Phase::Done, Direction::Sending) => Some(match lang {
            Lang::De => ("Übertragung fertig".into(), format!("{name} ist bei „{peer}“ angekommen ({size})")),
            Lang::En => ("Transfer complete".into(), format!("{name} arrived at “{peer}” ({size})")),
        }),
        (Phase::Done, Direction::Receiving) => Some(match lang {
            Lang::De => ("Dateien angekommen".into(), format!("{name} von „{peer}“ ist da ({size})")),
            Lang::En => ("Files received".into(), format!("{name} from “{peer}” has arrived ({size})")),
        }),
        (Phase::Cancelled { by_peer: true }, _) => Some(match lang {
            Lang::De => ("Übertragung abgebrochen".into(), format!("„{peer}“ hat die Übertragung von {name} abgebrochen")),
            Lang::En => ("Transfer canceled".into(), format!("“{peer}” canceled the transfer of {name}")),
        }),
        (Phase::Failed(reason), _) => Some(match lang {
            Lang::De => ("Übertragung fehlgeschlagen".into(), format!("{name}: {reason}")),
            Lang::En => ("Transfer failed".into(), format!("{name}: {reason}")),
        }),
    }
}

/// Erst ab dieser Laufzeit zeigen Tray und Taskleiste eine Übertragung; kurze blitzten sonst nur auf.
pub const SHOW_OUTSIDE_AFTER: Duration = Duration::from_secs(1);
/// Längster Dateiname im Tooltip (Windows nimmt dort höchstens 127 Zeichen)
const TOOLTIP_NAME_CHARS: usize = 32;

/// Gesamtstand aller laufenden Übertragungen, für Tray und Taskleiste
#[derive(Debug, Clone, PartialEq)]
pub struct Overall {
    /// 0 bis 100, nach Bytes über alle Übertragungen
    pub percent: u8,
    /// Zeile für den Tooltip: „Film.mkv an „laptop“: 42 % · noch 21 s“, bei mehreren
    /// „3 Übertragungen: 42 %“
    pub text: String,
}

/// Laufende Übertragungen mit ihrem letzten Stand
#[derive(Default)]
pub struct Ongoing(HashMap<(Direction, u64), Progress>);

impl Ongoing {
    /// Übernimmt einen Stand. Zu Ende (fertig, abgebrochen, fehlgeschlagen) fällt die Übertragung heraus.
    pub fn update(&mut self, p: &Progress) {
        let key = (p.direction, p.transfer);
        if p.phase.is_running() {
            self.0.insert(key, p.clone());
        } else {
            self.0.remove(&key);
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// `None`, solange keine Übertragung lange genug läuft. Text in `lang`.
    pub fn overall(&self, lang: Lang) -> Option<Overall> {
        let shown: Vec<&Progress> = self.0.values().filter(|p| p.elapsed >= SHOW_OUTSIDE_AFTER).collect();
        let (bytes, total) = shown.iter().fold((0u64, 0u64), |(b, t), p| (b + p.bytes.min(p.total), t + p.total));
        // Nur leere Ordner: gleich fertig
        let percent = (bytes * 100).checked_div(total).map_or(100, |p| p as u8);
        let text = match shown.as_slice() {
            [] => return None,
            [p] => {
                let route = route(p.direction, &p.peer, lang);
                let left = p.remaining.map(|left| format!(" · {}", remaining_text(left, lang))).unwrap_or_default();
                let name = shorten(&names_text(&p.names, lang), TOOLTIP_NAME_CHARS);
                match lang {
                    Lang::De => format!("{name} {route}: {percent} %{left}"),
                    Lang::En => format!("{name} {route}: {percent}%{left}"),
                }
            }
            many => {
                // Die Restzeit der langsamsten, sofern alle eine Schätzung haben
                let left = many.iter().map(|p| p.remaining).collect::<Option<Vec<_>>>().and_then(|all| all.into_iter().max());
                let left = left.map(|left| format!(" · {}", remaining_text(left, lang))).unwrap_or_default();
                match lang {
                    Lang::De => format!("{} Übertragungen: {percent} %{left}", many.len()),
                    Lang::En => format!("{} transfers: {percent}%{left}", many.len()),
                }
            }
        };
        Some(Overall { percent, text })
    }
}

/// Kürzt auf höchstens `max` Zeichen, mit „…“ am Ende
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    short.push('…');
    short
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
        let v = view(&progress(Direction::Sending, Phase::Running), Lang::De);
        assert_eq!(v.id, "send-17234567890123456789");
        assert_eq!((v.direction, v.kind, v.state), ("sending", "drag", "running"));
        assert_eq!((v.name.as_str(), v.route.as_str()), ("Film.mkv", "an „laptop“"));
        assert_eq!(v.percent, 42.0);
        assert_eq!(v.size, "1,3 GB von 3 GB");
        assert_eq!(v.speed, "85 MB/s · noch 21 s");
        assert_eq!(v.outcome, None);

        let early = Progress { remaining: None, ..progress(Direction::Receiving, Phase::Running) };
        assert_eq!(view(&early, Lang::De).speed, "85 MB/s", "ohne Schätzung nur die Rate");
        assert_eq!(view(&early, Lang::De).route, "von „laptop“");
        let stalled = Progress { bytes_per_s: 0, remaining: None, ..early };
        assert_eq!(view(&stalled, Lang::De).speed, "wartet …");
    }

    #[test]
    fn running_transfers_in_english() {
        let v = view(&progress(Direction::Sending, Phase::Running), Lang::En);
        assert_eq!((v.name.as_str(), v.route.as_str()), ("Film.mkv", "to “laptop”"));
        assert_eq!(v.size, "1.3 GB of 3 GB");
        assert_eq!(v.speed, "85 MB/s · 21 s left");
        let early = Progress { remaining: None, ..progress(Direction::Receiving, Phase::Running) };
        assert_eq!(view(&early, Lang::En).route, "from “laptop”");
        let stalled = Progress { bytes_per_s: 0, remaining: None, ..early };
        assert_eq!(view(&stalled, Lang::En).speed, "waiting …");
        let many = Progress { names: vec!["a".into(), "b".into()], ..stalled };
        assert_eq!(view(&many, Lang::En).name, "a and b");
    }

    #[test]
    fn the_end_is_described() {
        let done = view(&progress(Direction::Sending, Phase::Done), Lang::De);
        assert_eq!((done.state, done.outcome.as_deref()), ("done", Some("Bei „laptop“ angekommen")));
        assert_eq!(done.speed, "3 GB in 15 s");
        let cancelled = view(&progress(Direction::Receiving, Phase::Cancelled { by_peer: true }), Lang::De);
        assert_eq!((cancelled.state, cancelled.outcome.as_deref()), ("cancelled", Some("„laptop“ hat abgebrochen")));
        assert_eq!(cancelled.speed, "");
        let failed = view(&progress(Direction::Receiving, Phase::Failed("Verbindung getrennt".into())), Lang::De);
        assert_eq!(failed.outcome.as_deref(), Some("Fehlgeschlagen: Verbindung getrennt"));
    }

    #[test]
    fn the_end_is_described_in_english() {
        let outcome = |direction, phase| view(&progress(direction, phase), Lang::En).outcome;
        assert_eq!(outcome(Direction::Sending, Phase::Done).as_deref(), Some("Arrived at “laptop”"));
        assert_eq!(outcome(Direction::Receiving, Phase::Done).as_deref(), Some("Arrived"));
        assert_eq!(outcome(Direction::Receiving, Phase::Cancelled { by_peer: false }).as_deref(), Some("Canceled"));
        assert_eq!(outcome(Direction::Receiving, Phase::Cancelled { by_peer: true }).as_deref(), Some("“laptop” canceled"));
        let failed = outcome(Direction::Receiving, Phase::Failed("connection lost".into()));
        assert_eq!(failed.as_deref(), Some("Failed: connection lost"));
        let done = Progress { elapsed: Duration::from_millis(2400), ..progress(Direction::Sending, Phase::Done) };
        assert_eq!(view(&done, Lang::En).speed, "3 GB in 2.4 s");
    }

    #[test]
    fn empty_folders_count_as_complete() {
        let folders = Progress { total: 0, bytes: 0, ..progress(Direction::Receiving, Phase::Running) };
        assert_eq!(view(&folders, Lang::De).percent, 100.0);
    }

    #[test]
    fn times_are_short_and_german() {
        let remaining_text = |left| remaining_text(left, Lang::De);
        let duration_text = |d| duration_text(d, Lang::De);
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
    fn times_in_english() {
        assert_eq!(remaining_text(Duration::from_millis(300), Lang::En), "1 s left");
        assert_eq!(remaining_text(Duration::from_secs(61), Lang::En), "2 min left");
        assert_eq!(remaining_text(Duration::from_secs(3600), Lang::En), "1 h left");
        assert_eq!(remaining_text(Duration::from_secs(7500), Lang::En), "2 h 5 min left");
        assert_eq!(duration_text(Duration::from_millis(2400), Lang::En), "2.4 s");
        assert_eq!(duration_text(Duration::from_secs(125), Lang::En), "2 min 5 s");
    }

    #[test]
    fn system_notices_only_for_long_transfers_not_cancelled_here() {
        let end_notice = |p: &Progress| end_notice(p, Lang::De);
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
    fn system_notices_in_english() {
        let end_notice = |direction, phase| end_notice(&progress(direction, phase), Lang::En);
        assert_eq!(
            end_notice(Direction::Sending, Phase::Done),
            Some(("Transfer complete".into(), "Film.mkv arrived at “laptop” (3 GB)".into()))
        );
        assert_eq!(
            end_notice(Direction::Receiving, Phase::Done),
            Some(("Files received".into(), "Film.mkv from “laptop” has arrived (3 GB)".into()))
        );
        assert_eq!(
            end_notice(Direction::Receiving, Phase::Cancelled { by_peer: true }),
            Some(("Transfer canceled".into(), "“laptop” canceled the transfer of Film.mkv".into()))
        );
        assert_eq!(
            end_notice(Direction::Sending, Phase::Failed("connection lost".into())),
            Some(("Transfer failed".into(), "Film.mkv: connection lost".into()))
        );
    }

    #[test]
    fn tray_and_taskbar_show_all_running_transfers_together() {
        let mut ongoing = Ongoing::default();
        assert_eq!(ongoing.overall(Lang::De), None);

        // Eben erst gestartet: noch nichts, kurze Übertragungen blitzten sonst nur auf
        let fresh = Progress { elapsed: Duration::from_millis(400), ..progress(Direction::Sending, Phase::Running) };
        ongoing.update(&fresh);
        assert_eq!(ongoing.overall(Lang::De), None);

        ongoing.update(&progress(Direction::Sending, Phase::Running));
        let one = ongoing.overall(Lang::De).unwrap();
        assert_eq!(one.percent, 42);
        assert_eq!(one.text, "Film.mkv an „laptop“: 42 % · noch 21 s");
        assert_eq!(ongoing.overall(Lang::En).unwrap().text, "Film.mkv to “laptop”: 42% · 21 s left");

        // Eine zweite (Empfang, gleiche Nummer ist eine andere Übertragung): zusammen nach Bytes
        let incoming = Progress {
            total: 1_000_000_000,
            bytes: 940_000_000,
            remaining: Some(Duration::from_secs(2)),
            ..progress(Direction::Receiving, Phase::Running)
        };
        ongoing.update(&incoming);
        let both = ongoing.overall(Lang::De).unwrap();
        assert_eq!(both.percent, 55, "(1,26 + 0,94) / (3 + 1) GB");
        assert_eq!(both.text, "2 Übertragungen: 55 % · noch 21 s");
        assert_eq!(ongoing.overall(Lang::En).unwrap().text, "2 transfers: 55% · 21 s left");

        // Ohne Schätzung bei einer davon keine Restzeit
        ongoing.update(&Progress { remaining: None, ..incoming.clone() });
        assert_eq!(ongoing.overall(Lang::De).unwrap().text, "2 Übertragungen: 55 %");
        assert_eq!(ongoing.overall(Lang::En).unwrap().text, "2 transfers: 55%");

        // Zu Ende: fällt heraus, egal wie sie endete
        ongoing.update(&Progress { phase: Phase::Done, ..incoming });
        assert_eq!(ongoing.overall(Lang::De).unwrap().percent, 42);
        ongoing.update(&progress(Direction::Sending, Phase::Failed("weg".into())));
        assert_eq!(ongoing.overall(Lang::De), None);
    }

    #[test]
    fn long_names_are_shortened_in_the_tooltip() {
        let mut ongoing = Ongoing::default();
        let names = vec!["Ein sehr langer Dateiname für den Bericht 2026.pdf".into()];
        ongoing.update(&Progress { names, ..progress(Direction::Receiving, Phase::Running) });
        let text = ongoing.overall(Lang::De).unwrap().text;
        assert_eq!(text, "Ein sehr langer Dateiname für d… von „laptop“: 42 % · noch 21 s");
        assert!(text.chars().count() < 100);
        let text = ongoing.overall(Lang::En).unwrap().text;
        assert_eq!(text, "Ein sehr langer Dateiname für d… from “laptop”: 42% · 21 s left");
    }

    #[test]
    fn ids_round_trip() {
        let v = view(&progress(Direction::Receiving, Phase::Running), Lang::De);
        assert_eq!(parse_id(&v.id), Some((Direction::Receiving, 17_234_567_890_123_456_789)));
        assert_eq!(parse_id("send-x"), None);
        assert_eq!(parse_id("other-1"), None);
    }
}
