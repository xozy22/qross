//! Wie viel Rechenzeit und Arbeitsspeicher Qross auf diesem Rechner braucht: der eigene Prozess und alle,
//! die zu ihm gehören, vor allem die Web-Oberfläche der App (Windows: WebView2, macOS: die WebKit-Dienste,
//! Linux: die Prozesse von WebKitGTK).
//!
//! Gemessen wird nur auf Abruf ([`UsageMeter::sample`]); die Rechenzeit ergibt sich aus dem Unterschied
//! zur vorigen Messung.

use std::collections::HashMap;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(any(target_os = "linux", test))]
mod procfs;
#[cfg(windows)]
mod win;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use mac as imp;
#[cfg(windows)]
use win as imp;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub(crate) fn processes() -> Vec<super::Process> {
        Vec::new()
    }
}

/// Mindestabstand zweier Messungen für die Rechenzeit; bei kürzerem bleibt die vorige der Bezug.
pub const MIN_INTERVAL: Duration = Duration::from_millis(200);

/// Kann Qross das auf diesem System messen?
pub const fn supported() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

/// Ein Prozess von Qross bei einer Messung
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Process {
    /// Prozessnummer und Startzeit (in Einheiten des Systems): zusammen eindeutig, auch wenn das System
    /// die Nummer eines beendeten Prozesses neu vergibt
    pub id: (u32, u64),
    /// Rechenzeit seit dem Start, Benutzer- und Systemanteil, in Nanosekunden
    pub cpu_ns: u64,
    /// Eigener Arbeitsspeicher in Bytes: unter Windows der private Arbeitssatz (wie im Task-Manager), unter
    /// macOS der physische Speicherbedarf (wie in der Aktivitätsanzeige), unter Linux PSS (geteilte Seiten
    /// anteilig)
    pub memory: u64,
}

/// Rechenzeit und Arbeitsspeicher von Qross auf diesem Rechner
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    /// Rechenzeit seit der vorigen Messung in Prozent eines Kerns (über 100, wenn mehrere Kerne zugleich
    /// rechnen). `None` bei der ersten Messung und wenn die vorige keine [`MIN_INTERVAL`] zurückliegt.
    pub cpu_percent: Option<f64>,
    /// Arbeitsspeicher in Bytes, den Qross für sich belegt
    pub memory: u64,
    /// Zahl der Prozesse: Qross selbst und seine Helfer (etwa die Web-Oberfläche)
    pub processes: u32,
}

/// Rechenzeit in Nanosekunden je Prozess (Nummer und Startzeit)
type CpuTimes = HashMap<(u32, u64), u64>;

/// Misst wiederholt; die Rechenzeit gilt jeweils seit der vorigen Messung.
#[derive(Debug, Default)]
pub struct UsageMeter {
    /// Zeitpunkt der vorigen Messung und die Rechenzeit jedes Prozesses damals
    last: Option<(Instant, CpuTimes)>,
}

