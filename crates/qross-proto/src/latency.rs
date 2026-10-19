//! Verzögerung der Eingabekette (ab `LATENCY_SINCE`): Erfassung am Server bis eingespeist am Client.
//!
//! Auf dem Eingabeweg kommt dafür kein Byte dazu. Stichproben sind die Maus-Datagramme, deren `seq`
//! durch `MOUSE_SAMPLE_EVERY` teilbar ist, und jeder Tastendruck und Klick auf dem Input-Stream, erkannt
//! an seiner laufenden Nummer dort. So wissen beide Seiten ohne Absprache, welche Eingaben zählen. Der
//! Client meldet seine Zeiten mit `Latency::Report` auf dem Control-Stream zurück, der Server rechnet und
//! schickt das Ergebnis mit `Latency::Summary` zur Anzeige zurück.

use serde::{Deserialize, Serialize};

use crate::Input;

/// Jedes Maus-Datagramm mit `seq` als Vielfachem davon ist eine Stichprobe (bei 125 Hz etwa vier je
/// Sekunde, bei 1000 Hz etwa 30).
pub const MOUSE_SAMPLE_EVERY: u32 = 32;
/// Höchstens so viele Stichproben in einer Rückmeldung
pub const MAX_REPORT_SAMPLES: usize = 64;

/// Ist dieses Maus-Datagramm (`MouseMove` oder `MouseDelta`) eine Stichprobe?
pub const fn is_sampled_move(seq: u32) -> bool {
    seq.is_multiple_of(MOUSE_SAMPLE_EVERY)
}

/// Ist diese Nachricht auf dem Input-Stream eine Stichprobe? Jeder Druck einer Taste oder Maustaste,
/// auch als Zeichen oder Kürzel; Loslassen, Rad und Wechsel zählen nicht.
pub fn is_sampled_input(input: &Input) -> bool {
    matches!(
        input,
        Input::Key { down: true, .. } | Input::Button { down: true, .. } | Input::Text { .. } | Input::Shortcut { .. }
    )
}

/// Nachricht auf dem Control-Stream (`Control::Latency`)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Latency {
    /// Client → Server: Zeiten der Stichproben seit der letzten Rückmeldung
    Report(LatencyReport),
    /// Server → Client: das Ergebnis zur Anzeige, etwa jede Sekunde, solange sich etwas ändert
    Summary(LatencySummary),
}

/// Welche Eingabe eine Stichprobe war
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleId {
    /// Maus-Datagramm (`MouseMove` bzw. `MouseDelta`)
    Mouse { epoch: u16, seq: u32 },
    /// Die soundsovielte Nachricht auf dem Input-Stream, ab 1 gezählt
    Input { index: u64 },
}

/// Rückmeldung des Clients. Zeiten in Mikrosekunden seiner eigenen, monotonen Uhr; ihr Nullpunkt ist
/// beliebig (etwa der Beginn der Verbindung), denn den Versatz zur Uhr des Servers schätzt der Server.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LatencyReport {
    /// Uhr des Clients unmittelbar vor dem Absenden
    pub sent_us: u64,
    pub samples: Vec<LatencySample>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencySample {
    pub id: SampleId,
    /// So lange vor `sent_us` kam die Eingabe beim Client an
    pub age_us: u32,
    /// Vom Empfang bis das Einspeisen erledigt war
    pub client_us: u32,
}

/// Median und 95. Perzentil in Mikrosekunden
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spread {
    pub p50_us: u32,
    pub p95_us: u32,
}

/// Die Kette für eine Art von Eingaben (Maus bzw. Tasten), über die letzte Minute
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chain {
    /// Stichproben, aus denen `server` stammt
    pub samples: u32,
    /// Am Server: von der Erfassung bis zum Versand
    pub server: Spread,
    /// Stichproben mit Rückmeldung des Clients; aus ihnen stammen die folgenden Werte. 0 bei Clients,
    /// die nichts zurückmelden (älter als `LATENCY_SINCE`); dann fehlen sie.
    pub reported: u32,
    /// Im Netz: vom Versand bis zum Empfang beim Client
    pub network: Option<Spread>,
    /// Am Client: vom Empfang bis das Einspeisen erledigt war
    pub client: Option<Spread>,
    /// Alles zusammen, je Stichprobe addiert (deshalb nicht die Summe der Mediane)
    pub total: Option<Spread>,
}

/// Ergebnis für eine Verbindung
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LatencySummary {
    /// Mausbewegungen; `None`, solange keine Stichprobe vorliegt
    pub mouse: Option<Chain>,
    /// Tastendrücke und Klicks
    pub keys: Option<Chain>,
    /// So weit kann der Netzanteil höchstens danebenliegen: die halbe Umlaufzeit des Austauschs, aus
    /// dem der Uhrenversatz stammt, samt Zuschlag für sein Alter. `None` ohne Rückmeldungen.
    pub uncertainty_us: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyCode, MouseButton};

    #[test]
    fn both_sides_agree_on_samples() {
        let sampled: Vec<u32> = (0..100).filter(|&seq| is_sampled_move(seq)).collect();
        assert_eq!(sampled, vec![0, 32, 64, 96]);
        assert!(is_sampled_move(u32::MAX - 31), "auch nach dem Überlauf von seq jedes 32.");

        assert!(is_sampled_input(&Input::Key { key: KeyCode::A, down: true }));
        assert!(!is_sampled_input(&Input::Key { key: KeyCode::A, down: false }));
        assert!(is_sampled_input(&Input::Button { button: MouseButton::Left, down: true, x: 1, y: 2 }));
        assert!(!is_sampled_input(&Input::Button { button: MouseButton::Left, down: false, x: 1, y: 2 }));
        assert!(is_sampled_input(&Input::Text { key: KeyCode::A, text: "a".into() }));
        assert!(is_sampled_input(&Input::Shortcut { key: KeyCode::A, ch: 'a' }));
        assert!(!is_sampled_input(&Input::Wheel { dx: 0, dy: 120 }));
        assert!(!is_sampled_input(&Input::Enter { epoch: 1, x: 0, y: 0 }));
    }

    #[test]
    fn reports_are_small() {
        let sample = LatencySample { id: SampleId::Mouse { epoch: 3, seq: 4_000_000 }, age_us: 180_000, client_us: 450 };
        let report = LatencyReport { sent_us: 3_600_000_000, samples: vec![sample; 8] };
        let bytes = postcard::to_stdvec(&Latency::Report(report.clone())).unwrap();
        assert!(bytes.len() <= 8 + 8 * 12, "{} Bytes", bytes.len());
        assert_eq!(postcard::from_bytes::<Latency>(&bytes).unwrap(), Latency::Report(report));

        let spread = Spread { p50_us: 800, p95_us: 2400 };
        let chain = Chain { samples: 900, server: spread, reported: 880, network: Some(spread), client: None, total: Some(spread) };
        let summary = LatencySummary { mouse: Some(chain), keys: None, uncertainty_us: Some(90) };
        let bytes = postcard::to_stdvec(&Latency::Summary(summary)).unwrap();
        assert!(bytes.len() <= 40, "{} Bytes", bytes.len());
        assert_eq!(postcard::from_bytes::<Latency>(&bytes).unwrap(), Latency::Summary(summary));
    }
}
