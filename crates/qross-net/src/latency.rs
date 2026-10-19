//! Verzögerung der Eingabekette: von der Erfassung am Server, bis das Einspeisen am Client erledigt
//! ist, in drei Teilen (Server, Netz, Client), für Mausbewegungen und Tasten getrennt, als Median und
//! 95. Perzentil über die letzte Minute.
//!
//! - **Stichproben** ohne ein Byte mehr auf dem Eingabeweg: jedes Maus-Datagramm mit `seq` als
//!   Vielfachem von `MOUSE_SAMPLE_EVERY` und jeder Druck einer Taste oder Maustaste (siehe
//!   `qross_proto::is_sampled_input`), erkannt an der laufenden Nummer auf dem Input-Stream.
//! - **Server:** vom Abholen des Ereignisses aus der Erfassung bis der Versand an QUIC übergeben ist
//!   (`send_datagram` bzw. `write_frame` zurück). Den Thread-Wechsel vom Hook bis zum Abholen sieht
//!   die Messung nicht, denn dafür müsste jede Erfassung selbst Zeitstempel setzen.
//! - **Client:** vom Empfang (das Datagramm bzw. die Nachricht ist gelesen) bis das Einspeisen im
//!   Einspeise-Thread erledigt ist. Er misst mit seiner eigenen Uhr und meldet etwa alle 200 ms zurück,
//!   wann jede Stichprobe ankam und wie lange sie bei ihm brauchte.
//! - **Netz:** Die Uhren beider Rechner haben verschiedene Nullpunkte und laufen leicht verschieden
//!   schnell. Wie bei NTP ergibt jede Stichprobe mit ihrer Rückmeldung einen Austausch mit vier
//!   Zeitpunkten: Versand am Server (T1), Empfang am Client (T2), Rückmeldung ab Client (T3), an beim
//!   Server (T4). Daraus folgen Umlaufzeit δ = (T4 − T1) − (T3 − T2) und Versatz θ = ((T2 − T1) + (T3 − T4)) / 2.
//!   Gilt der Versatz aus dem Austausch mit dem kleinsten δ / 2 + 15 µs je Sekunde Alter der letzten
//!   10 s (der Alterungszuschlag deckt auseinanderlaufende Uhren ab, wie `PHI` bei NTP), ist der Netzanteil
//!   einer Stichprobe T2 − T1 − θ, begrenzt auf 0 bis δ.
//!
//! **Genauigkeit:** Server- und Client-Anteil messen je eine Uhr, auf etwa eine Mikrosekunde genau. Den
//! Netzanteil kann niemand ohne gemeinsame Uhr genau kennen: Für den besten Austausch ist er δ / 2,
//! richtig nur, wenn Hin- und Rückweg gleich lang dauern. Er liegt höchstens um diesen Wert daneben
//! (`LatencySummary::uncertainty_us`, auf Loopback einige zehn Mikrosekunden, im LAN einige hundert).
//! Schwankungen einzelner Stichproben (etwa ein voller Puffer im WLAN) zeigt er dagegen richtig, denn
//! sie bleiben im Unterschied T2 − T1 erhalten. Die Wartezeit beim Client bis zur Rückmeldung (bis
//! 200 ms) geht mit der Gangabweichung der Uhren ein, bei 50 ppm also mit höchstens 10 µs.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qross_proto::{
    is_sampled_input, is_sampled_move, Chain, Input, LatencyReport, LatencySample, LatencySummary, SampleId, Spread,
    MAX_REPORT_SAMPLES,
};

