//! Wie viel Rechenzeit und Arbeitsspeicher Qross auf diesem Rechner braucht: der eigene Prozess und die
//! Web-Oberfläche der App (Windows: WebView2, macOS: die Dienste von WebKit, Linux: die Prozesse von WebKitGTK),
//! aufgeteilt nach Art der Prozesse ([`UsageKind`]). Programme, die Qross nur startet (etwa einen Browser für einen
//! Link oder den Dateimanager), zählen nicht mit.
//!
//! Gemessen wird nur auf Abruf ([`UsageMeter::sample`]); die Rechenzeit ergibt sich aus dem Unterschied
//! zur vorigen Messung.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use qross_proto::UsageKind;

#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
mod kinds;
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
    pub kind: UsageKind,
    /// Rechenzeit seit dem Start, Benutzer- und Systemanteil, in Nanosekunden
    pub cpu_ns: u64,
    /// Eigener Arbeitsspeicher in Bytes: unter Windows der private Arbeitssatz (wie im Task-Manager), unter
    /// macOS der physische Speicherbedarf (wie in der Aktivitätsanzeige), unter Linux PSS (geteilte Seiten
    /// anteilig)
    pub memory: u64,
}

/// Rechenzeit und Arbeitsspeicher von Qross auf diesem Rechner
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    /// Rechenzeit seit der vorigen Messung in Prozent eines Kerns (über 100, wenn mehrere Kerne zugleich
    /// rechnen). `None` bei der ersten Messung und wenn die vorige keine [`MIN_INTERVAL`] zurückliegt.
    pub cpu_percent: Option<f64>,
    /// Arbeitsspeicher in Bytes, den Qross für sich belegt
    pub memory: u64,
    /// Zahl der Prozesse: Qross selbst und die seiner Web-Oberfläche
    pub processes: u32,
    /// Dieselben Werte je Art der Prozesse, nur Arten, die vorkommen, in der Reihenfolge von [`UsageKind`]
    /// (Qross selbst zuerst). Zusammen ergeben sie die Summe.
    pub parts: Vec<Part>,
}

