//! Verbindungsqualität je QUIC-Verbindung: Latenz, Jitter, Paketverlust und Durchsatz.
//!
//! Gemessen wird ohne zusätzlichen Netzverkehr und ohne Protokollanteil: quinn führt die nötigen
//! Werte ohnehin, und weil beide Seiten jede Sekunde ein Keep-alive senden, das bestätigt werden
//! muss, gibt es auch ohne Eingaben laufend neue Messungen.
//!
//! - **Latenz**: die geglättete Round-Trip-Time von quinn (RFC 9002: neu = 7/8 · alt + 1/8 · Probe).
//! - **Jitter**: mittlerer Betrag der Differenz aufeinanderfolgender RTT-Proben der letzten 10 s
//!   (wie der Interarrival-Jitter aus RFC 3550, nur ungewichtet gemittelt). quinn gibt nur den
//!   geglätteten Wert heraus. Kam seit der letzten Abfrage genau ein ACK-Frame an, lässt sich die
//!   einzelne Probe daraus zurückrechnen: Probe = 8 · neu − 7 · alt. Bei mehreren ACKs (viel
//!   Verkehr) folgt der geglättete Wert den Proben dicht und dient selbst als Probe; der Jitter fällt
//!   dann etwas zu niedrig aus.
//! - **Verlust**: von quinn als verloren erkannte Pakete in Prozent der gesendeten, über 10 s.
//!   Bei wenig Verkehr wird das Fenster bis auf 60 s verlängert, bis mindestens 100 Pakete
//!   beisammen sind – sonst machte ein einziges verlorenes Keep-alive gleich 5 % aus. Reicht auch
//!   das nicht (kurz nach dem Verbinden), wird trotzdem durch 100 geteilt. Erkennbar ist nur der
//!   Verlust in Senderichtung dieses Rechners; die Gegenrichtung misst die Gegenstelle.
//! - **Durchsatz**: gesendete und empfangene UDP-Bytes pro Sekunde, einschließlich QUIC-Overhead.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use quinn::Connection;
use serde::Serialize;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

/// So oft werden die Zähler von quinn gelesen (eine Mutex-Sperre und eine Kopie).
const POLL: Duration = Duration::from_millis(100);
/// Ein Messpunkt nach so vielen Abfragen, also etwa jede Sekunde.
const POLLS_PER_REPORT: u32 = 10;
/// Fenster für Jitter und Verlust.
pub const WINDOW: Duration = Duration::from_secs(10);
/// Längstes Fenster für den Verlust bei wenig Verkehr.
pub const MAX_LOSS_WINDOW: Duration = Duration::from_secs(60);
/// So viele gesendete Pakete braucht eine belastbare Verlustquote.
pub const MIN_LOSS_PACKETS: u64 = 100;

/// Obergrenzen (jeweils ausschließlich) für „Sehr gut“, „Gut“ und „Mäßig“; darüber „Schlecht“.
pub const RTT_LIMITS_MS: [f64; 3] = [10.0, 30.0, 80.0];
pub const JITTER_LIMITS_MS: [f64; 3] = [3.0, 10.0, 25.0];
pub const LOSS_LIMITS_PCT: [f64; 3] = [0.5, 2.0, 5.0];

/// Qualitätsstufe; die Reihenfolge geht von gut nach schlecht (`max` ist die schlechtere).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Wie über Kabel: Maus und Tastatur ohne spürbare Verzögerung
    VeryGood,
    /// Typisches WLAN: kaum zu merken
    Good,
    /// Verzögerung oder Ruckler sind gelegentlich spürbar
    Fair,
    /// Deutlich spürbar
    Poor,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::VeryGood => "Sehr gut",
            Level::Good => "Gut",
            Level::Fair => "Mäßig",
            Level::Poor => "Schlecht",
        }
    }

    fn of(value: f64, limits: [f64; 3]) -> Self {
        match limits.iter().position(|&limit| value < limit) {
            Some(0) => Level::VeryGood,
            Some(1) => Level::Good,
            Some(2) => Level::Fair,
            _ => Level::Poor,
        }
    }
}

/// Stufen der einzelnen Werte; die Gesamtstufe ist die schlechteste davon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rating {
    pub rtt: Level,
    pub jitter: Level,
    pub loss: Level,
}

impl Rating {
    /// Aus Latenz (ms), Jitter (ms) und Verlust (%)
    pub fn of(rtt_ms: f64, jitter_ms: f64, loss_pct: f64) -> Self {
        Self {
            rtt: Level::of(rtt_ms, RTT_LIMITS_MS),
            jitter: Level::of(jitter_ms, JITTER_LIMITS_MS),
            loss: Level::of(loss_pct, LOSS_LIMITS_PCT),
        }
    }