/// Statistik über so lange
pub const WINDOW: Duration = Duration::from_secs(60);
/// Höchstens so viele Stichproben je Art in der Statistik
const MAX_SAMPLES: usize = 1000;
/// So lange wartet eine gesendete Stichprobe auf die Rückmeldung des Clients
const PENDING_FOR: Duration = Duration::from_secs(5);
const MAX_PENDING: usize = 512;
/// Aus Austauschen der letzten Sekunden wird der Uhrenversatz geschätzt
const OFFSET_WINDOW: Duration = Duration::from_secs(10);
const MAX_EXCHANGES: usize = 256;
/// Zuschlag für das Alter eines Austauschs: So weit können zwei Quarzuhren je Sekunde auseinanderlaufen.
const DRIFT_US_PER_S: f64 = 15.0;
/// Der Client sammelt Stichproben höchstens so lange, bevor er sie meldet
pub const REPORT_AFTER: Duration = Duration::from_millis(200);
/// So oft gibt der Server ein neues Ergebnis heraus, wenn sich etwas geändert hat
pub const SUMMARY_EVERY: Duration = Duration::from_secs(1);
/// Ab so lange gilt eine Stichprobe als Ausreißer und kommt mit ihren Teilen ins Log. Median und
/// 95. Perzentil zeigen einen kurzen Hänger beim Wechsel sonst nicht.
const SLOW_US: u32 = 50_000;

/// Die langsamste Stichprobe über [`SLOW_US`] seit der letzten Abfrage, und wie viele es waren
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slow {
    pub mouse: bool,
    pub server_us: u32,
    pub network_us: u32,
    pub client_us: u32,
    pub count: u32,
}

impl Slow {
    fn total_us(&self) -> u32 {
        self.server_us.saturating_add(self.network_us).saturating_add(self.client_us)
    }

    /// Fürs Log, z. B. „Maus 142 ms (Server 0,1 · Netz 138,0 · Client 3,9), 3 Stichproben über 50 ms“
    pub fn describe(&self) -> String {
        let ms = |us: u32| format!("{:.1}", f64::from(us) / 1000.0).replace('.', ",");
        format!(
            "{} {} ms (Server {} · Netz {} · Client {}), {} {} über {} ms",
            if self.mouse { "Maus" } else { "Taste" },
            self.total_us() / 1000,
            ms(self.server_us),
            ms(self.network_us),
            ms(self.client_us),
            self.count,
            if self.count == 1 { "Stichprobe" } else { "Stichproben" },
            SLOW_US / 1000
        )
    }
}

/// Messung einer Verbindung beim Server, geteilt zwischen Ereignisschleife, Input-Stream und Control-Stream
pub type Shared = Arc<Mutex<Meter>>;

/// Eine Stichprobe beim Client: angekommen und eingespeist
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    pub id: SampleId,
    pub received: Instant,
    pub done: Instant,
}

/// Wann und als was eine Eingabe beim Client ankam
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub id: SampleId,
    pub received: Instant,
}

impl Stamp {
    /// Für ein Maus-Datagramm, wenn es eine Stichprobe ist
    pub fn mouse(epoch: u16, seq: u32, received: Instant) -> Option<Self> {
        is_sampled_move(seq).then_some(Self { id: SampleId::Mouse { epoch, seq }, received })
    }

    /// Für die `index`-te Nachricht auf dem Input-Stream (ab 1), wenn sie eine Stichprobe ist
    pub fn input(input: &Input, index: u64, received: Instant) -> Option<Self> {
        is_sampled_input(input).then_some(Self { id: SampleId::Input { index }, received })
    }
}

/// Gesendet, wartet auf die Rückmeldung
#[derive(Debug, Clone, Copy)]
struct Sent {
    at: Instant,
    server_us: u32,
}

/// Mit Rückmeldung: alle drei Teile (µs)
#[derive(Debug, Clone, Copy)]
struct Complete {
    /// Versand am Server
    at: Instant,
    server_us: u32,
    network_us: u32,
    client_us: u32,
}

/// Ein Austausch für den Uhrenversatz
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Exchange {
    /// Rückmeldung angekommen
    at: Instant,
    /// Umlaufzeit ohne die Wartezeit beim Client (µs)
    delay_us: i64,
    /// Uhr des Clients minus Uhr des Servers (µs)
    offset_us: i64,
}

