//! Auswertung der Messungen.

use hdrhistogram::Histogram;

use crate::workload::Inner;

/// Latenz-Kennzahlen in Millisekunden.
#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub n: u64,
    pub p50: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
}

impl Summary {
    fn from_ns(values: impl IntoIterator<Item = u64>) -> Self {
        let mut h = Histogram::<u64>::new_with_bounds(1, 120_000_000, 3).unwrap();
        for ns in values {
            h.saturating_record((ns / 1_000).max(1));
        }
        if h.is_empty() {
            return Self::default();
        }
        let ms = |us: u64| us as f64 / 1_000.0;
        Self {
            n: h.len(),
            p50: ms(h.value_at_quantile(0.50)),
            p99: ms(h.value_at_quantile(0.99)),
            p999: ms(h.value_at_quantile(0.999)),
            max: ms(h.max()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    /// Wie lange es dauert, bis der Cursor beim Empfänger die gesendete Position
    /// (oder eine neuere) erreicht hat. Verlorene Datagramme zählen also nicht als
    /// unendlich, sondern bis die nächste Bewegung ankommt.
    pub cursor: Summary,
    pub mouse_sent: usize,
    pub mouse_lost: usize,
    /// Cursor-Updates, die länger als 50 ms gebraucht haben (spürbares Ruckeln)
    pub stalls_50ms: usize,
    pub key: Summary,
    pub keys_missing: usize,
    pub clip: Summary,
    pub clips_missing: usize,
}

pub fn evaluate(data: Inner) -> Report {
    let sent = &data.mouse_sent;
    let mut arrival: Vec<Option<u64>> = vec![None; sent.len()];
    for &(seq, t) in &data.mouse_arrivals {
        if let Some(slot) = arrival.get_mut(seq as usize) {
            *slot = Some(slot.map_or(t, |old| old.min(t)));
        }
    }

    // Rückwärts laufen: früheste Ankunft dieser oder einer neueren Bewegung.
    let mut cursor_latencies = Vec::with_capacity(sent.len());
    let mut earliest_newer: Option<u64> = None;
    for i in (0..sent.len()).rev() {
        if let Some(a) = arrival[i] {
            earliest_newer = Some(earliest_newer.map_or(a, |e| e.min(a)));
        }
        if let Some(e) = earliest_newer {
            cursor_latencies.push(e.saturating_sub(sent[i]));
        }
    }

    Report {
        stalls_50ms: cursor_latencies.iter().filter(|&&ns| ns > 50_000_000).count(),
        cursor: Summary::from_ns(cursor_latencies),
        mouse_sent: sent.len(),
        mouse_lost: arrival.iter().filter(|a| a.is_none()).count(),
        keys_missing: data.keys_sent.saturating_sub(data.key_latencies.len()),
        key: Summary::from_ns(data.key_latencies),
        clips_missing: data.clips_sent.saturating_sub(data.clip_latencies.len()),
        clip: Summary::from_ns(data.clip_latencies),
    }
}
