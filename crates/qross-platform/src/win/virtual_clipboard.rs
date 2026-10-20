//! Kopierte Dateien auf Abruf unter Windows: virtuelle Dateien in der OLE-Zwischenablage, wie sie auch
//! Outlook für Anhänge und Remotedesktop ablegen. `FileGroupDescriptorW` nennt Namen, Größen und
//! Ordner, `FileContents` liefert je Datei einen `IStream`. Der Explorer fordert die Inhalte erst beim
//! Einfügen an und zeigt dabei seinen Fortschritt; jedes `Read` holt sie über die `VirtualSource`.
//!
//! Zwei eigene Threads:
//! - **OLE** (STA mit Nachrichtenschleife): `OleSetClipboard` verlangt ein STA. Solange das Datenobjekt
//!   in der Zwischenablage liegt, kommen dort die Aufrufe der Programme an (`GetData`) und das
//!   Nachreichen von Text und Bild für Programme ohne OLE (`WM_RENDERFORMAT`).
//! - **MTA**: Die Streams leben im Multithread-Apartment. Der Explorer liest in Stücken und wartet dabei
//!   auf das Netz; im STA hielte jedes wartende `Read` die Zwischenablage für alle Programme an.
//!   `GetData` gibt deshalb einen Proxy auf einen im MTA gemarshalten Stream heraus. COM verbindet den
//!   Explorer dann direkt mit dem MTA, wo jeder Aufruf auf einem eigenen RPC-Thread läuft.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context};
use windows::core::{implement, w, Interface, Ref, BOOL, HRESULT};
use windows::Win32::Foundation::{
    CLIPBRD_E_CANT_OPEN, DATA_S_SAMEFORMATETC, DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED, ERROR_ACCESS_DENIED,
    ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_UNEXP_NET_ERR, E_FAIL, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, HGLOBAL, HINSTANCE, HWND,
    LPARAM, LRESULT, OLE_E_ADVISENOTSUPPORTED, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, S_OK, WPARAM,
};
use windows::Win32::System::Com::Marshal::CoMarshalInterThreadInterfaceInStream;
use windows::Win32::System::Com::StructuredStorage::CoGetInterfaceAndReleaseStream;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemAlloc, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
    ISequentialStream_Impl, IStream, IStream_Impl, COINIT_MULTITHREADED, DATADIR_GET, DVASPECT_CONTENT, FORMATETC,
    LOCKTYPE, STATFLAG, STATFLAG_NONAME, STATSTG, STGC, STGMEDIUM, STGMEDIUM_0, STGM_READ, STGTY_STREAM,
    STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, OleSetClipboard, ReleaseStgMedium};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, RegisterClassW, HWND_MESSAGE, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WNDCLASSW,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

use super::clipboard::{global, register};
use crate::virtual_files::{file_group_descriptor, is_cancelled};
use crate::{VirtualFile, VirtualFiles};

/// Abschaltbar, falls ein Programm nur echte Dateien annimmt: `QROSS_PASTE_ON_DEMAND=0` (dann kommen
/// kopierte Dateien wieder vorab, bis zur eingestellten Grenze).
pub fn enabled() -> bool {
    std::env::var_os("QROSS_PASTE_ON_DEMAND").is_none_or(|v| v != "0")
}

/// So lange wartet `set` höchstens auf den OLE-Thread (etwa wenn ein anderes Programm hängt, das die
/// Zwischenablage noch besitzt).
const SET_TIMEOUT: Duration = Duration::from_secs(10);
const WM_JOB: u32 = WM_APP + 1;

fn file_group_descriptor_format() -> u16 {
    static SLOT: OnceLock<u32> = OnceLock::new();
    *SLOT.get_or_init(|| register("FileGroupDescriptorW")) as u16
}

fn file_contents_format() -> u16 {
    static SLOT: OnceLock<u32> = OnceLock::new();
    *SLOT.get_or_init(|| register("FileContents")) as u16
}

