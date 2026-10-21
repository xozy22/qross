//! macOS: dieser Prozess und die Dienste von WebKit für seine Web-Oberfläche (Seite, Netz, Grafik). Die startet
//! launchd; sie gehören aber zur App, wie in der Aktivitätsanzeige (`responsibility_get_pid_responsible_for_pid`,
//! gibt es das nicht: ohne sie). Was Qross selbst startet (etwa `open` für einen Link), zählt nicht mit.

use std::ffi::{c_int, c_void};
use std::mem::size_of_val;
use std::sync::OnceLock;

use libc::pid_t;
use qross_proto::UsageKind;

use super::kinds;
use super::Process;

pub(crate) fn processes() -> Vec<Process> {
    let own = std::process::id() as pid_t;
    let Some(root) = measure(own, UsageKind::Core) else { return Vec::new() };
    let mut found = vec![root];
    // Dienste von WebKit, die launchd für diese App gestartet hat. Nur, wenn sie für sich selbst verantwortlich
    // ist (aus dem Finder, dem Dock oder als Anmeldeobjekt gestartet); aus dem Terminal heraus wäre es das
    // Terminal, und dessen Dienste gehören nicht hierher.
    if let Some(responsible) = responsible_for() {
        for pid in all_pids() {
            if pid == own || unsafe { responsible(pid) } != own {
                continue;
            }
            if let Some(kind) = program(pid).as_deref().and_then(kinds::webkit_service_kind) {
                found.extend(measure(pid, kind));
            }
        }
    }
    found
}

/// Rechenzeit und physischer Speicherbedarf eines Prozesses (wie „Speicher“ in der Aktivitätsanzeige)
fn measure(pid: pid_t, kind: UsageKind) -> Option<Process> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&raw mut info).cast::<libc::rusage_info_t>()) };
    if result != 0 {
        return None;
    }
    // Die Zeiten zählen in Einheiten der Mach-Uhr, auf Apple Silicon nicht in Nanosekunden.
    let (numer, denom) = timebase();
    let ticks = info.ri_user_time.saturating_add(info.ri_system_time);
    let cpu_ns = (u128::from(ticks) * u128::from(numer) / u128::from(denom)) as u64;
    Some(Process { id: (pid as u32, info.ri_proc_start_abstime), kind, cpu_ns, memory: info.ri_phys_footprint })
}

/// Name des Programms eines Prozesses: der letzte Teil seines Pfads (etwa `com.apple.WebKit.WebContent`)
fn program(pid: pid_t) -> Option<String> {
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast::<c_void>(), path.len() as u32) };
    path.truncate(usize::try_from(len).ok().filter(|&len| len > 0)?);
    let path = String::from_utf8_lossy(&path);
    Some(path.rsplit('/').next().unwrap_or_default().to_owned())
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