impl Exchange {
    /// Wie weit der Versatz daneben liegen kann: halbe Umlaufzeit plus Zuschlag fürs Alter
    fn distance_us(&self, now: Instant) -> f64 {
        self.delay_us as f64 / 2.0 + DRIFT_US_PER_S * now.saturating_duration_since(self.at).as_secs_f64()
    }
}

#[derive(Default)]
struct Series {
    /// Versandzeitpunkt und Server-Anteil jeder Stichprobe
    server: VecDeque<(Instant, u32)>,
    /// Stichproben mit Rückmeldung
    complete: VecDeque<Complete>,
}

impl Series {
    fn prune(&mut self, now: Instant) -> bool {
        let old = |at: Instant| now.saturating_duration_since(at) > WINDOW;
        let before = (self.server.len(), self.complete.len());
        while self.server.len() > MAX_SAMPLES || self.server.front().is_some_and(|(at, _)| old(*at)) {
            self.server.pop_front();
        }
        while self.complete.len() > MAX_SAMPLES || self.complete.front().is_some_and(|c| old(c.at)) {
            self.complete.pop_front();
        }
        before != (self.server.len(), self.complete.len())
    }

    fn chain(&self) -> Option<Chain> {
        if self.server.is_empty() {
            return None;
        }
        let parts = |part: fn(&Complete) -> u32| (!self.complete.is_empty()).then(|| spread(self.complete.iter().map(part)));
        Some(Chain {
            samples: self.server.len() as u32,
            server: spread(self.server.iter().map(|(_, us)| *us)),
            reported: self.complete.len() as u32,
            network: parts(|c| c.network_us),
            client: parts(|c| c.client_us),
            total: parts(|c| c.server_us.saturating_add(c.network_us).saturating_add(c.client_us)),
        })
    }
}

/// Messung beim Server für einen Client; ohne Netz, die Zeitpunkte kommen von außen.
pub struct Meter {
    /// Nullpunkt der Server-Uhr für den Vergleich mit dem Client
    epoch: Instant,
    pending: HashMap<SampleId, Sent>,
    /// Reihenfolge von `pending` zum Aufräumen
    order: VecDeque<(Instant, SampleId)>,
    mouse: Series,
    keys: Series,
    exchanges: VecDeque<Exchange>,
    /// Der Client misst mit (er hat sich gemeldet, auch ohne Stichproben)
    reporting: bool,
    /// Seit dem letzten Ergebnis hat sich etwas geändert
    changed: bool,
    /// Ausreißer seit der letzten Abfrage
    slow: Option<Slow>,
}