/// Legt `files` als virtuelle Dateien in die Zwischenablage, dazu `formats` (Format, Inhalt als HGLOBAL).
pub fn set(files: VirtualFiles, formats: Vec<(u32, Vec<u8>)>) -> anyhow::Result<()> {
    let descriptor = file_group_descriptor(&files.entries)?;
    let ole = ole_thread().as_ref().map_err(|e| anyhow!("OLE clipboard not available: {e}"))?;
    let (reply, result) = channel();
    let formats = formats.into_iter().filter_map(|(f, bytes)| Some((u16::try_from(f).ok().filter(|&f| f != 0)?, bytes))).collect();
    let job = Job { descriptor, files, formats, reply };
    ole.jobs.lock().unwrap_or_else(|e| e.into_inner()).send(job).map_err(|_| anyhow!("OLE thread has exited"))?;
    unsafe { PostMessageW(Some(HWND(ole.hwnd as _)), WM_JOB, WPARAM(0), LPARAM(0)) }.context("OLE thread not reachable")?;
    result.recv_timeout(SET_TIMEOUT).map_err(|_| anyhow!("clipboard not responding"))?
}

/// Fügt die Zwischenablage in den Ordner `folder` ein wie Strg+V im Explorer: über das Drop-Ziel des
/// Ordners, im aufrufenden Thread (für Diagnose und Tests; der Thread darf COM noch nicht benutzen).
/// Kehrt zurück, wenn der Explorer fertig ist.
pub fn paste_into(folder: &std::path::Path) -> anyhow::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::POINTL;
    use windows::Win32::System::Ole::{IDropTarget, OleGetClipboard, DROPEFFECT_COPY};
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, BHID_SFUIObject};
    use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_REMOVE};
    unsafe {
        OleInitialize(None)?;
        let data = OleGetClipboard()?;
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(folder.as_os_str()), None)?;
        let target: IDropTarget = item.BindToHandler(None, &BHID_SFUIObject)?;
        let mut effect = DROPEFFECT_COPY;
        target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)?;
        effect = DROPEFFECT_COPY;
        target.Drop(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)?;
        // Was der Explorer danach noch nachreicht
        let mut msg = MSG::default();
        for _ in 0..50 {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

/// Auftrag an den OLE-Thread
struct Job {
    descriptor: Vec<u8>,
    files: VirtualFiles,
    formats: Vec<(u16, Vec<u8>)>,
    reply: Sender<anyhow::Result<()>>,
}

struct OleThread {
    jobs: Mutex<Sender<Job>>,
    /// Nachrichtenfenster des Threads, weckt ihn für neue Aufträge
    hwnd: isize,
}

thread_local! {
    static JOBS: RefCell<Option<Receiver<Job>>> = const { RefCell::new(None) };
}

fn ole_thread() -> &'static Result<OleThread, String> {
    static OLE: OnceLock<Result<OleThread, String>> = OnceLock::new();
    OLE.get_or_init(|| {
        let (ready, started) = channel();
        let (jobs, rx) = channel::<Job>();
        thread::Builder::new()
            .name("qross-ole-clipboard".into())
            .spawn(move || match unsafe { setup() } {
                Ok(hwnd) => {
                    JOBS.with_borrow_mut(|slot| *slot = Some(rx));
                    let _ = ready.send(Ok(hwnd.0 as isize));
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
            })
            .map_err(|e| e.to_string())?;
        let hwnd = started.recv().map_err(|e| e.to_string())??;
        Ok(OleThread { jobs: Mutex::new(jobs), hwnd })
    })
}

unsafe fn setup() -> windows::core::Result<HWND> {
    OleInitialize(None)?;
    let instance = HINSTANCE(GetModuleHandleW(None)?.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: w!("QrossOleClipboard"),
        ..Default::default()
    };
    RegisterClassW(&class);
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("QrossOleClipboard"),
        w!(""),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        Some(instance),
        None,
    )
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg != WM_JOB {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    while let Some(job) = JOBS.with_borrow(|jobs| jobs.as_ref().and_then(|rx| rx.try_recv().ok())) {
        let Job { descriptor, files, formats, reply } = job;
        let _ = reply.send(put(descriptor, files, formats));
    }
    LRESULT(0)
}

/// Im OLE-Thread: das Datenobjekt in die Zwischenablage legen
fn put(descriptor: Vec<u8>, files: VirtualFiles, formats: Vec<(u16, Vec<u8>)>) -> anyhow::Result<()> {
    let object: IDataObject = DataObject { descriptor, files, formats: Mutex::new(formats) }.into();
    // Andere Programme halten die Zwischenablage oft kurz offen – daher ein paar Versuche.
    let mut tries = 0;
    loop {
        match unsafe { OleSetClipboard(&object) } {
            Ok(()) => return Ok(()),
            Err(e) if e.code() == CLIPBRD_E_CANT_OPEN && tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(anyhow!("failed to set the clipboard: {e}")),
        }
    }
}

/// Das Datenobjekt in der Zwischenablage. Lebt im STA des OLE-Threads.
#[implement(IDataObject, Agile = false)]
struct DataObject {
    /// Inhalt von `FileGroupDescriptorW`
    descriptor: Vec<u8>,
    files: VirtualFiles,
    /// Weitere Formate als HGLOBAL: Text, HTML, Bild, „Preferred DropEffect“ und was Programme mit
    /// `SetData` dazulegen (der Explorer etwa „Paste Succeeded“)
    formats: Mutex<Vec<(u16, Vec<u8>)>>,
}

impl DataObject {
    /// Datei zu `lindex` in `FileContents`; -1 nimmt die einzige Datei (so fragen manche Programme).
    fn file_index(&self, lindex: i32) -> Option<usize> {
        let entries = &self.files.entries;
        match usize::try_from(lindex) {
            Ok(index) => entries.get(index).filter(|e| !e.is_dir).map(|_| index),
            Err(_) if lindex == -1 => {
                let mut files = entries.iter().enumerate().filter(|(_, e)| !e.is_dir);
                match (files.next(), files.next()) {
                    (Some((index, _)), None) => Some(index),
                    _ => None,
                }
            }
            Err(_) => None,
        }
    }

    fn bytes(&self, format: u16) -> Option<Vec<u8>> {
        if format == file_group_descriptor_format() {
            return Some(self.descriptor.clone());
        }
        self.formats.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(f, _)| *f == format).map(|(_, b)| b.clone())
    }

    fn has(&self, format: u16) -> bool {
        format == file_group_descriptor_format()
            || self.formats.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(f, _)| *f == format)
    }

    /// Prüft eine Anfrage, ohne etwas herauszugeben.
    fn check(&self, format: &FORMATETC) -> HRESULT {
        if format.dwAspect != DVASPECT_CONTENT.0 {
            return DV_E_FORMATETC;
        }
        if format.cfFormat == file_contents_format() {
            if format.tymed & TYMED_ISTREAM.0 as u32 == 0 {
                return DV_E_TYMED;
            }
            return if self.file_index(format.lindex).is_some() { S_OK } else { DV_E_LINDEX };
        }
        if !self.has(format.cfFormat) {
            return DV_E_FORMATETC;
        }
        if format.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
            return DV_E_TYMED;
        }
        S_OK
    }
}

