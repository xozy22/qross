//! Ersatz-Implementierungen ohne Betriebssystem-Zugriff: für `--dry-run` und Tests.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use qross_core::keys::KeyOptions;
use qross_core::Inject;
use qross_proto::{ClipboardData, CursorFinder};

use crate::{Capture, Clipboard, FinderOverlay, Injector};

/// Protokolliert eingespeiste Ereignisse, statt sie auszuführen.
pub struct LogInjector;

impl Injector for LogInjector {
    fn inject(&mut self, event: &Inject) {
        match event {
            Inject::MoveTo { .. } | Inject::MoveBy { .. } => tracing::trace!(?event, "würde einspeisen"),
            _ => tracing::info!(?event, "würde einspeisen"),
        }
    }

    fn types_text(&self) -> bool {
        true
    }
}

/// Merkt sich alle Ereignisse (für Tests); `1` ist der Ablageordner für Drag & Drop, `2` die
/// Dateien, die hier gerade gezogen werden (Drag & Drop zum Server), `3`: kann Zeichen eingeben
/// (Zeichenmodus, siehe [`RecordingInjector::typing`]).
#[derive(Clone, Default)]
pub struct RecordingInjector(
    pub Arc<Mutex<Vec<Inject>>>,
    pub Arc<Mutex<Option<PathBuf>>>,
    pub Arc<Mutex<Vec<PathBuf>>>,
    pub bool,
);

impl RecordingInjector {
    /// Einer, der Zeichen eingeben kann (wie Windows, macOS, X11)
    pub fn typing() -> Self {
        Self { 3: true, ..Self::default() }
    }
}

impl Injector for RecordingInjector {
    fn inject(&mut self, event: &Inject) {
        self.0.lock().unwrap().push(*event);
    }

    fn types_text(&self) -> bool {
        self.3
    }

    fn drop_folder(&mut self, _x: i32, _y: i32) -> Option<PathBuf> {
        self.1.lock().unwrap().clone()
    }

    fn take_dragged_files(&mut self, _x: i32, _y: i32) -> Vec<PathBuf> {
        std::mem::take(&mut *self.2.lock().unwrap())
    }
}

#[derive(Default)]
pub struct MemoryClipboard {
    data: Option<ClipboardData>,
    files: Vec<PathBuf>,
    seq: u64,
    /// Markierung eines Passwortmanagers zum aktuellen Inhalt
    concealed: Option<&'static str>,
}

impl MemoryClipboard {
    /// Markiert den aktuellen Inhalt wie ein Passwortmanager (`marker` z. B. aus
    /// `concealed::WINDOWS`); der nächste `write` hebt das auf.
    pub fn conceal(&mut self, marker: &'static str) {
        self.concealed = Some(marker);
    }
}

impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> Option<ClipboardData> {
        self.data.clone()
    }

    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()> {
        self.data = (!data.is_empty()).then(|| data.clone());
        self.files.clear();
        self.concealed = None;
        self.seq += 1;
        Ok(())
    }

    fn sequence(&mut self) -> u64 {
        self.seq
    }

    fn files(&mut self) -> Vec<PathBuf> {
        self.files.clone()
    }

    fn write_files(&mut self, files: &[PathBuf], data: &ClipboardData) -> anyhow::Result<()> {
        self.write(data)?;
        self.files = files.to_vec();
        Ok(())
    }

    fn concealed(&mut self) -> Option<&'static str> {
        self.concealed
    }
}

/// Von außen veränderbares Clipboard (für Tests).
#[derive(Clone, Default)]
pub struct SharedClipboard(pub Arc<Mutex<MemoryClipboard>>);

impl Clipboard for SharedClipboard {
    fn read(&mut self) -> Option<ClipboardData> {
        self.0.lock().unwrap().read()
    }

    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()> {
        self.0.lock().unwrap().write(data)
    }

    fn sequence(&mut self) -> u64 {
        self.0.lock().unwrap().sequence()
    }

    fn files(&mut self) -> Vec<PathBuf> {
        self.0.lock().unwrap().files()
    }

    fn write_files(&mut self, files: &[PathBuf], data: &ClipboardData) -> anyhow::Result<()> {
        self.0.lock().unwrap().write_files(files, data)
    }

    fn concealed(&mut self) -> Option<&'static str> {
        self.0.lock().unwrap().concealed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCall {
    Remote,
    Release { x: i32, y: i32 },
    ProbeDrag { x: i32, y: i32 },
    CancelLocalDrag,
    AwaitLocalDrop,
}

/// Merkt sich Aufrufe der Erfassungs-Steuerung (für Tests); `1` ist der Ablageordner für Dateien
/// vom Client, `2` die zuletzt eingestellten Tasten-Optionen.
#[derive(Clone, Default)]
pub struct RecordingCapture(
    pub Arc<Mutex<Vec<CaptureCall>>>,
    pub Arc<Mutex<Option<PathBuf>>>,
    pub Arc<Mutex<Option<KeyOptions>>>,
);

impl Capture for RecordingCapture {
    fn capture_remote(&self) {
        self.0.lock().unwrap().push(CaptureCall::Remote);
    }

    fn release(&self, x: i32, y: i32) {
        self.0.lock().unwrap().push(CaptureCall::Release { x, y });
    }

    fn probe_drag(&self, x: i32, y: i32) {
        self.0.lock().unwrap().push(CaptureCall::ProbeDrag { x, y });
    }

    fn cancel_local_drag(&self) {
        self.0.lock().unwrap().push(CaptureCall::CancelLocalDrag);
    }

    fn await_local_drop(&self) {
        self.0.lock().unwrap().push(CaptureCall::AwaitLocalDrop);
    }

    fn drop_folder(&self, _x: i32, _y: i32) -> Option<PathBuf> {
        self.1.lock().unwrap().clone()
    }

    fn set_key_options(&self, options: KeyOptions) {
        *self.2.lock().unwrap() = Some(options);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinderCall {
    Show { style: CursorFinder, name: String },
    Hide,
}

/// Merkt sich, was der Cursor-Finder zeigen sollte (für Tests)
#[derive(Clone, Default)]
pub struct RecordingFinder(pub Arc<Mutex<Vec<FinderCall>>>);

impl RecordingFinder {
    /// Alle bisherigen Aufrufe
    pub fn calls(&self) -> Vec<FinderCall> {
        self.0.lock().unwrap().clone()
    }

    /// Nur die Aufrufe zum Zeigen
    pub fn shown(&self) -> Vec<FinderCall> {
        self.calls().into_iter().filter(|c| matches!(c, FinderCall::Show { .. })).collect()
    }
}

impl FinderOverlay for RecordingFinder {
    fn show(&self, style: CursorFinder, name: &str) {
        self.0.lock().unwrap().push(FinderCall::Show { style, name: name.to_owned() });
    }

    fn hide(&self) {
        self.0.lock().unwrap().push(FinderCall::Hide);
    }
}
