//! Erzeugt KVM-typischen Eingabeverkehr in exakten Zeitabständen und protokolliert,
//! was wann gesendet bzw. empfangen wurde.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::wire::{Kind, Msg};

/// Gemeinsame monotone Uhr für Sender und Empfänger (beide laufen im selben Prozess).
#[derive(Clone, Copy)]
pub struct Clock(Instant);

impl Clock {
    pub fn new() -> Self {
        Self(Instant::now())
    }

    pub fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

#[derive(Clone, Debug)]
pub struct Workload {
    pub duration: Duration,
    pub mouse_hz: u32,
    pub key_hz: u32,
    /// 0 = kein Clipboard-Verkehr
    pub clipboard_bytes: usize,
    pub clipboard_interval: Duration,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub kind: Kind,
    pub seq: u32,
    pub ts: u64,
    pub clip_len: usize,
}

#[derive(Default)]
pub struct Recorder {
    inner: Mutex<Inner>,
}

#[derive(Default)]
pub struct Inner {
    /// Sendezeitpunkt je Maus-Sequenznummer
    pub mouse_sent: Vec<u64>,
    /// (Sequenznummer, Ankunftszeit)
    pub mouse_arrivals: Vec<(u32, u64)>,
    pub keys_sent: usize,
    pub key_latencies: Vec<u64>,
    pub clips_sent: usize,
    pub clip_latencies: Vec<u64>,
}

impl Recorder {
    fn mouse_sent(&self, ts: u64) {
        self.inner.lock().unwrap().mouse_sent.push(ts);
    }

    fn key_sent(&self) {
        self.inner.lock().unwrap().keys_sent += 1;
    }

    fn clip_sent(&self) {
        self.inner.lock().unwrap().clips_sent += 1;
    }

    pub fn on_message(&self, msg: &Msg, now: u64) {
        let mut inner = self.inner.lock().unwrap();
        match msg.kind {
            Kind::Mouse => inner.mouse_arrivals.push((msg.seq, now)),
            Kind::Key => inner.key_latencies.push(now.saturating_sub(msg.ts)),
            Kind::Clip => {
                // Erstes Payload-Byte markiert den letzten Chunk einer Übertragung.
                if msg.payload.first() == Some(&1) {
                    inner.clip_latencies.push(now.saturating_sub(msg.ts));
                }
            }
        }
    }

    pub fn take(&self) -> Inner {
        std::mem::take(&mut *self.inner.lock().unwrap())
    }
}

/// Startet einen OS-Thread, der Ereignisse taktgenau erzeugt (Tokio-Timer wären unter
/// Windows zu grob). Der Kanal wird geschlossen, sobald die Laufzeit vorbei ist.
pub fn spawn_generator(
    w: Workload,
    clock: Clock,
    tx: UnboundedSender<Event>,
    rec: Arc<Recorder>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mouse_period = 1_000_000_000 / w.mouse_hz.max(1) as u64;
        let key_period = 1_000_000_000 / w.key_hz.max(1) as u64;
        let clip_period = w.clipboard_interval.as_nanos() as u64;

        let t0 = clock.now_ns();
        let end = t0 + w.duration.as_nanos() as u64;
        let mut next_mouse = t0;
        let mut next_key = t0 + key_period / 2;
        let mut next_clip = if w.clipboard_bytes > 0 { t0 + clip_period } else { u64::MAX };
        let (mut mouse_seq, mut key_seq, mut clip_seq) = (0u32, 0u32, 0u32);

        loop {
            let now = clock.now_ns();
            if now >= end {
                break;
            }
            let next = next_mouse.min(next_key).min(next_clip);
            if now < next {
                let wait = next - now;
                if wait > 1_500_000 {
                    thread::sleep(Duration::from_nanos(wait - 1_000_000));
                } else {
                    std::hint::spin_loop();
                }
                continue;
            }

            if now >= next_mouse {
                rec.mouse_sent(now);
                let _ = tx.send(Event { kind: Kind::Mouse, seq: mouse_seq, ts: now, clip_len: 0 });
                mouse_seq += 1;
                next_mouse = advance(next_mouse, mouse_period, now);
            }
            if now >= next_key {
                rec.key_sent();
                let _ = tx.send(Event { kind: Kind::Key, seq: key_seq, ts: now, clip_len: 0 });
                key_seq += 1;
                next_key = advance(next_key, key_period, now);
            }
            if now >= next_clip {
                rec.clip_sent();
                let _ = tx.send(Event { kind: Kind::Clip, seq: clip_seq, ts: now, clip_len: w.clipboard_bytes });
                clip_seq += 1;
                next_clip = advance(next_clip, clip_period, now);
            }
        }
    })
}

/// Nächster Takt; nach einem verpassten Takt (z. B. Scheduler-Hänger) keine Burst-Nachholung.
fn advance(prev: u64, period: u64, now: u64) -> u64 {
    let next = prev + period;
    if next < now {
        now + period
    } else {
        next
    }
}
