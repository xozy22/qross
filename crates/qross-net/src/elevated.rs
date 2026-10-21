//! Fenster mit höheren Rechten am Server (Windows): regelmäßig nachsehen, ob eines vorne oder unter dem
//! Cursor liegt, etwa der Task-Manager (siehe `qross_platform::elevated`). Solange erreichen Eingaben an
//! dieses Fenster die Erfassung nicht, und Tasten landeten darin statt auf einem Client; der Kern
//! wechselt deshalb nicht zu Clients (siehe `qross_core::server::ServerEvent::LocalElevatedWindow`).
//! Weil die Erfassung den Cursor dann womöglich nicht sieht, fragt der Server seine Position selbst ab:
//! Steht er an einer Kante zu einem Client, wollte der Nutzer wechseln, und das wird gemeldet.

use std::time::{Duration, Instant};

use qross_platform::ElevatedWindow;
use tokio::sync::watch;
use tracing::warn;

/// So oft wird nachgesehen, und so oft fragt der Server den Cursor ab, solange ein Fenster im Weg ist
pub const POLL: Duration = Duration::from_millis(250);
/// So lange muss ein Fenster im Weg liegen, bevor es gemeldet wird: zwei Abfragen hintereinander, nicht
/// schon beim Überfahren mit dem Cursor
pub const BLOCKED_AFTER: Duration = Duration::from_millis(200);
/// So lange muss der Weg frei sein, bevor das gemeldet wird: Wandert der Cursor nur kurz von dem Fenster
/// herunter, bleibt der Hinweis stehen, statt zu flackern. Nicht länger: Bis dahin wechselt der Cursor
/// noch nicht, auch wenn der Nutzer schon woanders hingeklickt hat.
pub const FREE_AFTER: Duration = Duration::from_millis(500);

/// Liefert das Fenster mit höheren Rechten, das gerade im Weg ist
pub type WindowQuery = Box<dyn Fn() -> Option<ElevatedWindow> + Send + Sync>;
/// Liefert, wo der Cursor steht
pub type CursorQuery = Box<dyn Fn() -> Option<(i32, i32)> + Send + Sync>;

/// Wie der Server nachsieht (üblich: [`Detector::system`])
pub struct Detector {
    pub window: WindowQuery,
    pub cursor: CursorQuery,
}

impl Detector {
    /// Die Abfragen des Systems, `qross_platform::elevated_window` und `qross_platform::cursor_position`;
    /// Ergebnisse gibt es nur unter Windows.
    pub fn system() -> Self {
        Self { window: Box::new(qross_platform::elevated_window), cursor: Box::new(qross_platform::cursor_position) }
    }
}

/// Entprellt die Abfragen: Ein Fenster gilt als im Weg, wenn es [`BLOCKED_AFTER`] lang dort lag, der Weg
/// als frei, wenn er [`FREE_AFTER`] lang frei war. Welches Fenster es ist, zählt nach seinem Prozess: Ein
/// neuer Titel ist keine Änderung, ein anderes Programm schon.
#[derive(Debug, Default)]
pub struct Debounce {
    /// Zuletzt gemeldet
    reported: Option<ElevatedWindow>,
    /// Ein anderer Stand (Prozess des Fensters, `None`: frei) und seit wann er gilt
    pending: Option<(Option<u32>, Instant)>,
}

impl Debounce {
    /// Eine Abfrage zur Zeit `now`; liefert den neuen Stand, wenn er sich damit ändert.
    pub fn observe(&mut self, now: Instant, seen: Option<ElevatedWindow>) -> Option<Option<ElevatedWindow>> {
        let key = seen.as_ref().map(|window| window.pid);
        if key == self.reported.as_ref().map(|window| window.pid) {
            self.pending = None;
            return None;
        }
        let since = match self.pending {
            Some((pending, since)) if pending == key => since,
            _ => {
                self.pending = Some((key, now));
                now
            }
        };
        let wait = if seen.is_some() { BLOCKED_AFTER } else { FREE_AFTER };
        if now.saturating_duration_since(since) < wait {
            return None;
        }
        self.pending = None;
        self.reported.clone_from(&seen);
        Some(seen)
    }
}

/// Startet die Abfrage in einem eigenen Thread und meldet jeden entprellten Wechsel. Sie endet, sobald
/// alle Empfänger verworfen sind.
pub fn watch(query: WindowQuery) -> watch::Receiver<Option<ElevatedWindow>> {
    let (tx, rx) = watch::channel(None);
    let spawned = std::thread::Builder::new().name("qross-elevated".into()).spawn(move || {
        let mut debounce = Debounce::default();
        while !tx.is_closed() {
            if let Some(now) = debounce.observe(Instant::now(), query()) {
                tx.send_replace(now);
            }
            std::thread::sleep(POLL);
        }
    });
    if let Err(e) = spawned {
        warn!("windows with higher privileges: failed to start polling: {e}");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taskmgr() -> ElevatedWindow {
        ElevatedWindow { pid: 4711, exe: Some("Taskmgr.exe".into()), title: Some("Task-Manager".into()), admin: true }
    }

    fn regedit() -> ElevatedWindow {
        ElevatedWindow { pid: 815, exe: Some("regedit.exe".into()), title: Some("Registrierungs-Editor".into()), admin: true }
    }

    /// Abfragen im Takt von `POLL`: was die `n`-te sah und was danach gemeldet wurde
    fn run(polls: &[Option<ElevatedWindow>]) -> Vec<Option<Option<ElevatedWindow>>> {
        let t0 = Instant::now();
        let mut debounce = Debounce::default();
        polls.iter().enumerate().map(|(n, seen)| debounce.observe(t0 + POLL * n as u32, seen.clone())).collect()
    }

    #[test]
    fn a_window_counts_once_two_polls_saw_it() {
        let tm = Some(taskmgr());
        assert_eq!(run(&[tm.clone(), tm.clone(), tm.clone()]), [None, Some(tm), None]);
    }

    #[test]
    fn passing_over_a_window_is_no_blockade() {
        let tm = Some(taskmgr());
        let seen = run(&[tm.clone(), None, tm.clone(), None, None, None, None, None, None]);
        assert!(seen.iter().all(Option::is_none), "{seen:?}");
        assert_eq!(run(&[None, tm.clone(), None, tm.clone(), tm.clone()]), [None, None, None, None, Some(tm)]);
    }

    #[test]
    fn the_way_is_free_after_half_a_second() {
        let tm = Some(taskmgr());
        assert_eq!(run(&[tm.clone(), tm.clone(), None, None, None]), [None, Some(tm.clone()), None, None, Some(None)]);
        // Kommt es zwischendurch zurück, bleibt es beim Hinweis, und die halbe Sekunde beginnt neu.
        let polls = [tm.clone(), tm.clone(), None, None, tm.clone(), None, None, None];
        let reported: Vec<_> = run(&polls).into_iter().enumerate().filter_map(|(n, r)| r.map(|r| (n, r))).collect();
        assert_eq!(reported, [(1, tm), (7, None)]);
    }

    #[test]
    fn another_program_takes_over_but_a_new_title_does_not() {
        let (tm, reg) = (Some(taskmgr()), Some(regedit()));
        let renamed = Some(ElevatedWindow { title: Some("Task-Manager – Leistung".into()), ..taskmgr() });
        let seen = run(&[tm.clone(), tm.clone(), renamed.clone(), renamed, reg.clone(), reg.clone()]);
        assert_eq!(seen, [None, Some(tm), None, None, None, Some(reg)]);
    }
}