impl UsageMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Misst jetzt. Sieht dafür die Prozesse des Systems durch und braucht einige Millisekunden, deshalb
    /// nicht in einer Schleife aufrufen, die Eingaben weitergibt.
    pub fn sample(&mut self) -> Usage {
        self.sample_at(Instant::now())
    }

    /// Wie [`UsageMeter::sample`], mit dem Zeitpunkt einer anderen Uhr (etwa einer, die Tests anhalten)
    pub fn sample_at(&mut self, now: Instant) -> Usage {
        let processes = imp::processes();
        self.update(now, &processes)
    }

    /// Zeitpunkt der vorigen Messung, auf die sich die nächste bezieht (`None`: noch keine)
    pub fn last_sample(&self) -> Option<Instant> {
        self.last.as_ref().map(|(at, _)| *at)
    }

    fn update(&mut self, now: Instant, processes: &[Process]) -> Usage {
        let memory = processes.iter().map(|p| p.memory).sum();
        let count = processes.len() as u32;
        let elapsed = self.last.as_ref().map(|(at, _)| now.saturating_duration_since(*at));
        if elapsed.is_some_and(|elapsed| elapsed < MIN_INTERVAL) {
            return Usage { cpu_percent: None, memory, processes: count };
        }
        let cpu_percent = self.last.as_ref().zip(elapsed).map(|((_, before), elapsed)| {
            // Ein neuer Prozess zählt mit seiner ganzen Rechenzeit; was ein beendeter zuletzt rechnete, fehlt.
            let used: u64 =
                processes.iter().map(|p| p.cpu_ns.saturating_sub(before.get(&p.id).copied().unwrap_or(0))).sum();
            used as f64 / elapsed.as_nanos() as f64 * 100.0
        });
        self.last = Some((now, processes.iter().map(|p| (p.id, p.cpu_ns)).collect()));
        Usage { cpu_percent, memory, processes: count }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    fn process(pid: u32, started: u64, cpu_ms: u64, memory: u64) -> Process {
        Process { id: (pid, started), cpu_ns: cpu_ms * 1_000_000, memory }
    }

    #[test]
    fn cpu_counts_from_the_previous_sample() {
        let mut meter = UsageMeter::new();
        let start = Instant::now();
        let first = meter.update(start, &[process(10, 1, 900, 7 * MB), process(11, 2, 5_000, 120 * MB)]);
        assert_eq!(first, Usage { cpu_percent: None, memory: 127 * MB, processes: 2 }, "the first sample has no CPU time");

        // 2 s später: 300 ms im eigenen Prozess, 100 ms in der Web-Oberfläche
        let second = meter.update(start + Duration::from_secs(2), &[process(10, 1, 1_200, 8 * MB), process(11, 2, 5_100, 120 * MB)]);
        assert_eq!(second.cpu_percent, Some(20.0));
        assert_eq!((second.memory, second.processes), (128 * MB, 2));
    }

    #[test]
    fn new_processes_count_fully_and_ended_ones_drop_out() {
        let mut meter = UsageMeter::new();
        let start = Instant::now();
        meter.update(start, &[process(10, 1, 1_000, MB), process(11, 2, 4_000, MB)]);
        // 11 ist beendet, 12 neu (500 ms gerechnet), 13 trägt die Nummer eines beendeten, ist aber ein anderer
        // Prozess (spätere Startzeit) und zählt deshalb ganz.
        let later = [process(10, 1, 1_100, MB), process(12, 5, 500, MB), process(11, 9, 400, MB)];
        let usage = meter.update(start + Duration::from_secs(1), &later);
        assert_eq!(usage.cpu_percent, Some(100.0));
        assert_eq!(usage.processes, 3);
    }

    #[test]
    fn a_too_short_pause_keeps_the_previous_sample_as_reference() {
        let mut meter = UsageMeter::new();
        let start = Instant::now();
        meter.update(start, &[process(10, 1, 0, MB)]);
        let soon = meter.update(start + Duration::from_millis(50), &[process(10, 1, 40, 2 * MB)]);
        assert_eq!(soon, Usage { cpu_percent: None, memory: 2 * MB, processes: 1 }, "too short for a steady value");
        // Gemessen wird dann ab der ersten Messung: 250 ms in 500 ms
        let later = meter.update(start + Duration::from_millis(500), &[process(10, 1, 250, 2 * MB)]);
        assert_eq!(later.cpu_percent, Some(50.0));
    }

    /// Echte Messung: dieser Testprozess rechnet, und ein Kind, das er startet, zählt mit.
    #[test]
    fn measures_this_process_and_what_it_started() {
        if !supported() {
            return;
        }
        let mut meter = UsageMeter::new();
        let first = meter.sample();
        assert!(first.cpu_percent.is_none() && first.processes >= 1 && first.memory > 0, "{first:?}");
        assert!(meter.last_sample().is_some());

        let mut child = sleeper();
        let busy_until = Instant::now() + Duration::from_millis(400);
        let mut x = 1u64;
        while Instant::now() < busy_until {
            x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
        }
        let second = meter.sample();
        let found = imp::processes();
        let _ = child.kill();
        let _ = child.wait();

        let cpu = second.cpu_percent.expect("the second sample has CPU time");
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        // 400 ms Rechnen in etwa 400 ms; auch ein ausgelasteter Testrechner lässt davon mehr als ein Zehntel
        // übrig. Nach oben begrenzt die Zahl der Kerne (ein falscher Zeitmaßstab fiele hier auf).
        assert!(cpu > 10.0 && cpu <= 100.0 * cores + 50.0, "{second:?}");
        assert!(second.processes >= 2, "the child counts too: {second:?}");
        assert!(found.iter().any(|p| p.id.0 == child.id()), "child {} missing in {found:?}", child.id());
        assert!(found.iter().any(|p| p.id.0 == std::process::id()), "this process missing in {found:?}");
    }

    /// Ein Kindprozess, der ein paar Sekunden schläft
    fn sleeper() -> std::process::Child {
        use std::process::{Command, Stdio};
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("ping");
            command.args(["-n", "4", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("3");
            command
        };
        command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("start a child process")
    }
}