/// Anteil einer Art von Prozessen an [`Usage`], in denselben Einheiten
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub kind: UsageKind,
    pub cpu_percent: Option<f64>,
    pub memory: u64,
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
        let elapsed = self.last.as_ref().map(|(at, _)| now.saturating_duration_since(*at));
        // Zu kurz für einen ruhigen Wert: ohne Rechenzeit, und die vorige Messung bleibt der Bezug
        let too_soon = elapsed.is_some_and(|elapsed| elapsed < MIN_INTERVAL);
        let reference = self.last.as_ref().map(|(_, before)| before).zip(elapsed).filter(|_| !too_soon);
        // Je Art: Arbeitsspeicher, Zahl der Prozesse und Rechenzeit seit der vorigen Messung in Nanosekunden
        let mut kinds: BTreeMap<UsageKind, (u64, u32, u64)> = BTreeMap::new();
        for process in processes {
            let (memory, count, used) = kinds.entry(process.kind).or_default();
            *memory += process.memory;
            *count += 1;
            if let Some((before, _)) = reference {
                // Ein neuer Prozess zählt mit seiner ganzen Rechenzeit; was ein beendeter zuletzt rechnete, fehlt.
                *used += process.cpu_ns.saturating_sub(before.get(&process.id).copied().unwrap_or(0));
            }
        }
        let percent = |used: u64| reference.map(|(_, elapsed)| used as f64 / elapsed.as_nanos() as f64 * 100.0);
        let usage = Usage {
            cpu_percent: percent(kinds.values().map(|(_, _, used)| used).sum()),
            memory: processes.iter().map(|p| p.memory).sum(),
            processes: processes.len() as u32,
            parts: kinds
                .into_iter()
                .map(|(kind, (memory, processes, used))| Part { kind, cpu_percent: percent(used), memory, processes })
                .collect(),
        };
        if !too_soon {
            self.last = Some((now, processes.iter().map(|p| (p.id, p.cpu_ns)).collect()));
        }
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    /// Ein Prozess von Qross selbst
    fn process(pid: u32, started: u64, cpu_ms: u64, memory: u64) -> Process {
        Process { id: (pid, started), kind: UsageKind::Core, cpu_ns: cpu_ms * 1_000_000, memory }
    }

    /// Ein Prozess der Web-Oberfläche
    fn of(kind: UsageKind, process: Process) -> Process {
        Process { kind, ..process }
    }

    #[test]
    fn cpu_counts_from_the_previous_sample() {
        let mut meter = UsageMeter::new();
        let start = Instant::now();
        let first = meter.update(start, &[process(10, 1, 900, 7 * MB), of(UsageKind::Page, process(11, 2, 5_000, 120 * MB))]);
        assert_eq!((first.cpu_percent, first.memory, first.processes), (None, 127 * MB, 2), "the first sample has no CPU time");
        assert!(first.parts.iter().all(|part| part.cpu_percent.is_none()), "{first:?}");

        // 2 s später: 300 ms im eigenen Prozess, 100 ms in der Web-Oberfläche
        let later = [process(10, 1, 1_200, 8 * MB), of(UsageKind::Page, process(11, 2, 5_100, 120 * MB))];
        let second = meter.update(start + Duration::from_secs(2), &later);
        assert_eq!(second.cpu_percent, Some(20.0));
        assert_eq!((second.memory, second.processes), (128 * MB, 2));
        let core = Part { kind: UsageKind::Core, cpu_percent: Some(15.0), memory: 8 * MB, processes: 1 };
        let page = Part { kind: UsageKind::Page, cpu_percent: Some(5.0), memory: 120 * MB, processes: 1 };
        assert_eq!(second.parts, [core, page]);
    }

    #[test]
    fn parts_add_up_per_kind_in_a_fixed_order() {
        let mut meter = UsageMeter::new();
        let start = Instant::now();
        let at_first = [
            of(UsageKind::Other, process(30, 3, 0, 2 * MB)),
            of(UsageKind::Graphics, process(21, 2, 1_000, 28 * MB)),
            process(10, 1, 0, 8 * MB),
            of(UsageKind::Other, process(31, 3, 100, 3 * MB)),
            of(UsageKind::Browser, process(20, 2, 0, 23 * MB)),
        ];
        meter.update(start, &at_first);
        // Eine Sekunde später: Die Grafik rechnete 250 ms, beide sonstigen zusammen 50 ms; ein neuer Prozess der
        // Seite kam hinzu (200 ms seit seinem Start).
        let later = [
            of(UsageKind::Other, process(30, 3, 30, 2 * MB)),
            of(UsageKind::Graphics, process(21, 2, 1_250, 28 * MB)),
            process(10, 1, 0, 8 * MB),
            of(UsageKind::Page, process(22, 9, 200, 24 * MB)),
            of(UsageKind::Other, process(31, 3, 120, 3 * MB)),
            of(UsageKind::Browser, process(20, 2, 0, 23 * MB)),
        ];
        let usage = meter.update(start + Duration::from_secs(1), &later);
        let summary: Vec<_> = usage.parts.iter().map(|p| (p.kind, p.cpu_percent, p.memory / MB, p.processes)).collect();
        assert_eq!(
            summary,
            [
                (UsageKind::Core, Some(0.0), 8, 1),
                (UsageKind::Page, Some(20.0), 24, 1),
                (UsageKind::Graphics, Some(25.0), 28, 1),
                (UsageKind::Browser, Some(0.0), 23, 1),
                (UsageKind::Other, Some(5.0), 5, 2),
            ]
        );
        assert_eq!((usage.cpu_percent, usage.memory, usage.processes), (Some(50.0), 88 * MB, 6));
        let parts_memory: u64 = usage.parts.iter().map(|p| p.memory).sum();
        let parts_processes: u32 = usage.parts.iter().map(|p| p.processes).sum();
        assert_eq!((parts_memory, parts_processes), (usage.memory, usage.processes), "the parts add up to the total");
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
        let core = Part { kind: UsageKind::Core, cpu_percent: None, memory: 2 * MB, processes: 1 };
        assert_eq!(soon, Usage { cpu_percent: None, memory: 2 * MB, processes: 1, parts: vec![core] }, "too short for a steady value");
        // Gemessen wird dann ab der ersten Messung: 250 ms in 500 ms
        let later = meter.update(start + Duration::from_millis(500), &[process(10, 1, 250, 2 * MB)]);
        assert_eq!(later.cpu_percent, Some(50.0));
    }

    /// Echte Messung: Dieser Testprozess rechnet und zählt als Qross selbst. Ein Kind, das er startet (wie Qross
    /// einen Browser für einen Link), gehört nicht dazu.
    #[test]
    fn measures_this_process_but_not_what_it_starts() {
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
        let own = std::process::id();
        assert!(found.iter().any(|p| p.id.0 == own && p.kind == UsageKind::Core), "this process missing in {found:?}");
        assert!(!found.iter().any(|p| p.id.0 == child.id()), "child {} counted: {found:?}", child.id());
        let core = Part { kind: UsageKind::Core, cpu_percent: second.cpu_percent, memory: second.memory, processes: 1 };
        assert_eq!((second.processes, second.parts.as_slice()), (1, [core].as_slice()), "{second:?}");
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