    pub fn overall(&self) -> Level {
        self.rtt.max(self.jitter).max(self.loss)
    }
}

/// Gesamtstufe aus Latenz (ms), Jitter (ms) und Verlust (%): der schlechteste der drei Werte.
pub fn rate(rtt_ms: f64, jitter_ms: f64, loss_pct: f64) -> Level {
    Rating::of(rtt_ms, jitter_ms, loss_pct).overall()
}

/// Ein Messpunkt, etwa jede Sekunde.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Quality {
    /// Geglättete Round-Trip-Time in Millisekunden
    pub rtt_ms: f64,
    /// Mittlere Schwankung aufeinanderfolgender RTT-Proben der letzten 10 s in Millisekunden
    pub jitter_ms: f64,
    /// Verlorene Pakete in Prozent der gesendeten (Senderichtung dieses Rechners); bei weniger
    /// als `MIN_LOSS_PACKETS` Paketen bezogen auf diese Mindestzahl
    pub loss_pct: f64,
    /// Über so viele Sekunden wurde der Verlust ermittelt (10 bis 60, am Anfang weniger)
    pub loss_window_s: u32,
    /// Gesendete bzw. empfangene Bytes pro Sekunde seit dem letzten Messpunkt (UDP)
    pub tx_bytes_per_s: u64,
    pub rx_bytes_per_s: u64,
    /// Gesamtstufe, gleich `rating.overall()`
    pub level: Level,
    pub rating: Rating,
}

