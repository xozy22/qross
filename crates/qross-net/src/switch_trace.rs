//! Protokoll jedes Wechsels auf einen Client: eine Zeile je Seite im Log, kurz danach (als Info, wenn
//! etwas auffällt, sonst zur Fehlersuche). Hakt der Cursor beim Wechsel, zeigt sie, wo die Zeit bleibt. Die Messung der Eingabekette ([`crate::latency`]) sieht
//! das nicht: Sie nimmt nur jede 32. Bewegung, und was vor dem Wechsel an der Kante geschieht oder beim
//! Client vor dem `Enter` ankommt, zählt sie gar nicht.
//!
//! - **Server:** Wie oft und wie lange lief der Cursor gegen die Kante, bis gewechselt wurde? Kamen
//!   danach die Bewegungen aus der Erfassung lückenlos (größte Lücke, größter Einzelschritt), und kehrte
//!   der Cursor gleich wieder zurück?
//! - **Client:** Wie viele Bewegungen kamen vor dem `Enter` an (die neueste folgt ihm gleich, siehe
//!   `ClientState`)? Wann kam die erste danach, wie groß war die größte Lücke beim Empfang, und wie
//!   lange nach dem Empfang war jede eingespeist?
//!
//! Stimmt auf beiden Seiten alles, liegt ein Haken hinter dem Einspeisen, beim Zeichnen des Systems.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tracing::{debug, info};

/// So lange nach dem Wechsel wird beobachtet
pub const WINDOW: Duration = Duration::from_millis(300);
/// Kantenberührungen mit höchstens diesem Abstand gehören zu einem Anlauf
const HIT_GAP: Duration = Duration::from_millis(300);
/// So viele Bewegungen vor einem `Enter` merkt sich der Client höchstens
const MAX_EARLY: usize = 512;
/// Ab hier fällt eine Lücke oder Wartezeit auf (ein Bild bei 60 Hz dauert 17 ms)
const NOTABLE: Duration = Duration::from_millis(30);

fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0).replace('.', ",")
}

fn moves(n: u32) -> String {
    if n == 1 {
        "1 Bewegung".to_owned()
    } else {
        format!("{n} Bewegungen")
    }
}

/// Server: Berührungen der Kante vor dem Wechsel
#[derive(Debug, Default)]
pub struct EdgeHits {
    first: Option<Instant>,
    last: Option<Instant>,
    count: u32,
}

impl EdgeHits {
    pub fn hit(&mut self, now: Instant) {
        if self.last.is_none_or(|last| now.saturating_duration_since(last) > HIT_GAP) {
            self.first = Some(now);
            self.count = 0;
        }
        self.count += 1;
        self.last = Some(now);
    }

    /// Beim Wechsel: wie oft die Kante in diesem Anlauf berührt wurde und wie lange er dauerte
    fn take(&mut self, now: Instant) -> (u32, Duration) {
        let run = match (self.first, self.last) {
            (Some(first), Some(last)) if now.saturating_duration_since(last) <= HIT_GAP => {
                (self.count, now.saturating_duration_since(first))
            }
            _ => (0, Duration::ZERO),
        };
        *self = Self::default();
        run
    }
}

/// Server: was nach dem Wechsel geschah
#[derive(Debug)]
pub struct ServerTrace {
    to: String,
    at: Instant,
    hits: u32,
    pushed: Duration,
    moves: u32,
    first: Option<Duration>,
    last: Instant,
    max_gap: Duration,
    max_step: i32,
    back: Option<Duration>,
}

impl ServerTrace {
    /// Der Wechsel zu `to` ist beschlossen; `at`: wann das auslösende Ereignis abgeholt wurde
    pub fn start(to: String, at: Instant, hits: &mut EdgeHits) -> Self {
        let (hits, pushed) = hits.take(at);
        Self { to, at, hits, pushed, moves: 0, first: None, last: at, max_gap: Duration::ZERO, max_step: 0, back: None }
    }

    /// Eine Bewegung aus der Erfassung, abgeholt um `at` (schon wartende sind zusammengefasst)
    pub fn on_move(&mut self, at: Instant, dx: i32, dy: i32) {
        if self.back.is_some() || at.saturating_duration_since(self.at) > WINDOW {
            return;
        }
        self.moves += 1;
        self.first.get_or_insert(at.saturating_duration_since(self.at));
        self.max_gap = self.max_gap.max(at.saturating_duration_since(self.last));
        self.max_step = self.max_step.max(dx.saturating_abs().max(dy.saturating_abs()));
        self.last = at;
    }

