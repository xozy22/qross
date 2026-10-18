//! Drag & Drop am Client: Welcher Ordner liegt unter dem Cursor?
//! Desktop → Desktop-Ordner; Explorer-Fenster → dessen aktueller Ordner (bei Windows-11-Tabs
//! der sichtbare Tab). Alles andere (Programme, virtuelle Ordner) → `None`.

use std::cell::Cell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::{w, Interface, GUID};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IServiceProvider, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView, IPersistFolder2, IShellBrowser, IShellWindows, IWebBrowserApp, SHGetKnownFolderPath,
    SHGetPathFromIDListW, ShellWindows, FOLDERID_Desktop, KF_FLAG_DEFAULT, SID_STopLevelBrowser,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetAncestor, GetClassNameW, WindowFromPoint, GA_ROOT};

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

pub fn folder_at(x: i32, y: i32) -> Option<PathBuf> {
    unsafe {
        if !COM_READY.get() {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            COM_READY.set(true);
        }
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.0.is_null() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let folder = match class_name(root).as_str() {
            "Progman" | "WorkerW" => known_folder(&FOLDERID_Desktop),
            "CabinetWClass" | "ExploreWClass" => explorer_folder(root),
            other => {
                tracing::debug!("unter dem Cursor liegt kein Ordner (Fensterklasse {other})");
                None
            }
        };
        tracing::debug!(?folder, "Ablageordner bei {x},{y}");
        folder
    }
}

/// Diagnose: Ordner eines Explorer-Fensters über sein Handle.
pub fn folder_of_window(hwnd: isize) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        explorer_folder(HWND(hwnd as _))
    }
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..len])
}

unsafe fn known_folder(id: &GUID) -> Option<PathBuf> {
    let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
    let result = path.to_string().ok().map(PathBuf::from);
    CoTaskMemFree(Some(path.0 as _));
    result
}

unsafe fn explorer_folder(root: HWND) -> Option<PathBuf> {
    let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
    // Bei Tabs ist der erste ShellTabWindowClass-Kindknoten der sichtbare Tab.
    let active_tab = FindWindowExW(Some(root), None, w!("ShellTabWindowClass"), None).ok();
    let mut fallback = None;
    for i in 0..windows.Count().ok()? {
        let Ok(item) = windows.Item(&VARIANT::from(i)) else { continue };
        let Ok(app) = item.cast::<IWebBrowserApp>() else { continue };
        if app.HWND().map(|h| h.0) != Ok(root.0 as isize) {
            continue;
        }
        let Ok(provider) = item.cast::<IServiceProvider>() else { continue };
        let Ok(browser) = provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
        let Some(path) = browser_folder(&browser) else { continue };
        if active_tab.is_some() && browser.GetWindow().ok() == active_tab {
            return Some(path);
        }
        fallback.get_or_insert(path);
    }
    fallback
}

unsafe fn browser_folder(browser: &IShellBrowser) -> Option<PathBuf> {
    let view = browser.QueryActiveShellView().ok()?;
    let folder_view: IFolderView = view.cast().ok()?;
    let persist: IPersistFolder2 = folder_view.GetFolder().ok()?;
    let pidl = persist.GetCurFolder().ok()?;
    let mut buf = [0u16; 260];
    let ok = SHGetPathFromIDListW(pidl, &mut buf).as_bool();
    CoTaskMemFree(Some(pidl as _));
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    // Virtuelle Ordner (Dieser PC, Bibliotheken …) haben keinen Dateisystempfad.
    (ok && len > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len])))
}
