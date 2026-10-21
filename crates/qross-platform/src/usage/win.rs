//! Windows: dieser Prozess und WebView2, die Web-Oberfläche: der Browser-Prozess, den Qross startet, und dessen
//! Hilfsprozesse für Seite, Grafik, Netz und Speicher. Andere Programme, die Qross startet (etwa den Browser für
//! einen Link oder den Explorer für einen Ordner), zählen nicht mit, auch nicht deren Nachfahren.

use std::collections::HashMap;
use std::mem::size_of;

use qross_proto::UsageKind;
use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

use super::kinds;
use super::Process;

pub(crate) fn processes() -> Vec<Process> {
    let own = std::process::id();
    let Some(root) = measure(own, |_| UsageKind::Core) else { return Vec::new() };
    let children = webview2_by_parent();
    let mut found = vec![root];
    // Weiter geht es nur über Prozesse von WebView2. Ein Kind muss nach seinem Elternprozess gestartet sein;
    // sonst trägt es nur die Nummer eines beendeten Elternprozesses, die Windows inzwischen an diesen vergeben hat.
    let mut queue = vec![(own, root.id.1)];
    while let Some((parent, started)) = queue.pop() {
        // Ohne lesbare Kommandozeile: Das Kind von Qross ist der Browser-Prozess, dessen Kinder sind Helfer.
        let fallback = if parent == own { UsageKind::Browser } else { UsageKind::Other };
        for &pid in children.get(&parent).into_iter().flatten() {
            let kind = |handle| command_line(handle).map_or(fallback, |line| kinds::webview2_kind(&line));
            let Some(child) = measure(pid, kind).filter(|child| child.id.1 >= started) else { continue };
            queue.push((pid, child.id.1));
            found.push(child);
        }
    }
    found
}

/// Die Prozesse von WebView2 (`msedgewebview2.exe`) nach ihrem Elternprozess
fn webview2_by_parent() -> HashMap<u32, Vec<u32>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return children;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let name = &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len())];
            if entry.th32ProcessID != 0
                && entry.th32ProcessID != entry.th32ParentProcessID
                && kinds::is_webview2(&String::from_utf16_lossy(name))
            {
                children.entry(entry.th32ParentProcessID).or_default().push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    children
}

/// Rechenzeit und privater Arbeitssatz eines Prozesses, seine Art bestimmt `kind` (mit dem offenen Prozess);
/// `None`, wenn er sich nicht (mehr) öffnen lässt
fn measure(pid: u32, kind: impl FnOnce(HANDLE) -> UsageKind) -> Option<Process> {
    unsafe {
        let mut handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        }
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        let mut counters: PROCESS_MEMORY_COUNTERS_EX2 = std::mem::zeroed();
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        let memory = K32GetProcessMemoryInfo(handle, (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), counters.cb) != 0;
        let kind = if times { Some(kind(handle)) } else { None };
        CloseHandle(handle);
        let kind = kind?;
        // Wie der Task-Manager: der private Arbeitssatz. Wo Windows ihn nicht kennt, die privat
        // zugesagten Seiten.
        let private = match (memory, counters.PrivateWorkingSetSize) {
            (false, _) => 0,
            (true, 0) => counters.PrivateUsage,
            (true, working_set) => working_set,
        };
        Some(Process {
            id: (pid, ticks(created)),
            kind,
            // FILETIME zählt in 100 ns
            cpu_ns: ticks(kernel).saturating_add(ticks(user)).saturating_mul(100),
            memory: private as u64,
        })
    }
}

/// Kommandozeile eines Prozesses, ohne seinen Speicher zu lesen (`ProcessCommandLineInformation`, ab Windows 8.1
/// mit `PROCESS_QUERY_LIMITED_INFORMATION`); `None`, wenn Windows sie nicht herausgibt
fn command_line(handle: HANDLE) -> Option<String> {
    // Erst die Größe erfragen: ein UNICODE_STRING, dahinter der Text
    let mut size = 0u32;
    unsafe { NtQueryInformationProcess(handle, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut size) };
    let header = size_of::<UNICODE_STRING>();
    if (size as usize) < header || size > 1 << 20 {
        return None;
    }
    // In u64, damit der Kopf richtig ausgerichtet ist
    let mut buffer = vec![0u64; (size as usize).div_ceil(size_of::<u64>())];
    let status = unsafe {
        NtQueryInformationProcess(handle, ProcessCommandLineInformation, buffer.as_mut_ptr().cast(), size, &mut size)
    };
    if status < 0 {
        return None;
    }
    let text = unsafe { &*buffer.as_ptr().cast::<UNICODE_STRING>() };
    let len = usize::from(text.Length) / 2;
    if len == 0 {
        return Some(String::new());
    }
    // Der Text muss im Puffer hinter dem Kopf liegen.
    let start = buffer.as_ptr() as usize + header;
    let end = buffer.as_ptr() as usize + buffer.len() * size_of::<u64>();
    let text_at = text.Buffer as usize;
    if text_at < start || text_at + len * 2 > end || !text_at.is_multiple_of(2) {
        return None;
    }
    let chars = unsafe { std::slice::from_raw_parts(text.Buffer, len) };
    Some(String::from_utf16_lossy(chars))
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}