    /// Der Cursor ist zurück auf dem Server
    pub fn on_back(&mut self, at: Instant) {
        if self.back.is_none() && at.saturating_duration_since(self.at) <= WINDOW {
            self.back = Some(at.saturating_duration_since(self.at));
        }
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) > WINDOW
    }

    /// Sichtbar: an der Kante gewartet, eine Lücke in der Erfassung oder gleich zurück
    fn is_notable(&self) -> bool {
        (self.hits > 1 && self.pushed >= NOTABLE) || self.max_gap >= NOTABLE || self.back.is_some()
    }

    pub fn log(&self) {
        if self.is_notable() {
            info!("{}", self.describe());
        } else {
            debug!("{}", self.describe());
        }
    }

    /// Fürs Log, z. B. „Wechsel zu „mac“: Kante 1× berührt; in den ersten 300 ms 240 Bewegungen
    /// erfasst, die erste nach 1,0 ms, größte Lücke 4,1 ms, größter Schritt 18 px“
    pub fn describe(&self) -> String {
        let edge = match self.hits {
            0 => "ohne Kante am Server".to_owned(),
            1 => "Kante 1× berührt".to_owned(),
            n => format!("Kante {n}× berührt, {} ms lang", ms(self.pushed)),
        };
        let captured = match (self.moves, self.first) {
            (0, _) | (_, None) => "keine Bewegung erfasst".to_owned(),
            (n, Some(first)) => format!(
                "{} erfasst, die erste nach {} ms, größte Lücke {} ms, größter Schritt {} px",
                moves(n),
                ms(first),
                ms(self.max_gap),
                self.max_step
            ),
        };
        let back = self.back.map(|after| format!(", nach {} ms zurück auf dem Server", ms(after))).unwrap_or_default();
        format!("Wechsel zu „{}“: {edge}; in den ersten {} ms {captured}{back}", self.to, WINDOW.as_millis())
    }
}

/// Client: Bewegungen, die nicht zum laufenden Besuch gehörten (Epoche, Empfang): Nachzügler früherer
/// Besuche und solche, die vor dem `Enter` ihres Besuchs ankamen
#[derive(Debug, Default)]
pub struct EarlyMoves(VecDeque<(u16, Instant)>);

impl EarlyMoves {
    pub fn add(&mut self, epoch: u16, received: Instant) {
        if self.0.len() == MAX_EARLY {
            self.0.pop_front();
        }
        self.0.push_back((epoch, received));
    }

    /// Beim `Enter` mit `epoch`: so viele dieses Besuchs kamen schon vorher an, die erste so lange davor
    fn take(&mut self, epoch: u16, entered: Instant) -> (u32, Duration) {
        let early: Vec<Instant> = self.0.iter().filter(|(e, _)| *e == epoch).map(|(_, at)| *at).collect();
        self.0.clear();
        let earliest = early.iter().min().map(|at| entered.saturating_duration_since(*at)).unwrap_or_default();
        (early.len() as u32, earliest)
    }
}

/// Client: was nach dem `Enter` geschah
#[derive(Debug)]
pub struct ClientTrace {
    at: Instant,
    early: u32,
    earliest: Duration,
    moves: u32,
    first: Option<Duration>,
    last: Instant,
    max_gap: Duration,
    /// Größte Zeit vom Empfang bis eingespeist, und für wie viele Bewegungen sie bekannt ist
    max_inject: Duration,
    injected: u32,
}

impl ClientTrace {
    /// `Enter` mit `epoch`, empfangen um `at`
    pub fn start(epoch: u16, at: Instant, early_moves: &mut EarlyMoves) -> Self {
        let (early, earliest) = early_moves.take(epoch, at);
        Self {
            at,
            early,
            earliest,
            moves: 0,
            first: None,
            last: at,
            max_gap: Duration::ZERO,
            max_inject: Duration::ZERO,
            injected: 0,
        }
    }

    /// Beobachtet er noch? Nur dann lohnt es, das Einspeisen dieser Bewegung zu messen.
    pub fn watches(&self, received: Instant) -> bool {
        received.saturating_duration_since(self.at) <= WINDOW
    }

    /// Eine übernommene Bewegung, empfangen um `received`
    pub fn on_move(&mut self, received: Instant) {
        if !self.watches(received) {
            return;
        }
        self.moves += 1;
        self.first.get_or_insert(received.saturating_duration_since(self.at));
        self.max_gap = self.max_gap.max(received.saturating_duration_since(self.last));
        self.last = received;
    }

    /// Eine Bewegung aus dem Beobachtungsfenster ist eingespeist
    pub fn on_injected(&mut self, received: Instant, done: Instant) {
        self.injected += 1;
        self.max_inject = self.max_inject.max(done.saturating_duration_since(received));
    }

