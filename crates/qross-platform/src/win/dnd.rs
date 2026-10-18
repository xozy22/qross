//! Werden gerade Dateien gezogen? Dazu erscheint kurz ein unsichtbares OLE-Drop-Ziel
//! (48×48 px, fast vollständig transparent) unter dem Cursor. Der laufende Drag meldet beim
//! Betreten seine Daten – daraus lesen wir die Dateiliste. Kommt innerhalb von 400 ms nichts,
//! war es kein Datei-Drag (z. B. ein Fenster am Rand).
//!
//! Genutzt vom Server (Dateien an der Kante zum Client) und vom Client (Dateien zurück zum Server).

use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::thread;

use anyhow::anyhow;
use windows::core::{implement, w, Ref};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINTL, WPARAM};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    OleInitialize, RegisterDragDrop, ReleaseStgMedium, IDropTarget, IDropTarget_Impl, CF_HDROP, DROPEFFECT,
    DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, PostMessageW, RegisterClassW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, HWND_TOPMOST, LWA_ALPHA, MSG, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_HIDE, WM_APP, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

/// Zahl losgelassener Maustasten im lokalen Modus (pflegt der Hook). Nach einer erfolglosen
/// Prüfung wird erst nach dem nächsten Loslassen wieder geprüft – sonst würde das Drop-Ziel
/// flackern, solange jemand ein Fenster gegen die Kante drückt.
pub static BUTTON_RELEASES: AtomicU32 = AtomicU32::new(0);

static PROBE_HWND: AtomicIsize = AtomicIsize::new(0);
const WM_PROBE: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const PROBE_SIZE: i32 = 48;
const PROBE_TIMEOUT_MS: u32 = 400;

/// Empfänger des Ergebnisses der laufenden Prüfung
type Reply = Box<dyn FnOnce(Vec<PathBuf>) + Send>;

/// Angeforderte Prüfung: Empfänger und ob die Wiederholungssperre gilt
static PENDING: Mutex<Option<(Reply, bool)>> = Mutex::new(None);

struct ProbeState {
    active: bool,
    reply: Option<Reply>,
    /// `BUTTON_RELEASES` beim letzten Ergebnis ohne Dateien
    empty_at: Option<u32>,
}

thread_local! {
    static PROBE: RefCell<ProbeState> = const { RefCell::new(ProbeState { active: false, reply: None, empty_at: None }) };
}

/// Startet den Drop-Ziel-Thread (STA mit eigenem Fenster und Nachrichtenschleife) – nur einmal.
pub fn start() -> anyhow::Result<()> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            thread::Builder::new()
                .name("qross-dnd".into())
                .spawn(move || run(ready_tx))
                .map_err(|e| e.to_string())?;
            ready_rx.recv().map_err(|e| e.to_string())?
        })
        .clone()
        .map_err(|e| anyhow!("Drag-&-Drop-Erkennung nicht verfügbar: {e}"))
}

/// Fordert eine Prüfung an; `reply` bekommt die gezogenen Dateien (leer: kein Datei-Drag).
/// `once_per_press`: nach einem leeren Ergebnis erst nach dem nächsten Loslassen wieder prüfen
/// (pflegt der Hook des Servers). `false`, wenn die Erkennung nicht läuft.
pub fn probe(x: i32, y: i32, once_per_press: bool, reply: impl FnOnce(Vec<PathBuf>) + Send + 'static) -> bool {
    let hwnd = PROBE_HWND.load(Ordering::Acquire);
    if hwnd == 0 {
        return false;
    }
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some((Box::new(reply), once_per_press));
    unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_PROBE, WPARAM(x as u32 as usize), LPARAM(y as isize)) }.is_ok()
}

fn run(ready: Sender<Result<(), String>>) {
    match unsafe { setup() } {
        Ok(hwnd) => {
            PROBE_HWND.store(hwnd.0 as isize, Ordering::Release);
            let _ = ready.send(Ok(()));
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
        }
    }
}

unsafe fn setup() -> windows::core::Result<HWND> {
    OleInitialize(None)?;
    let instance = HINSTANCE(GetModuleHandleW(None)?.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: w!("QrossDragProbe"),
        ..Default::default()
    };
    RegisterClassW(&class);
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("QrossDragProbe"),
        w!(""),
        WS_POPUP,
        0,
        0,
        PROBE_SIZE,
        PROBE_SIZE,
        None,
        None,
        Some(instance),
        None,
    )?;
    // Alpha 1 statt 0: unsichtbar, aber Windows testet das Fenster noch auf Treffer.
    SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)?;
    let target: IDropTarget = DropTarget.into();
    RegisterDragDrop(hwnd, &target)?;
    Ok(hwnd)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PROBE => {
            on_probe(hwnd, wparam.0 as u32 as i32, lparam.0 as i32);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            finish(hwnd, Vec::new());
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn on_probe(hwnd: HWND, x: i32, y: i32) {
    let Some((reply, once_per_press)) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let releases = BUTTON_RELEASES.load(Ordering::Acquire);
    let show = PROBE.with_borrow_mut(|p| {
        if once_per_press && p.empty_at == Some(releases) {
            // Dieselbe gedrückte Taste wie bei der letzten erfolglosen Prüfung
            reply(Vec::new());
            return false;
        }
        if let Some(previous) = p.reply.replace(reply) {
            previous(Vec::new());
        }
        p.active = true;
        true
    });
    if show {
        unsafe {
            let half = PROBE_SIZE / 2;
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x - half,
                y - half,
                PROBE_SIZE,
                PROBE_SIZE,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            SetTimer(Some(hwnd), TIMER_ID, PROBE_TIMEOUT_MS, None);
        }
    }
}

fn finish(hwnd: HWND, files: Vec<PathBuf>) {
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_ID);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    PROBE.with_borrow_mut(|p| {
        if !p.active {
            return;
        }
        p.active = false;
        p.empty_at = files.is_empty().then(|| BUTTON_RELEASES.load(Ordering::Acquire));
        if !files.is_empty() {
            tracing::info!("{} Datei(en) werden über die Kante gezogen", files.len());
        }
        if let Some(reply) = p.reply.take() {
            reply(files);
        }
    });
}

#[implement(IDropTarget)]
struct DropTarget;

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        refuse(effect);
        // Auch ein Drag ohne Dateien (z. B. markierter Text) beendet die Prüfung – ergebnislos.
        let files = data.as_ref().map(|d| unsafe { dropped_files(d) }).unwrap_or_default();
        finish(HWND(PROBE_HWND.load(Ordering::Acquire) as _), files);
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        refuse(effect);
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        _data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        refuse(effect);
        Ok(())
    }
}

/// Wir nehmen nie wirklich etwas an – die Quelle soll nichts verschieben oder löschen.
fn refuse(effect: *mut DROPEFFECT) {
    if !effect.is_null() {
        unsafe { *effect = DROPEFFECT_NONE };
    }
}

unsafe fn dropped_files(data: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = data.GetData(&format) else { return Vec::new() };
    let hdrop = HDROP(medium.u.hGlobal.0);
    let count = DragQueryFileW(hdrop, u32::MAX, None);
    let mut files = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = DragQueryFileW(hdrop, i, None) as usize;
        let mut buf = vec![0u16; len + 1];
        DragQueryFileW(hdrop, i, Some(&mut buf));
        files.push(PathBuf::from(OsString::from_wide(&buf[..len])));
    }
    ReleaseStgMedium(&mut medium);
    files
}