impl Meter {
    pub fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            pending: HashMap::new(),
            order: VecDeque::new(),
            mouse: Series::default(),
            keys: Series::default(),
            exchanges: VecDeque::new(),
            reporting: false,
            changed: false,
            slow: None,
        }
    }

    /// Der langsamste Ausreißer seit dem letzten Aufruf
    pub fn take_slow(&mut self) -> Option<Slow> {
        self.slow.take()
    }

    fn note_slow(&mut self, mouse: bool, c: &Complete) {
        let this = Slow { mouse, server_us: c.server_us, network_us: c.network_us, client_us: c.client_us, count: 1 };
        if this.total_us() < SLOW_US {
            return;
        }
        self.slow = Some(match self.slow {
            Some(slow) if slow.total_us() >= this.total_us() => Slow { count: slow.count + 1, ..slow },
            Some(slow) => Slow { count: slow.count + 1, ..this },
            None => this,
        });
    }

    pub fn shared(epoch: Instant) -> Shared {
        Arc::new(Mutex::new(Self::new(epoch)))
    }

    /// Meldet der Client seine Zeiten zurück? Erst dann gibt es Netz- und Client-Anteil, und er
    /// bekommt das Ergebnis.
    pub fn client_reports(&self) -> bool {
        self.reporting
    }

    fn series(&mut self, id: SampleId) -> &mut Series {
        match id {
            SampleId::Mouse { .. } => &mut self.mouse,
            SampleId::Input { .. } => &mut self.keys,
        }
    }

    fn micros_at(&self, at: Instant) -> i64 {
        at.saturating_duration_since(self.epoch).as_micros() as i64
    }

    /// Eine Stichprobe ist an QUIC übergeben: aus der Erfassung abgeholt um `captured`, gesendet um `sent`.
    pub fn sent(&mut self, id: SampleId, captured: Instant, sent: Instant) {
        let server_us = micros(sent.saturating_duration_since(captured));
        self.series(id).server.push_back((sent, server_us));
        self.pending.insert(id, Sent { at: sent, server_us });
        self.order.push_back((sent, id));
        while self.order.len() > MAX_PENDING || self.order.front().is_some_and(|(at, _)| sent.saturating_duration_since(*at) > PENDING_FOR) {
            let (at, id) = self.order.pop_front().unwrap();
            // Nur entfernen, wenn es noch dieselbe ist (`seq` läuft irgendwann über)
            if self.pending.get(&id).is_some_and(|p| p.at == at) {
                self.pending.remove(&id);
            }
        }
        self.changed = true;
    }

    /// Rückmeldung des Clients, angekommen um `received`
    pub fn report(&mut self, report: &LatencyReport, received: Instant) {
        if !self.reporting {
            self.reporting = true;
            self.changed = true;
        }
        let (t3, t4) = (report.sent_us as i64, self.micros_at(received));
        let mut matched = Vec::new();
        for sample in report.samples.iter().take(MAX_REPORT_SAMPLES) {
            let Some(sent) = self.pending.remove(&sample.id) else { continue };
            let t1 = self.micros_at(sent.at);
            let t2 = t3 - i64::from(sample.age_us);
            let delay_us = ((t4 - t1) - i64::from(sample.age_us)).max(0);
            let offset_us = ((t2 - t1) + (t3 - t4)) / 2;
            self.exchanges.push_back(Exchange { at: received, delay_us, offset_us });
            matched.push((*sample, sent, t2 - t1, delay_us));
        }
        while self.exchanges.len() > MAX_EXCHANGES
            || self.exchanges.front().is_some_and(|e| received.saturating_duration_since(e.at) > OFFSET_WINDOW)
        {
            self.exchanges.pop_front();
        }
        let Some(best) = self.best_exchange(received) else { return };
        for (sample, sent, forward_us, delay_us) in matched {
            let LatencySample { id, client_us, .. } = sample;
            let network_us = (forward_us - best.offset_us).clamp(0, delay_us) as u32;
            let complete = Complete { at: sent.at, server_us: sent.server_us, network_us, client_us };
            self.note_slow(matches!(id, SampleId::Mouse { .. }), &complete);
            self.series(id).complete.push_back(complete);
            self.changed = true;
        }
    }

    fn best_exchange(&self, now: Instant) -> Option<Exchange> {
        self.exchanges.iter().copied().min_by(|a, b| a.distance_us(now).total_cmp(&b.distance_us(now)))
    }

    /// Das aktuelle Ergebnis (alte Stichproben fallen dabei heraus)
    pub fn summary(&mut self, now: Instant) -> LatencySummary {
        let pruned = self.mouse.prune(now) | self.keys.prune(now);
        self.changed |= pruned;
        LatencySummary {
            mouse: self.mouse.chain(),
            keys: self.keys.chain(),
            uncertainty_us: self.best_exchange(now).map(|e| e.distance_us(now).round() as u32),
        }
    }

    /// Das Ergebnis, wenn sich seit dem letzten etwas geändert hat
    pub fn take_summary(&mut self, now: Instant) -> Option<LatencySummary> {
        let summary = self.summary(now);
        std::mem::take(&mut self.changed).then_some(summary)
    }
}

/// Beim Client: sammelt die Zeiten der Stichproben, bis sie gemeldet werden.
pub struct Collector {
    /// Nullpunkt der eigenen Uhr in den Rückmeldungen
    epoch: Instant,
    samples: VecDeque<Measured>,
}

impl Collector {
    pub fn new(epoch: Instant) -> Self {
        Self { epoch, samples: VecDeque::new() }
    }