    /// Fertig, sobald das Fenster vorbei ist und das Einspeisen kurz nachkommen konnte
    pub fn is_due(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) > WINDOW + Duration::from_millis(50)
    }

    /// Sichtbar: Das `Enter` kam merklich nach den ersten Bewegungen (so lange stand der Cursor), eine
    /// Lücke beim Empfang oder langsames Einspeisen
    fn is_notable(&self) -> bool {
        (self.early > 0 && self.earliest >= Duration::from_millis(10))
            || self.max_gap >= NOTABLE
            || self.max_inject >= Duration::from_millis(20)
    }

    pub fn log(&self) {
        if self.is_notable() {
            info!("{}", self.describe());
        } else {
            debug!("{}", self.describe());
        }
    }

    /// Fürs Log, z. B. „Betreten: keine Bewegung vor dem Enter; in den ersten 300 ms 241 Bewegungen
    /// empfangen, die erste nach 1,0 ms, größte Lücke 4,2 ms, eingespeist höchstens 0,5 ms nach dem
    /// Empfang“
    pub fn describe(&self) -> String {
        let early = match self.early {
            0 => "keine Bewegung vor dem Enter".to_owned(),
            1 => format!("1 Bewegung kam {} ms vor dem Enter an und folgte ihm gleich", ms(self.earliest)),
            n => format!(
                "{n} Bewegungen kamen vor dem Enter an, die erste {} ms vorher; die neueste folgte ihm gleich",
                ms(self.earliest)
            ),
        };
        let received = match (self.moves, self.first) {
            (0, _) | (_, None) => "keine Bewegung empfangen".to_owned(),
            (n, Some(first)) => {
                let inject = if self.injected == 0 {
                    String::new()
                } else {
                    format!(", eingespeist höchstens {} ms nach dem Empfang", ms(self.max_inject))
                };
                format!("{} empfangen, die erste nach {} ms, größte Lücke {} ms{inject}", moves(n), ms(first), ms(self.max_gap))
            }
        };
        format!("Betreten: {early}; in den ersten {} ms {received}", WINDOW.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_smooth_switch_reads_as_such() {
        let t = Instant::now();
        let mut hits = EdgeHits::default();
        hits.hit(t);
        let mut trace = ServerTrace::start("mac".into(), t, &mut hits);
        for i in 1..=100 {
            trace.on_move(at(t, i), 12, -1);
        }
        trace.on_move(at(t, 400), 500, 0);
        assert!(trace.is_due(at(t, 301)));
        assert!(!trace.is_notable(), "nur zur Fehlersuche im Log");
        assert_eq!(
            trace.describe(),
            "Wechsel zu „mac“: Kante 1× berührt; in den ersten 300 ms 100 Bewegungen erfasst, die erste nach 1,0 ms, \
             größte Lücke 1,0 ms, größter Schritt 12 px"
        );
    }

    #[test]
    fn pushing_against_the_edge_and_gaps_show_up() {
        let t = Instant::now();
        let mut hits = EdgeHits::default();
        // Ein früherer Anlauf zählt nicht mit.
        hits.hit(t);
        for i in 0..40 {
            hits.hit(at(t, 1000 + i * 2));
        }
        let switched = at(t, 1078);
        let mut trace = ServerTrace::start("mac".into(), switched, &mut hits);
        trace.on_move(switched + Duration::from_millis(2), 900, 3);
        trace.on_move(switched + Duration::from_millis(120), 8, 0);
        trace.on_back(switched + Duration::from_millis(130));
        trace.on_move(switched + Duration::from_millis(140), 5, 0);
        assert_eq!(
            trace.describe(),
            "Wechsel zu „mac“: Kante 40× berührt, 78,0 ms lang; in den ersten 300 ms 2 Bewegungen erfasst, die erste \
             nach 2,0 ms, größte Lücke 118,0 ms, größter Schritt 900 px, nach 130,0 ms zurück auf dem Server"
        );
        assert!(trace.is_notable());
        // Ein neuer Wechsel ohne Kante am Server (Kürzel, von Client zu Client)
        let trace = ServerTrace::start("mac".into(), at(t, 5000), &mut hits);
        assert!(trace.describe().starts_with("Wechsel zu „mac“: ohne Kante am Server; in den ersten 300 ms keine"));
    }

    #[test]
    fn the_client_counts_what_came_before_the_enter() {
        let t = Instant::now();
        let mut early_moves = EarlyMoves::default();
        early_moves.add(6, t);
        early_moves.add(7, at(t, 10));
        early_moves.add(7, at(t, 12));
        let entered = at(t, 15);
        let mut trace = ClientTrace::start(7, entered, &mut early_moves);
        for i in 1..=50 {
            let received = entered + Duration::from_millis(i);
            assert!(trace.watches(received));
            trace.on_move(received);
            trace.on_injected(received, received + Duration::from_micros(400));
        }
        let late = entered + Duration::from_millis(250);
        trace.on_move(late);
        trace.on_injected(late, late + Duration::from_millis(30));
        assert!(!trace.watches(entered + Duration::from_millis(301)));
        assert!(!trace.is_due(entered + Duration::from_millis(340)));
        assert!(trace.is_due(entered + Duration::from_millis(351)));
        assert_eq!(
            trace.describe(),
            "Betreten: 2 Bewegungen kamen vor dem Enter an, die erste 5,0 ms vorher; die neueste folgte ihm gleich; \
             in den ersten 300 ms 51 Bewegungen empfangen, die erste nach 1,0 ms, größte Lücke 200,0 ms, eingespeist \
             höchstens 30,0 ms nach dem Empfang"
        );
        assert!(trace.is_notable());
        let quiet = ClientTrace::start(8, at(t, 1000), &mut early_moves);
        assert_eq!(quiet.describe(), "Betreten: keine Bewegung vor dem Enter; in den ersten 300 ms keine Bewegung empfangen");
        assert!(!quiet.is_notable());
    }
}