impl IDataObject_Impl for DataObject_Impl {
    fn GetData(&self, format: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let format = unsafe { format.as_ref() }.ok_or(E_POINTER)?;
        tracing::trace!(format = format.cfFormat, lindex = format.lindex, tymed = format.tymed, "GetData");
        self.check(format).ok()?;
        if format.cfFormat == file_contents_format() {
            let index = self.file_index(format.lindex).ok_or(DV_E_LINDEX)?;
            let entry = &self.files.entries[index];
            let name = entry.path.rsplit('/').next().unwrap_or_default().encode_utf16().collect();
            let stream: IStream = VirtualStream {
                size: entry.size,
                name,
                state: Mutex::new(StreamState { position: 0, file: self.files.source.open(index) }),
            }
            .into();
            return Ok(STGMEDIUM {
                tymed: TYMED_ISTREAM.0 as u32,
                u: STGMEDIUM_0 { pstm: ManuallyDrop::new(Some(in_mta(stream)?)) },
                pUnkForRelease: ManuallyDrop::new(None),
            });
        }
        let bytes = self.bytes(format.cfFormat).ok_or(DV_E_FORMATETC)?;
        let memory = unsafe { global(&bytes) }.map_err(|_| windows::core::Error::from(E_OUTOFMEMORY))?;
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: HGLOBAL(memory) },
            pUnkForRelease: ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(&self, _format: *const FORMATETC, _medium: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        match unsafe { format.as_ref() } {
            Some(format) => self.check(format),
            None => E_POINTER,
        }
    }