    /// Liefert `true`, wenn vorher nichts wartete (dann in `REPORT_AFTER` melden).
    pub fn add(&mut self, measured: Measured) -> bool {
        if self.samples.len() == MAX_REPORT_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(measured);
        self.samples.len() == 1
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Rückmeldung mit allem Gesammelten; `now` unmittelbar vor dem Absenden
    pub fn report(&mut self, now: Instant) -> LatencyReport {
        let samples = self
            .samples
            .drain(..)
            .map(|m| LatencySample {
                id: m.id,
                age_us: micros(now.saturating_duration_since(m.received)),
                client_us: micros(m.done.saturating_duration_since(m.received)),
            })
            .collect();
        LatencyReport { sent_us: now.saturating_duration_since(self.epoch).as_micros() as u64, samples }
    }
}

fn micros(d: Duration) -> u32 {
    d.as_micros().min(u128::from(u32::MAX)) as u32
}

/// Median und 95. Perzentil (Nearest-Rank: der kleinste Wert, bis zu dem mindestens so viel Prozent reichen)
pub fn spread(values: impl Iterator<Item = u32>) -> Spread {
    let mut sorted: Vec<u32> = values.collect();
    sorted.sort_unstable();
    Spread { p50_us: percentile(&sorted, 50), p95_us: percentile(&sorted, 95) }
}

fn percentile(sorted: &[u32], pct: usize) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// Fürs Log, z. B. „Maus 0,9 ms (95 %: 1,6 ms; Server 0,1 · Netz 0,3 · Client 0,5)“
pub fn describe(summary: &LatencySummary) -> String {
    let ms = |us: u32| format!("{:.2}", f64::from(us) / 1000.0).replace('.', ",");
    let chain = |label: &str, chain: &Chain| match (chain.total, chain.network, chain.client) {
        (Some(total), Some(network), Some(client)) => format!(
            "{label} {} ms (95 %: {} ms; Server {} · Netz {} · Client {}; {} Stichproben)",
            ms(total.p50_us),
            ms(total.p95_us),
            ms(chain.server.p50_us),
            ms(network.p50_us),
            ms(client.p50_us),
            chain.reported
        ),
        _ => format!("{label}: Server {} ms (95 %: {} ms; {} Stichproben)", ms(chain.server.p50_us), ms(chain.server.p95_us), chain.samples),
    };
    let parts: Vec<String> = [("Maus", summary.mouse), ("Tasten", summary.keys)]
        .into_iter()
        .filter_map(|(label, c)| c.map(|c| chain(label, &c)))
        .collect();
    if parts.is_empty() {
        "noch keine Stichproben".to_owned()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const US: Duration = Duration::from_micros(1);

    fn mouse(seq: u32) -> SampleId {
        SampleId::Mouse { epoch: 1, seq }
    }

    /// Spielt eine Stichprobe durch: Server-Uhr `start`, Client-Uhr um `offset` µs voraus.
    struct Link {
        server_epoch: Instant,
        /// Client-Uhr minus Server-Uhr in µs (positiv: Client vorn)
        offset_us: i64,
    }

    impl Link {
        fn client_us(&self, at: Instant) -> u64 {
            (at.duration_since(self.server_epoch).as_micros() as i64 + self.offset_us) as u64
        }

        /// Stichprobe `id`, erfasst um `t0`, mit den Zeiten aus `timing`
        fn sample(&self, meter: &mut Meter, id: SampleId, t0: Instant, timing: Timing) {
            let Timing { server, forward, client, hold, back } = timing;
            let t1 = t0 + server * US;
            meter.sent(id, t0, t1);
            let t2 = t1 + forward * US;
            let t3 = t2 + hold * US;
            let report = LatencyReport {
                sent_us: self.client_us(t3),
                samples: vec![LatencySample { id, age_us: hold, client_us: client }],
            };
            meter.report(&report, t3 + back * US);
        }
    }

    /// In µs: nach `server` gesendet, nach `forward` beim Client, nach `client` eingespeist, `hold`
    /// nach dem Empfang gemeldet, nach `back` beim Server
    #[derive(Clone, Copy)]
    struct Timing {
        server: u32,
        forward: u32,
        client: u32,
        hold: u32,
        back: u32,
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let s = spread((1..=100).rev());
        assert_eq!((s.p50_us, s.p95_us), (50, 95));
        let s = spread([7].into_iter());
        assert_eq!((s.p50_us, s.p95_us), (7, 7));
        // Gerade Anzahl: der untere der beiden mittleren Werte, kein Mittelwert
        let s = spread([1, 2, 3, 4].into_iter());
        assert_eq!((s.p50_us, s.p95_us), (2, 4));
        // Ein Ausreißer unter 20 Werten zählt fürs 95. Perzentil noch nicht, zwei schon
        let mut values = vec![10; 19];
        values.push(900);
        assert_eq!(spread(values.iter().copied()).p95_us, 10);
        values[0] = 800;
        assert_eq!(spread(values.iter().copied()).p95_us, 800);
        assert_eq!(spread(std::iter::empty()), Spread::default());
    }

    #[test]
    fn symmetric_link_yields_exact_parts_whatever_the_clock_offset() {
        // Die Uhr des Clients geht 5000 s vor, gleich oder 3 s nach (die Proben beginnen erst nach 10 s).
        for offset_us in [5_000_000_000, 0, -3_000_000] {
            let start = Instant::now();
            let link = Link { server_epoch: start, offset_us };
            let mut meter = Meter::new(start);
            // Hin und zurück je 400 µs, Server 50 µs, Client 300 µs, Rückmeldung nach 150 ms
            let timing = Timing { server: 50, forward: 400, client: 300, hold: 150_000, back: 400 };
            for seq in 0..10 {
                let t0 = start + Duration::from_secs(10) + Duration::from_millis(10 * seq as u64);
                link.sample(&mut meter, mouse(seq), t0, timing);
            }
            let summary = meter.summary(start + Duration::from_secs(11));
            let chain = summary.mouse.unwrap();
            assert_eq!((chain.samples, chain.reported), (10, 10));
            assert_eq!(chain.server, Spread { p50_us: 50, p95_us: 50 });
            assert_eq!(chain.network, Some(Spread { p50_us: 400, p95_us: 400 }), "Versatz {offset_us}");
            assert_eq!(chain.client, Some(Spread { p50_us: 300, p95_us: 300 }));
            assert_eq!(chain.total, Some(Spread { p50_us: 750, p95_us: 750 }));
            assert!(summary.keys.is_none());
            // Halbe Umlaufzeit (400 µs) plus etwas Alterung
            let uncertainty = summary.uncertainty_us.unwrap();
            assert!((400..420).contains(&uncertainty), "{uncertainty}");
        }
    }

    #[test]
    fn the_fastest_exchange_sets_the_offset_and_spikes_stay_visible() {
        let start = Instant::now();
        let link = Link { server_epoch: start, offset_us: 123_456 };
        let mut meter = Meter::new(start);
        // Grundlaufzeit 300 µs je Richtung; jede fünfte Stichprobe steckt hin 5 ms im Puffer.
        for i in 0..40u32 {
            let t0 = start + Duration::from_millis(5 * u64::from(i));
            let forward = if i % 5 == 4 { 5_300 } else { 300 };
            let timing = Timing { server: 20, forward, client: 100, hold: 50_000, back: 300 };
            link.sample(&mut meter, SampleId::Input { index: u64::from(i) + 1 }, t0, timing);
        }
        let chain = meter.summary(start + Duration::from_secs(1)).keys.unwrap();
        let network = chain.network.unwrap();
        assert_eq!(network.p50_us, 300, "der Versatz stammt aus einem schnellen Austausch");
        assert_eq!(network.p95_us, 5_300, "die Spitzen bleiben in voller Höhe sichtbar");
        assert_eq!(chain.total.unwrap().p95_us, 20 + 5_300 + 100);
    }

    #[test]
    fn outliers_are_kept_with_their_parts_until_taken() {
        let start = Instant::now();
        let link = Link { server_epoch: start, offset_us: -40_000 };
        let mut meter = Meter::new(start);
        // Wie beim Wechsel auf einen Mac im WLAN: zwei Stichproben stecken lange im Netz.
        for (i, forward) in [300, 300, 120_300, 300, 80_300, 300].into_iter().enumerate() {
            let t0 = start + Duration::from_millis(10 * i as u64);
            let timing = Timing { server: 20, forward, client: 100, hold: 50_000, back: 300 };
            link.sample(&mut meter, mouse(32 * (i as u32 + 1)), t0, timing);
        }
        let slow = meter.take_slow().expect("Ausreißer");
        assert_eq!(slow, Slow { mouse: true, server_us: 20, network_us: 120_300, client_us: 100, count: 2 });
        assert_eq!(slow.describe(), "Maus 120 ms (Server 0,0 · Netz 120,3 · Client 0,1), 2 Stichproben über 50 ms");
        assert_eq!(meter.take_slow(), None, "einmal abgeholt");
        // Ein hängender Einspeise-Thread beim Client zählt genauso.
        let timing = Timing { server: 20, forward: 300, client: 70_000, hold: 50_000, back: 300 };
        link.sample(&mut meter, SampleId::Input { index: 1 }, start + Duration::from_millis(100), timing);
        assert_eq!(meter.take_slow().map(|s| (s.mouse, s.client_us, s.count)), Some((false, 70_000, 1)));
    }

    #[test]
    fn asymmetric_link_is_off_by_at_most_half_the_round_trip() {
        let start = Instant::now();
        let link = Link { server_epoch: start, offset_us: -42 };
        let mut meter = Meter::new(start);
        // Hin 200 µs, zurück 1000 µs: Erkennbar ist nur die Summe, geschätzt wird die Hälfte.
        let timing = Timing { server: 0, forward: 200, client: 0, hold: 1_000, back: 1_000 };
        for seq in 0..5 {
            link.sample(&mut meter, mouse(seq), start + Duration::from_millis(seq as u64), timing);
        }
        let summary = meter.summary(start + Duration::from_millis(100));
        let network = summary.mouse.unwrap().network.unwrap().p50_us;
        assert_eq!(network, 600);
        let uncertainty = summary.uncertainty_us.unwrap();
        assert!(network.abs_diff(200) <= uncertainty, "{network} ± {uncertainty} enthält die echten 200 µs");
    }

    #[test]
    fn drifting_clocks_prefer_recent_exchanges() {
        // Die Client-Uhr geht 100 ppm vor. Ein alter, etwas schnellerer Austausch verliert gegen einen
        // frischen: 9 s Alter kosten 135 µs Zuschlag, mehr als die 20 µs Vorteil.
        let start = Instant::now();
        let mut meter = Meter::new(start);
        let drift = |at: Instant| (at.duration_since(start).as_micros() as f64 * 100e-6) as i64;
        let send = |meter: &mut Meter, seq: u32, at: Instant, one_way: u32| {
            let link = Link { server_epoch: start, offset_us: 1_000 + drift(at) };
            link.sample(meter, mouse(seq), at, Timing { server: 0, forward: one_way, client: 0, hold: 0, back: one_way });
        };
        send(&mut meter, 0, start, 280);
        send(&mut meter, 1, start + Duration::from_secs(9), 300);
        let network = meter.summary(start + Duration::from_secs(9)).mouse.unwrap().network.unwrap();
        // Mit dem frischen Versatz stimmt die neue Stichprobe genau; mit dem alten käme sie 900 µs zu hoch
        // heraus (9 s × 100 ppm) und würde auf die Umlaufzeit begrenzt.
        assert_eq!(network.p95_us, 300);
    }

    #[test]
    fn older_clients_leave_only_the_server_part() {
        let start = Instant::now();
        let mut meter = Meter::new(start);
        for seq in 0..3 {
            let t0 = start + Duration::from_millis(seq as u64);
            meter.sent(mouse(seq * 32), t0, t0 + 80 * US);
        }
        let summary = meter.take_summary(start + Duration::from_millis(10)).unwrap();
        let chain = summary.mouse.unwrap();
        assert_eq!((chain.samples, chain.reported), (3, 0));
        assert_eq!(chain.server.p50_us, 80);
        assert_eq!((chain.network, chain.client, chain.total), (None, None, None));
        assert_eq!(summary.uncertainty_us, None);
        assert!(!meter.client_reports());
        assert!(meter.take_summary(start + Duration::from_millis(20)).is_none(), "nichts Neues");
        assert!(describe(&summary).starts_with("Maus: Server 0,08 ms"), "{}", describe(&summary));
    }

    #[test]
    fn unknown_and_expired_samples_are_ignored() {
        let start = Instant::now();
        let link = Link { server_epoch: start, offset_us: 0 };
        let mut meter = Meter::new(start);
        // Rückmeldung für etwas, das der Server nie gesendet hat
        let report = LatencyReport {
            sent_us: 10,
            samples: vec![LatencySample { id: mouse(64), age_us: 1, client_us: 1 }],
        };
        meter.report(&report, start);
        assert!(meter.client_reports(), "auch eine Rückmeldung ohne Treffer zeigt: der Client misst mit");
        // Zu spät: Die Stichprobe ist nach 5 s vergessen
        meter.sent(mouse(0), start, start);
        meter.sent(mouse(32), start + Duration::from_secs(6), start + Duration::from_secs(6));
        let late = LatencyReport {
            sent_us: link.client_us(start + Duration::from_secs(6)),
            samples: vec![LatencySample { id: mouse(0), age_us: 100, client_us: 100 }],
        };
        meter.report(&late, start + Duration::from_secs(6));
        let chain = meter.summary(start + Duration::from_secs(6)).mouse.unwrap();
        assert_eq!((chain.samples, chain.reported), (2, 0));
    }

    #[test]
    fn statistics_cover_the_last_minute() {
        let start = Instant::now();
        let mut meter = Meter::new(start);
        meter.sent(mouse(0), start, start + 900 * US);
        meter.sent(mouse(32), start + Duration::from_secs(50), start + Duration::from_secs(50) + 100 * US);
        assert_eq!(meter.take_summary(start + Duration::from_secs(55)).unwrap().mouse.unwrap().server.p95_us, 900);
        let later = meter.take_summary(start + Duration::from_secs(61)).expect("die alte Stichprobe fällt heraus");
        assert_eq!(later.mouse.unwrap().samples, 1);
        assert_eq!(later.mouse.unwrap().server.p95_us, 100);
        assert!(meter.take_summary(start + Duration::from_secs(200)).unwrap().mouse.is_none());
    }

    #[test]
    fn collector_reports_ages_against_its_own_clock() {
        let epoch = Instant::now();
        let mut collector = Collector::new(epoch);
        assert!(collector.is_empty());
        let received = epoch + Duration::from_millis(5);
        assert!(collector.add(Measured { id: mouse(32), received, done: received + 250 * US }));
        assert!(!collector.add(Measured { id: SampleId::Input { index: 3 }, received: received + 100 * US, done: received + 900 * US }));
        let report = collector.report(epoch + Duration::from_millis(105));
        assert!(collector.is_empty());
        assert_eq!(report.sent_us, 105_000);
        assert_eq!(
            report.samples,
            vec![
                LatencySample { id: mouse(32), age_us: 100_000, client_us: 250 },
                LatencySample { id: SampleId::Input { index: 3 }, age_us: 99_900, client_us: 800 },
            ]
        );
        for i in 0..100 {
            collector.add(Measured { id: mouse(i), received, done: received });
        }
        let report = collector.report(received);
        assert_eq!(report.samples.len(), MAX_REPORT_SAMPLES);
        assert_eq!(report.samples[0].id, mouse(100 - MAX_REPORT_SAMPLES as u32), "die neuesten bleiben");
    }
}
