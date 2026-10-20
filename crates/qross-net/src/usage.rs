//! CPU und Arbeitsspeicher von Qross je Rechner: Ein Client meldet seine Werte, solange der Server sie
//! anzeigt (`Control::WatchUsage`), die Oberfläche fragt die eigenen selbst ab ([`measure`]). Eine Messung
//! sieht die Prozesse des Systems durch und läuft deshalb außerhalb der Ereignisschleifen.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use qross_platform::usage::{Usage, UsageMeter};
use qross_proto::UsageReport;
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

/// Meldung an den Server; ohne Rechenzeit (erste Messung) keine
pub fn report(usage: &Usage) -> Option<UsageReport> {
    let cpu = usage.cpu_percent?;
    Some(UsageReport {
        cpu_permille: (cpu * 10.0).round() as u32,
        memory_kib: usage.memory / 1024,
        processes: u16::try_from(usage.processes).unwrap_or(u16::MAX),
    })
}

/// Client: Solange `watched` an ist, gehen die eigenen Werte alle [`REPORT_EVERY`] über `reports` hinaus,
/// die erste gleich nach Beginn. Endet mit der Sitzung (einer der Kanäle zu).
pub(crate) async fn report_while_watched(mut watched: watch::Receiver<bool>, reports: mpsc::UnboundedSender<UsageReport>) {
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
                    if let Some(report) = report(&measure(&meter).await) {
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

    #[test]
    fn reports_carry_permille_and_kib() {
        let usage = Usage { cpu_percent: Some(12.34), memory: 128 * 1024 * 1024 + 300, processes: 6 };
        assert_eq!(report(&usage), Some(UsageReport { cpu_permille: 123, memory_kib: 131_072, processes: 6 }));
        assert_eq!(report(&Usage { cpu_percent: None, ..usage }), None, "no report without CPU time");
        let busy = Usage { cpu_percent: Some(812.0), memory: 0, processes: 70_000 };
        assert_eq!(report(&busy), Some(UsageReport { cpu_permille: 8120, memory_kib: 0, processes: u16::MAX }));
    }

    #[tokio::test(start_paused = true)]
    async fn reports_flow_only_while_watched() {
        let (watch_tx, watched) = watch::channel(false);
        let (tx, mut reports) = mpsc::unbounded_channel();
        let task = tokio::spawn(report_while_watched(watched, tx));

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
