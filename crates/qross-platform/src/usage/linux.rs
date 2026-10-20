//! Linux: dieser Prozess und alle, die er gestartet hat, samt deren Nachfahren (WebKitGTK mit Web- und
//! Netzwerkprozess, auch hinter bubblewrap), aus `/proc`.

use std::collections::HashMap;
use std::fs;
use std::sync::OnceLock;

use super::procfs::{self, Stat};
use super::Process;

pub(crate) fn processes() -> Vec<Process> {
    let all = all_stats();
    let own = std::process::id();
    let Some(&root) = all.get(&own) else { return Vec::new() };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, stat) in &all {
        if pid != stat.parent {
            children.entry(stat.parent).or_default().push(pid);
        }
    }
    let mut found = vec![(own, root)];
    // Ein Kind muss nach seinem Elternprozess gestartet sein; sonst trägt es nur eine neu vergebene Nummer.
    let mut queue = vec![(own, root.started)];
    while let Some((parent, started)) = queue.pop() {
        for &pid in children.get(&parent).into_iter().flatten() {
            let stat = all[&pid];
            if stat.started >= started {
                queue.push((pid, stat.started));
                found.push((pid, stat));
            }
        }
    }
    let ticks = clock_ticks();
    found
        .into_iter()
        .map(|(pid, stat)| Process {
            id: (pid, stat.started),
            cpu_ns: (u128::from(stat.cpu_ticks) * 1_000_000_000 / u128::from(ticks)) as u64,
            memory: memory(pid),
        })
        .collect()
}

/// `/proc/<pid>/stat` aller Prozesse, die sich lesen lassen
fn all_stats() -> HashMap<u32, Stat> {
    let Ok(dir) = fs::read_dir("/proc") else { return HashMap::new() };
    dir.flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = procfs::parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
            Some((pid, stat))
        })
        .collect()
}

/// Eigener Arbeitsspeicher: PSS (geteilte Seiten anteilig), sonst der residente Teil
fn memory(pid: u32) -> u64 {
    if let Some(bytes) = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok().and_then(|text| procfs::pss(&text)) {
        return bytes;
    }
    fs::read_to_string(format!("/proc/{pid}/statm"))
        .ok()
        .and_then(|text| procfs::resident_pages(&text))
        .map_or(0, |pages| pages * page_size())
}

/// Takte je Sekunde in `/proc/<pid>/stat`
fn clock_ticks() -> u64 {
    static TICKS: OnceLock<u64> = OnceLock::new();
    *TICKS.get_or_init(|| u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok().filter(|&t| t > 0).unwrap_or(100))
}

fn page_size() -> u64 {
    static PAGE: OnceLock<u64> = OnceLock::new();
    *PAGE.get_or_init(|| u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok().filter(|&p| p > 0).unwrap_or(4096))
}
