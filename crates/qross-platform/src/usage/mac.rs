//! macOS: dieser Prozess, was er selbst gestartet hat, und die Dienste, für die er verantwortlich ist. Die
//! WebKit-Prozesse der Web-Oberfläche startet launchd; sie gehören aber zur App, wie in der
//! Aktivitätsanzeige (`responsibility_get_pid_responsible_for_pid`, gibt es das nicht: ohne sie).

use std::ffi::{c_int, c_void};
use std::mem::size_of_val;
use std::sync::OnceLock;

use libc::pid_t;

use super::Process;

pub(crate) fn processes() -> Vec<Process> {
    let own = std::process::id() as pid_t;
    let Some(root) = measure(own) else { return Vec::new() };
    let mut found = vec![root];
    // Selbst gestartete Prozesse und deren Nachfahren; ein Kind muss nach seinem Elternprozess gestartet
    // sein, sonst trägt es nur eine neu vergebene Nummer.
    let mut queue = vec![(own, root.id.1)];
    while let Some((parent, started)) = queue.pop() {
        for pid in children(parent) {
            let Some(child) = measure(pid).filter(|child| child.id.1 >= started) else { continue };
            queue.push((pid, child.id.1));
            found.push(child);
        }
    }
    // Dienste, die launchd für diese App gestartet hat. Nur, wenn sie für sich selbst verantwortlich ist
    // (aus dem Finder, dem Dock oder als Anmeldeobjekt gestartet); aus dem Terminal heraus wäre es das
    // Terminal, und dessen Dienste gehören nicht hierher.
    if let Some(responsible) = responsible_for() {
        for pid in all_pids() {
            if found.iter().any(|p| p.id.0 == pid as u32) || unsafe { responsible(pid) } != own {
                continue;
            }
            found.extend(measure(pid));
        }
    }
    found
}

/// Rechenzeit und physischer Speicherbedarf eines Prozesses (wie „Speicher“ in der Aktivitätsanzeige)
fn measure(pid: pid_t) -> Option<Process> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&raw mut info).cast::<libc::rusage_info_t>()) };
    if result != 0 {
        return None;
    }
    // Die Zeiten zählen in Einheiten der Mach-Uhr, auf Apple Silicon nicht in Nanosekunden.
    let (numer, denom) = timebase();
    let ticks = info.ri_user_time.saturating_add(info.ri_system_time);
    let cpu_ns = (u128::from(ticks) * u128::from(numer) / u128::from(denom)) as u64;
    Some(Process { id: (pid as u32, info.ri_proc_start_abstime), cpu_ns, memory: info.ri_phys_footprint })
}

/// Prozesse, die `parent` gestartet hat
fn children(parent: pid_t) -> Vec<pid_t> {
    let mut pids = vec![0 as pid_t; 1024];
    let count = unsafe { libc::proc_listchildpids(parent, pids.as_mut_ptr().cast::<c_void>(), buffer_size(&pids)) };
    pids.truncate(usize::try_from(count).unwrap_or(0));
    pids.retain(|&pid| pid > 0);
    pids
}

/// Alle Prozesse des Systems
fn all_pids() -> Vec<pid_t> {
    let estimate = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(estimate) = usize::try_from(estimate) else { return Vec::new() };
    let mut pids = vec![0 as pid_t; estimate + 64];
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), buffer_size(&pids)) };
    pids.truncate(usize::try_from(count).unwrap_or(0));
    pids.retain(|&pid| pid > 0);
    pids
}

fn buffer_size(pids: &[pid_t]) -> c_int {
    c_int::try_from(size_of_val(pids)).unwrap_or(c_int::MAX)
}

type Responsible = unsafe extern "C" fn(pid_t) -> pid_t;

/// Wer für einen Prozess verantwortlich ist (nicht öffentlich, aber seit macOS 10.14 in libSystem)
fn responsible_for() -> Option<Responsible> {
    static FUNCTION: OnceLock<Option<Responsible>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr()) };
        (!symbol.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Responsible>(symbol) })
    })
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> c_int;
}

/// Umrechnung der Mach-Uhr in Nanosekunden (Zähler, Nenner)
fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = Timebase { numer: 1, denom: 1 };
        let ok = unsafe { mach_timebase_info(&mut info) } == 0;
        if ok && info.numer > 0 && info.denom > 0 {
            (info.numer, info.denom)
        } else {
            (1, 1)
        }
    })
}
