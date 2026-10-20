//! Windows: dieser Prozess und alle, die er gestartet hat, samt deren Nachfahren (WebView2 mit seinen
//! Hilfsprozessen für Darstellung, Grafik und Netz).

use std::collections::HashMap;
use std::mem::size_of;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

use super::Process;

pub(crate) fn processes() -> Vec<Process> {
    let own = std::process::id();
    let Some(root) = measure(own) else { return Vec::new() };
    let children = children_by_parent();
    let mut found = vec![root];
    // Ein Kind muss nach seinem Elternprozess gestartet sein; sonst trägt es nur die Nummer eines
    // beendeten Elternprozesses, die Windows inzwischen an diesen vergeben hat.
    let mut queue = vec![(own, root.id.1)];
    while let Some((parent, started)) = queue.pop() {
        for &pid in children.get(&parent).into_iter().flatten() {
            let Some(child) = measure(pid).filter(|child| child.id.1 >= started) else { continue };
            queue.push((pid, child.id.1));
            found.push(child);
        }
    }
    found
}

/// Alle Prozesse des Systems nach ihrem Elternprozess
fn children_by_parent() -> HashMap<u32, Vec<u32>> {
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
            if entry.th32ProcessID != 0 && entry.th32ProcessID != entry.th32ParentProcessID {
                children.entry(entry.th32ParentProcessID).or_default().push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    children
}

/// Rechenzeit und privater Arbeitssatz eines Prozesses; `None`, wenn er sich nicht (mehr) öffnen lässt
fn measure(pid: u32) -> Option<Process> {
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
        CloseHandle(handle);
        if !times {
            return None;
        }
        // Wie der Task-Manager: der private Arbeitssatz. Wo Windows ihn nicht kennt, die privat
        // zugesagten Seiten.
        let private = match (memory, counters.PrivateWorkingSetSize) {
            (false, _) => 0,
            (true, 0) => counters.PrivateUsage,
            (true, working_set) => working_set,
        };
        Some(Process {
            id: (pid, ticks(created)),
            // FILETIME zählt in 100 ns
            cpu_ns: ticks(kernel).saturating_add(ticks(user)).saturating_mul(100),
            memory: private as u64,
        })
    }
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}