/// Misst eine Verbindung im Hintergrund und meldet etwa jede Sekunde einen Messpunkt.
/// Endet mit der Verbindung oder sobald der zurückgegebene Griff fallen gelassen wird.
pub fn monitor(conn: Connection, mut report: impl FnMut(Quality) + Send + 'static) -> Monitor {
    Monitor(tokio::spawn(async move {
        let mut meter = Meter::default();
        let mut poll = tokio::time::interval(POLL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut polls = 0u32;
        loop {
            tokio::select! {
                _ = conn.closed() => break,
                _ = poll.tick() => {
                    meter.observe(Reading::of(&conn, Instant::now()));
                    polls += 1;
                    if polls.is_multiple_of(POLLS_PER_REPORT) {
                        if let Some(quality) = meter.report() {
                            report(quality);
                        }
                    }
                }
            }
        }
    }))
}

/// Beendet die Messung, wenn er fallen gelassen wird (er hält einen Griff auf die Verbindung).
pub struct Monitor(JoinHandle<()>);

impl Drop for Monitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Zählerstände einer Verbindung zu einem Zeitpunkt.
#[derive(Debug, Clone, Copy)]
struct Reading {
    at: Instant,
    /// Geglättete RTT
    rtt: Duration,
    /// Empfangene ACK-Frames (fortlaufend)
    acks: u64,
    sent_packets: u64,
    lost_packets: u64,
    tx_bytes: u64,
    rx_bytes: u64,
}

impl Reading {
    fn of(conn: &Connection, at: Instant) -> Self {
        let stats = conn.stats();
        Self {
            at,
            rtt: stats.path.rtt,
            acks: stats.frame_rx.acks,
            sent_packets: stats.path.sent_packets,
            lost_packets: stats.path.lost_packets,
            tx_bytes: stats.udp_tx.bytes,
            rx_bytes: stats.udp_rx.bytes,
        }
    }
}

/// Rechnet aus Abfragen Messpunkte; ohne Netz und Uhr, damit es sich prüfen lässt.
#[derive(Default)]
struct Meter {
    /// Letzte Abfrage
    last: Option<Reading>,
    /// RTT-Proben der letzten `WINDOW` in Millisekunden
    samples: VecDeque<(Instant, f64)>,
    /// Stände der bisherigen Messpunkte (für Verlust und Durchsatz), höchstens `MAX_LOSS_WINDOW` alt
    reports: VecDeque<Reading>,
}

impl Meter {
    /// Jede Abfrage: neue RTT-Probe erkennen.
    fn observe(&mut self, now: Reading) {
        let sample = match self.last {
            None => Some(now.rtt),
            Some(prev) if prev.rtt != now.rtt => Some(match now.acks.saturating_sub(prev.acks) {
                1 => single_sample(prev.rtt, now.rtt),
                _ => now.rtt,
            }),
            Some(_) => None,
        };
        if let Some(rtt) = sample {
            self.samples.push_back((now.at, rtt.as_secs_f64() * 1000.0));
        }
        while self.samples.front().is_some_and(|(at, _)| now.at.saturating_duration_since(*at) > WINDOW) {
            self.samples.pop_front();
        }
        self.last = Some(now);
    }

    /// Messpunkt aus dem Stand der letzten Abfrage (`None` vor der ersten).
    fn report(&mut self) -> Option<Quality> {
        let now = self.last?;
        let rtt_ms = now.rtt.as_secs_f64() * 1000.0;

        let jitter_ms = if self.samples.len() < 2 {
            0.0
        } else {
            let sum: f64 = self.samples.iter().zip(self.samples.iter().skip(1)).map(|((_, a), (_, b))| (b - a).abs()).sum();
            sum / (self.samples.len() - 1) as f64
        };

        // Verlust: zurück bis mindestens `WINDOW` und `MIN_LOSS_PACKETS`, höchstens `MAX_LOSS_WINDOW`.
        let mut base = None;
        for earlier in self.reports.iter().rev() {
            let age = now.at.saturating_duration_since(earlier.at);
            if age > MAX_LOSS_WINDOW {
                break;
            }
            base = Some(*earlier);
            let packets = now.sent_packets.saturating_sub(earlier.sent_packets);
            if age + POLL / 2 >= WINDOW && packets >= MIN_LOSS_PACKETS {
                break;
            }
        }
        let (loss_pct, loss_window_s) = match base {
            Some(base) => {
                let sent = now.sent_packets.saturating_sub(base.sent_packets);
                let lost = now.lost_packets.saturating_sub(base.lost_packets);
                // Wenige Pakete (kurz nach dem Verbinden): ein einzelner Verlust soll nicht gleich
                // wie eine hohe Quote aussehen, deshalb mindestens durch `MIN_LOSS_PACKETS` teilen.
                let pct = (lost as f64 * 100.0 / sent.max(MIN_LOSS_PACKETS) as f64).min(100.0);
                (pct, now.at.saturating_duration_since(base.at).as_secs_f64().round() as u32)
            }
            None => (0.0, 0),
        };

        let (tx_bytes_per_s, rx_bytes_per_s) = match self.reports.back() {
            Some(prev) => {
                let secs = now.at.saturating_duration_since(prev.at).as_secs_f64();
                if secs > 0.0 {
                    let per_s = |bytes: u64| (bytes as f64 / secs).round() as u64;
                    (per_s(now.tx_bytes.saturating_sub(prev.tx_bytes)), per_s(now.rx_bytes.saturating_sub(prev.rx_bytes)))
                } else {
                    (0, 0)
                }
            }
            None => (0, 0),
        };

        self.reports.push_back(now);
        while self.reports.front().is_some_and(|r| now.at.saturating_duration_since(r.at) > MAX_LOSS_WINDOW) {
            self.reports.pop_front();
        }
        let rating = Rating::of(rtt_ms, jitter_ms, loss_pct);
        Some(Quality {
            rtt_ms,
            jitter_ms,
            loss_pct,
            loss_window_s,
            tx_bytes_per_s,
            rx_bytes_per_s,
            level: rating.overall(),
            rating,
        })
    }
}

/// Einzelne RTT-Probe aus zwei geglätteten Werten, zwischen denen genau eine Aktualisierung lag
/// (neu = (7 · alt + Probe) / 8). Passt das Ergebnis nicht, bleibt es beim geglätteten Wert.
fn single_sample(old: Duration, new: Duration) -> Duration {
    (new * 8).checked_sub(old * 7).unwrap_or(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// Glättung wie in quinn (`RttEstimator::update` ohne ACK-Verzögerung).
    fn smooth(old: Duration, sample: Duration) -> Duration {
        (7 * old + sample) / 8
    }

    struct Fake {
        start: Instant,
        now: Reading,
    }

    impl Fake {
        fn new(rtt: Duration) -> Self {
            let start = Instant::now();
            let now = Reading { at: start, rtt, acks: 0, sent_packets: 0, lost_packets: 0, tx_bytes: 0, rx_bytes: 0 };
            Self { start, now }
        }

        fn at(&mut self, secs: f64) -> &mut Self {
            self.now.at = self.start + Duration::from_secs_f64(secs);
            self
        }

        /// Eine neue RTT-Probe kommt mit genau einem ACK-Frame an.
        fn ack(&mut self, sample: Duration) -> &mut Self {
            self.now.rtt = smooth(self.now.rtt, sample);
            self.now.acks += 1;
            self
        }

        fn packets(&mut self, sent: u64, lost: u64) -> &mut Self {
            self.now.sent_packets += sent;
            self.now.lost_packets += lost;
            self
        }

        fn reading(&self) -> Reading {
            self.now
        }
    }

    fn samples(meter: &Meter) -> Vec<f64> {
        meter.samples.iter().map(|(_, ms)| *ms).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn levels_follow_documented_limits() {
        assert_eq!(rate(0.4, 0.1, 0.0), Level::VeryGood);
        assert_eq!(rate(9.99, 2.99, 0.49), Level::VeryGood);
        assert_eq!(rate(10.0, 0.0, 0.0), Level::Good);
        assert_eq!(rate(29.9, 0.0, 0.0), Level::Good);
        assert_eq!(rate(30.0, 0.0, 0.0), Level::Fair);
        assert_eq!(rate(79.9, 0.0, 0.0), Level::Fair);
        assert_eq!(rate(80.0, 0.0, 0.0), Level::Poor);

        assert_eq!(rate(1.0, 3.0, 0.0), Level::Good);
        assert_eq!(rate(1.0, 10.0, 0.0), Level::Fair);
        assert_eq!(rate(1.0, 25.0, 0.0), Level::Poor);

        assert_eq!(rate(1.0, 0.0, 0.5), Level::Good);
        assert_eq!(rate(1.0, 0.0, 1.99), Level::Good);
        assert_eq!(rate(1.0, 0.0, 2.0), Level::Fair);
        assert_eq!(rate(1.0, 0.0, 5.0), Level::Poor);
        assert_eq!(rate(1.0, 0.0, 100.0), Level::Poor);
    }

    #[test]
    fn worst_value_decides() {
        assert_eq!(rate(5.0, 12.0, 0.0), Level::Fair);
        assert_eq!(rate(50.0, 1.0, 0.0), Level::Fair);
        assert_eq!(rate(50.0, 1.0, 6.0), Level::Poor);
        assert_eq!(rate(f64::NAN, 0.0, 0.0), Level::Poor);
        let rating = Rating::of(50.0, 1.0, 6.0);
        assert_eq!(rating, Rating { rtt: Level::Fair, jitter: Level::VeryGood, loss: Level::Poor });
        assert_eq!(rating.overall(), Level::Poor);
        assert_eq!(Level::VeryGood.max(Level::Fair), Level::Fair);
        assert_eq!(Level::Fair.label(), "Mäßig");
    }

    #[test]
    fn single_ack_reveals_raw_sample() {
        let mut fake = Fake::new(10 * MS);
        let mut meter = Meter::default();
        meter.observe(fake.reading());
        for (i, sample) in [18.0, 4.0, 10.5, 0.3, 37.25].into_iter().enumerate() {
            meter.observe(fake.at(i as f64 + 1.0).ack(Duration::from_secs_f64(sample / 1000.0)).reading());
            let got = *samples(&meter).last().unwrap();
            assert!((got - sample).abs() < 0.0001, "Probe {sample} ms zurückgerechnet als {got} ms");
        }
    }

    #[test]
    fn several_acks_use_smoothed_value_and_unchanged_rtt_adds_nothing() {
        let mut fake = Fake::new(10 * MS);
        let mut meter = Meter::default();
        meter.observe(fake.reading());
        fake.at(0.1).ack(20 * MS).ack(20 * MS).ack(20 * MS);
        let smoothed = fake.now.rtt.as_secs_f64() * 1000.0;
        meter.observe(fake.reading());
        assert_eq!(samples(&meter), vec![10.0, smoothed]);

        // ACK ohne neue Probe (z. B. für reine ACK-Pakete): kein Eintrag
        fake.now.acks += 1;
        meter.observe(fake.at(0.2).reading());
        assert_eq!(meter.samples.len(), 2);
    }

    #[test]
    fn jitter_is_mean_difference_of_consecutive_samples() {
        let mut fake = Fake::new(10 * MS);
        let mut meter = Meter::default();
        meter.observe(fake.reading());
        for (i, sample) in [12, 10, 14].into_iter().enumerate() {
            meter.observe(fake.at(i as f64 + 1.0).ack(sample * MS).reading());
        }
        let q = meter.report().unwrap();
        // Differenzen 2, 2 und 4 ms
        assert!(close(q.jitter_ms, 8.0 / 3.0), "{}", q.jitter_ms);
        assert!(close(q.rtt_ms, fake.now.rtt.as_secs_f64() * 1000.0));
    }

    #[test]
    fn steady_link_has_no_jitter_and_old_samples_expire() {
        let mut fake = Fake::new(5 * MS);
        let mut meter = Meter::default();
        meter.observe(fake.reading());
        meter.observe(fake.at(1.0).ack(25 * MS).reading());
        assert!(meter.report().unwrap().jitter_ms > 10.0);
        // Danach 15 s lang gleichmäßig 25 ms: der Sprung von 5 ms fällt aus dem Fenster.
        for i in 2..17 {
            meter.observe(fake.at(i as f64).ack(25 * MS).reading());
        }
        assert_eq!(meter.samples.len(), 11);
        let q = meter.report().unwrap();
        assert!(q.jitter_ms < 0.001, "{}", q.jitter_ms);
    }

    #[test]
    fn loss_over_ten_seconds_with_enough_traffic() {
        let mut fake = Fake::new(MS);
        let mut meter = Meter::default();
        for second in 0..=30 {
            // 200 Pakete pro Sekunde, in den letzten 10 s davon je 2 verloren
            let lost = if second > 20 { 2 } else { 0 };
            meter.observe(fake.at(second as f64).packets(200, lost).reading());
            let q = meter.report().unwrap();
            if second == 30 {
                assert_eq!(q.loss_window_s, 10);
                assert!(close(q.loss_pct, 1.0), "{}", q.loss_pct);
                assert_eq!(q.level, Level::Good);
            }
        }
    }

    #[test]
    fn little_traffic_widens_the_loss_window() {
        let mut fake = Fake::new(MS);
        let mut meter = Meter::default();
        let mut last = None;
        for second in 0..=90 {
            // Leerlauf: zwei Pakete pro Sekunde, ein einziges geht verloren
            let lost = u64::from(second == 70);
            meter.observe(fake.at(second as f64).packets(2, lost).reading());
            last = meter.report();
        }
        let q = last.unwrap();
        assert_eq!(q.loss_window_s, 50, "100 Pakete brauchen 50 s");
        assert!(close(q.loss_pct, 1.0), "{}", q.loss_pct);
    }

    #[test]
    fn loss_window_never_exceeds_a_minute() {
        let mut fake = Fake::new(MS);
        let mut meter = Meter::default();
        let mut last = None;
        for second in 0..=100 {
            meter.observe(fake.at(second as f64).packets(1, u64::from(second == 95)).reading());
            last = meter.report();
        }
        let q = last.unwrap();
        assert_eq!(q.loss_window_s, 60);
        // 1 von 60 Paketen, bezogen auf die Mindestzahl von 100
        assert!(close(q.loss_pct, 1.0), "{}", q.loss_pct);
    }

    #[test]
    fn few_packets_after_connecting_do_not_inflate_loss() {
        let mut fake = Fake::new(MS);
        let mut meter = Meter::default();
        let mut last = None;
        for second in 0..=4 {
            meter.observe(fake.at(second as f64).packets(2, u64::from(second == 2)).reading());
            last = meter.report();
        }
        let q = last.unwrap();
        assert_eq!(q.loss_window_s, 4);
        assert!(close(q.loss_pct, 1.0), "1 von 8 Paketen zählt wie 1 von 100: {}", q.loss_pct);
        assert_eq!(q.level, Level::Good);

        // Echter, hoher Verlust bleibt trotzdem sichtbar: 8 von 23 Paketen seit Beginn des Fensters
        meter.observe(fake.at(5.0).packets(15, 7).reading());
        let q = meter.report().unwrap();
        assert!(close(q.loss_pct, 8.0), "{}", q.loss_pct);
        assert_eq!(q.level, Level::Poor);
    }

    #[test]
    fn first_report_has_no_loss_and_no_throughput() {
        let mut fake = Fake::new(MS);
        let mut meter = Meter::default();
        assert!(meter.report().is_none());
        meter.observe(fake.packets(10, 3).reading());
        let q = meter.report().unwrap();
        assert_eq!((q.loss_pct, q.loss_window_s, q.tx_bytes_per_s), (0.0, 0, 0));
    }

    #[test]
    fn throughput_since_previous_report() {
        let mut fake = Fake::new(MS);
        let mut meter = Meter::default();
        meter.observe(fake.reading());
        meter.report();
        fake.now.tx_bytes += 3_000_000;
        fake.now.rx_bytes += 40_000;
        meter.observe(fake.at(2.0).reading());
        let q = meter.report().unwrap();
        assert_eq!((q.tx_bytes_per_s, q.rx_bytes_per_s), (1_500_000, 20_000));
    }
}
