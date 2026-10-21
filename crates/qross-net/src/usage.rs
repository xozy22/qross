//! CPU und Arbeitsspeicher von Qross je Rechner: Ein Client meldet seine Werte, solange der Server sie
//! anzeigt (`Control::WatchUsage`), die Oberfläche fragt die eigenen selbst ab ([`measure`]). Eine Messung
//! sieht die Prozesse des Systems durch und läuft deshalb außerhalb der Ereignisschleifen.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qross_platform::usage::{Usage, UsageMeter};
use qross_proto::{Control, UsageKind, UsagePart, UsageReport};
use tokio::sync::{mpsc, watch};

/// So oft meldet sich ein Client, solange der Server hinsieht
pub const REPORT_EVERY: Duration = Duration::from_secs(2);
/// Liegt die vorige Messung länger zurück, gälte die Rechenzeit für diese ganze Spanne (siehe [`measure`])
pub const STALE_AFTER: Duration = Duration::from_secs(5);
/// Abstand der kurzen Grundmessung vor einem frischen Wert
pub const SETTLE: Duration = Duration::from_millis(500);

/// Ein Messgerät, das sich mehrere Abfragen teilen
pub type SharedMeter = Arc<Mutex<UsageMeter>>;

/// Misst, ohne die Ereignisschleife aufzuhalten. Liegt die vorige Messung länger als [`STALE_AFTER`]
/// zurück (oder gab es keine), erst eine Grundmessung und [`SETTLE`] später die eigentliche: Die
/// Rechenzeit gilt dann für jetzt, nicht etwa für die Minute, in der niemand hinsah.
pub async fn measure(meter: &SharedMeter) -> Usage {
    let now = now();
    let fresh = meter.lock().unwrap().last_sample().is_some_and(|at| now.saturating_duration_since(at) <= STALE_AFTER);
    if !fresh {
        sample(meter).await;
        tokio::time::sleep(SETTLE).await;
    }
    sample(meter).await
}

async fn sample(meter: &SharedMeter) -> Usage {
    let (meter, now) = (meter.clone(), now());
    tokio::task::spawn_blocking(move || meter.lock().unwrap().sample_at(now)).await.unwrap_or_default()
}

/// Uhr von tokio: dieselbe wie die des Systems, außer in Tests mit angehaltener Zeit
fn now() -> std::time::Instant {
    tokio::time::Instant::now().into_std()
}

/// Meldung an den Server: mit `detail` (Server ab `USAGE_DETAIL_SINCE`) die Summe samt Teilen je Art
/// (`Control::UsageDetail`), sonst nur die Summe (`Control::Usage`). Ohne Rechenzeit (erste Messung) keine.
pub fn report(usage: &Usage, detail: bool) -> Option<Control> {
    let total = UsageReport {
        cpu_permille: permille(usage.cpu_percent?),
        memory_kib: usage.memory / 1024,
        processes: u16::try_from(usage.processes).unwrap_or(u16::MAX),
    };
    if !detail {
        return Some(Control::Usage(total));
    }
    let parts = usage
        .parts
        .iter()
        .map(|part| UsagePart {
            kind: part.kind,
            cpu_permille: part.cpu_percent.map_or(0, permille),
            memory_kib: part.memory / 1024,
            processes: u16::try_from(part.processes).unwrap_or(u16::MAX),
        })
        .collect();
    Some(Control::UsageDetail { total, parts })
}

fn permille(percent: f64) -> u32 {
    (percent * 10.0).round() as u32
}

/// Server: die Teile eines Clients, wie die Oberfläche sie zeigt: je Art einer (was eine neuere Version mehr
/// unterscheidet, kommt hier als `Other` an und wird zusammengezählt), in der Reihenfolge von [`UsageKind`]
pub fn by_kind(parts: Vec<UsagePart>) -> Vec<UsagePart> {
    let mut kinds: BTreeMap<UsageKind, UsagePart> = BTreeMap::new();
    for part in parts {
        kinds
            .entry(part.kind)
            .and_modify(|sum| {
                sum.cpu_permille = sum.cpu_permille.saturating_add(part.cpu_permille);
                sum.memory_kib = sum.memory_kib.saturating_add(part.memory_kib);
                sum.processes = sum.processes.saturating_add(part.processes);
            })
            .or_insert(part);
    }
    kinds.into_values().collect()
}