    fn GetCanonicalFormatEtc(&self, format_in: *const FORMATETC, format_out: *mut FORMATETC) -> HRESULT {
        unsafe {
            if let (Some(input), Some(output)) = (format_in.as_ref(), format_out.as_mut()) {
                *output = *input;
                output.ptd = std::ptr::null_mut();
            }
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, format: *const FORMATETC, medium: *const STGMEDIUM, release: BOOL) -> windows::core::Result<()> {
        let (format, medium) = unsafe { (format.as_ref().ok_or(E_POINTER)?, medium.as_ref().ok_or(E_POINTER)?) };
        if medium.tymed != TYMED_HGLOBAL.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let bytes = unsafe {
            let handle = medium.u.hGlobal.0;
            let ptr = GlobalLock(handle) as *const u8;
            if ptr.is_null() {
                Vec::new()
            } else {
                let bytes = std::slice::from_raw_parts(ptr, GlobalSize(handle)).to_vec();
                GlobalUnlock(handle);
                bytes
            }
        };
        {
            let mut formats = self.formats.lock().unwrap_or_else(|e| e.into_inner());
            formats.retain(|(f, _)| *f != format.cfFormat);
            formats.push((format.cfFormat, bytes));
        }
        if release.as_bool() {
            unsafe { ReleaseStgMedium(medium as *const STGMEDIUM as *mut STGMEDIUM) };
        }
        Ok(())
    }

    fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let entry = |cf: u16, tymed: u32| FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed,
        };
        let mut list = vec![
            entry(file_group_descriptor_format(), TYMED_HGLOBAL.0 as u32),
            entry(file_contents_format(), TYMED_ISTREAM.0 as u32),
        ];
        let formats = self.formats.lock().unwrap_or_else(|e| e.into_inner());
        list.extend(formats.iter().map(|(cf, _)| entry(*cf, TYMED_HGLOBAL.0 as u32)));
        unsafe { SHCreateStdEnumFmtEtc(&list) }
    }

    fn DAdvise(&self, _format: *const FORMATETC, _advf: u32, _sink: Ref<'_, IAdviseSink>) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _connection: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

struct StreamState {
    position: u64,
    file: Box<dyn VirtualFile>,
}

/// Inhalt einer Datei für `FileContents`. Lebt im MTA (siehe [`in_mta`]); die Aufrufe kommen auf
/// beliebigen RPC-Threads an, auch gleichzeitig.
#[implement(IStream, Agile = false)]
struct VirtualStream {
    size: u64,
    /// Dateiname für `Stat`
    name: Vec<u16>,
    state: Mutex<StreamState>,
}

/// Fehler beim Lesen für den Explorer: Ein Abbruch heißt „abgebrochen“ (er hört ohne Meldung auf), sonst
/// eine möglichst passende Meldung.
fn read_error(error: &std::io::Error) -> HRESULT {
    let code = if is_cancelled(error) {
        ERROR_CANCELLED
    } else {
        match error.kind() {
            std::io::ErrorKind::NotFound => ERROR_FILE_NOT_FOUND,
            std::io::ErrorKind::PermissionDenied => ERROR_ACCESS_DENIED,
            _ => ERROR_UNEXP_NET_ERR,
        }
    };
    code.to_hresult()
}

impl ISequentialStream_Impl for VirtualStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        if pv.is_null() {
            return E_POINTER;
        }
        let buf = unsafe { std::slice::from_raw_parts_mut(pv as *mut u8, cb as usize) };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Ganz füllen, bis das Ende kommt: Manche Leser halten ein kurzes Stück für das Ende der Datei.
        let mut filled = 0;
        while filled < buf.len() {
            let position = state.position;
            match state.file.read_at(position, &mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => {
                    filled += n;
                    state.position += n as u64;
                }
                // Lieber ganz scheitern als ein gekürztes Stück abliefern
                Err(e) => {
                    tracing::debug!("failed to read the virtual file: {e}");
                    return read_error(&e);
                }
            }
        }
        if !pcbread.is_null() {
            unsafe { *pcbread = filled as u32 };
        }
        S_OK
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for VirtualStream_Impl {
    fn Seek(&self, distance: i64, origin: STREAM_SEEK, new_position: *mut u64) -> windows::core::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let base = match origin {
            STREAM_SEEK_SET => 0,
            STREAM_SEEK_CUR => i128::from(state.position),
            STREAM_SEEK_END => i128::from(self.size),
            _ => return Err(STG_E_INVALIDFUNCTION.into()),
        };
        let target = u64::try_from(base + i128::from(distance)).map_err(|_| windows::core::Error::from(STG_E_INVALIDFUNCTION))?;
        state.position = target;
        if !new_position.is_null() {
            unsafe { *new_position = target };
        }
        Ok(())
    }

    fn SetSize(&self, _size: u64) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn CopyTo(&self, target: Ref<'_, IStream>, cb: u64, pcbread: *mut u64, pcbwritten: *mut u64) -> windows::core::Result<()> {
        let target = target.ok()?;
        let mut buf = vec![0u8; 1 << 20];
        let (mut read, mut written) = (0u64, 0u64);
        while read < cb {
            let want = (cb - read).min(buf.len() as u64) as u32;
            let mut n = 0u32;
            self.Read(buf.as_mut_ptr().cast(), want, &mut n).ok()?;
            if n == 0 {
                break;
            }
            read += u64::from(n);
            let mut done = 0u32;
            unsafe { target.Write(buf.as_ptr().cast(), n, Some(&mut done as *mut u32)) }.ok()?;
            written += u64::from(done);
        }
        unsafe {
            if !pcbread.is_null() {
                *pcbread = read;
            }
            if !pcbwritten.is_null() {
                *pcbwritten = written;
            }
        }
        Ok(())
    }

    fn Commit(&self, _flags: &STGC) -> windows::core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _offset: u64, _cb: u64, _lock: &LOCKTYPE) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(&self, _offset: u64, _cb: u64, _lock: u32) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, stat: *mut STATSTG, flag: &STATFLAG) -> windows::core::Result<()> {
        let stat = unsafe { stat.as_mut() }.ok_or(E_POINTER)?;
        *stat = STATSTG { r#type: STGTY_STREAM.0 as u32, cbSize: self.size, grfMode: STGM_READ, ..Default::default() };
        if flag.0 & STATFLAG_NONAME.0 == 0 {
            unsafe {
                let name = CoTaskMemAlloc((self.name.len() + 1) * 2) as *mut u16;
                if !name.is_null() {
                    std::ptr::copy_nonoverlapping(self.name.as_ptr(), name, self.name.len());
                    *name.add(self.name.len()) = 0;
                    stat.pwcsName = windows::core::PWSTR(name);
                }
            }
        }
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

/// COM-Zeiger für den Weg zwischen den Threads: der Stream vor dem Marshaling, danach das Paket
/// aus `CoMarshalInterThreadInterfaceInStream` (dafür gemacht, in einen anderen Thread zu gehen).
struct Handover(IStream);

// SAFETY: Der Stream ist noch keinem Apartment zugeordnet, das Paket ist ein Speicher-Stream.
unsafe impl Send for Handover {}

type MarshalJob = (Handover, Sender<windows::core::Result<Handover>>);

/// Marshalt `stream` im MTA und liefert einen Proxy darauf für diesen Thread (siehe Modulbeschreibung).
fn in_mta(stream: IStream) -> windows::core::Result<IStream> {
    static MTA: OnceLock<Option<Mutex<Sender<MarshalJob>>>> = OnceLock::new();
    let mta = MTA.get_or_init(|| {
        let (jobs, rx) = channel::<MarshalJob>();
        let (ready, started) = channel();
        let spawned = thread::Builder::new().name("qross-ole-mta".into()).spawn(move || {
            let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let _ = ready.send(joined);
            // Bleibt im MTA, solange Qross läuft: Mit dem letzten Thread darin endeten auch die Streams.
            while let Ok((Handover(stream), reply)) = rx.recv() {
                let packet = unsafe { CoMarshalInterThreadInterfaceInStream(&IStream::IID, &stream) };
                let _ = reply.send(packet.map(Handover));
            }
        });
        match spawned.ok().and_then(|_| started.recv().ok()) {
            Some(true) => Some(Mutex::new(jobs)),
            _ => None,
        }
    });
    let Some(jobs) = mta else {
        tracing::warn!("MTA for virtual files not available; streams run in the STA");
        return Ok(stream);
    };
    let (reply, packet) = channel();
    jobs.lock().unwrap_or_else(|e| e.into_inner()).send((Handover(stream), reply)).map_err(|_| windows::core::Error::from(E_FAIL))?;
    let Handover(packet) = packet.recv().map_err(|_| windows::core::Error::from(E_FAIL))??;
    unsafe { CoGetInterfaceAndReleaseStream(&packet) }
}