/// Client: Solange `watched` an ist, gehen die eigenen Werte alle [`REPORT_EVERY`] über `reports` hinaus,
/// die erste gleich nach Beginn; mit `detail` samt Teilen je Art (siehe [`report`]). Endet mit der Sitzung
/// (einer der Kanäle zu).
pub(crate) async fn report_while_watched(
    mut watched: watch::Receiver<bool>,
    reports: mpsc::UnboundedSender<Control>,
    detail: bool,
) {
    let meter = SharedMeter::default();
    loop {
        while !*watched.borrow_and_update() {
            if watched.changed().await.is_err() {
                return;
            }
        }
        let mut next = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(next) => {
                    if let Some(report) = report(&measure(&meter).await, detail) {
                        if reports.send(report).is_err() {
                            return;
                        }
                    }
                    next = tokio::time::Instant::now() + REPORT_EVERY;
                }
                changed = watched.changed() => match changed {
                    Ok(()) if *watched.borrow_and_update() => {}
                    Ok(()) => break,
                    Err(_) => return,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use qross_platform::usage::Part;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn reports_carry_permille_and_kib() {
        let usage = Usage { cpu_percent: Some(12.34), memory: 128 * MB + 300, processes: 6, parts: Vec::new() };
        let total = UsageReport { cpu_permille: 123, memory_kib: 131_072, processes: 6 };
        assert_eq!(report(&usage, false), Some(Control::Usage(total)));
        assert_eq!(report(&Usage { cpu_percent: None, ..usage.clone() }, false), None, "no report without CPU time");
        assert_eq!(report(&Usage { cpu_percent: None, ..usage }, true), None);
        let busy = Usage { cpu_percent: Some(812.0), memory: 0, processes: 70_000, parts: Vec::new() };
        let total = UsageReport { cpu_permille: 8120, memory_kib: 0, processes: u16::MAX };
        assert_eq!(report(&busy, false), Some(Control::Usage(total)));
    }

    #[test]
    fn detailed_reports_carry_the_parts() {
        let parts = vec![
            Part { kind: UsageKind::Core, cpu_percent: Some(0.26), memory: 8 * MB + 512, processes: 1 },
            Part { kind: UsageKind::Page, cpu_percent: Some(1.04), memory: 24 * MB, processes: 1 },
            Part { kind: UsageKind::Other, cpu_percent: Some(0.0), memory: 5 * MB, processes: 2 },
        ];
        let usage = Usage { cpu_percent: Some(1.3), memory: 37 * MB + 512, processes: 4, parts };
        let total = UsageReport { cpu_permille: 13, memory_kib: 37 * 1024, processes: 4 };
        let parts = vec![
            UsagePart { kind: UsageKind::Core, cpu_permille: 3, memory_kib: 8 * 1024, processes: 1 },
            UsagePart { kind: UsageKind::Page, cpu_permille: 10, memory_kib: 24 * 1024, processes: 1 },
            UsagePart { kind: UsageKind::Other, cpu_permille: 0, memory_kib: 5 * 1024, processes: 2 },
        ];
        assert_eq!(report(&usage, true), Some(Control::UsageDetail { total, parts }));
        assert_eq!(report(&usage, false), Some(Control::Usage(total)), "older servers get only the total");
    }

    #[test]
    fn parts_are_shown_once_per_kind_in_order() {
        let part = |kind, cpu_permille, memory_kib, processes| UsagePart { kind, cpu_permille, memory_kib, processes };
        // Wie von einer neueren Version: Zwei Arten, die es hier nicht gibt, kommen als `Other` an.
        let parts = vec![
            part(UsageKind::Page, 21, 24_576, 1),
            part(UsageKind::Other, 1, 3_072, 1),
            part(UsageKind::Core, 12, 8_192, 1),
            part(UsageKind::Other, 2, 2_048, 1),
            part(UsageKind::Other, 0, 1_024, 2),
        ];
        let expected =
            vec![part(UsageKind::Core, 12, 8_192, 1), part(UsageKind::Page, 21, 24_576, 1), part(UsageKind::Other, 3, 6_144, 4)];
        assert_eq!(by_kind(parts), expected);
        assert!(by_kind(Vec::new()).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn reports_flow_only_while_watched() {
        let (watch_tx, watched) = watch::channel(false);
        let (tx, mut reports) = mpsc::unbounded_channel();
        let task = tokio::spawn(report_while_watched(watched, tx, true));

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(reports.try_recv().is_err(), "nobody is watching");

        watch_tx.send_replace(true);
        // Die erste kommt nach der Grundmessung, dann alle 2 s
        let first = tokio::time::timeout(SETTLE + Duration::from_secs(1), reports.recv()).await;
        assert!(matches!(first, Ok(Some(_))), "{first:?}");
        let second = tokio::time::timeout(REPORT_EVERY + Duration::from_secs(1), reports.recv()).await;
        assert!(matches!(second, Ok(Some(_))), "{second:?}");

        watch_tx.send_replace(false);
        tokio::time::sleep(Duration::from_secs(1)).await;
        while reports.try_recv().is_ok() {}
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(reports.try_recv().is_err(), "no longer watched");

        drop(watch_tx);
        tokio::time::timeout(Duration::from_secs(1), task).await.expect("ends with the session").unwrap();
    }
}
