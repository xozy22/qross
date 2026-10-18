//! Ersatz-Implementierungen ohne Betriebssystem-Zugriff: für `--dry-run` und Tests.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use qross_core::Inject;
use qross_proto::ClipboardData;

use crate::{Capture, Clipboard, Injector};

/// Protokolliert eingespeiste Ereignisse, statt sie auszuführen.
pub struct LogInjector;

impl Injector for LogInjector {
    fn inject(&mut self, event: &Inject) {
        match event {
            Inject::MoveTo { .. } => tracing::trace!(?event, "würde einspeisen"),
            _ => tracing::info!(?event, "würde einspeisen"),
        }
    }
}

/// Merkt sich alle Ereignisse (für Tests); `1` ist der Ablageordner für Drag & Drop.
#[derive(Clone, Default)]
pub struct RecordingInjector(pub Arc<Mutex<Vec<Inject>>>, pub Arc<Mutex<Option<PathBuf>>>);

impl Injector for RecordingInjector {
    fn inject(&mut self, event: &Inject) {
        self.0.lock().unwrap().push(*event);
    }

    fn drop_folder(&mut self, _x: i32, _y: i32) -> Option<PathBuf> {
        self.1.lock().unwrap().clone()
    }
}

#[derive(Default)]
pub struct MemoryClipboard {
    data: Option<ClipboardData>,
    seq: u64,
}

impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> Option<ClipboardData> {
        self.data.clone()
    }

    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()> {
        self.data = (!data.is_empty()).then(|| data.clone());
        self.seq += 1;
        Ok(())
    }

    fn sequence(&mut self) -> u64 {
        self.seq
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCall {
    Remote,
    Release { x: i32, y: i32 },
    ProbeDrag { x: i32, y: i32 },
    CancelLocalDrag,
}

/// Merkt sich Aufrufe der Erfassungs-Steuerung (für Tests).
#[derive(Clone, Default)]
pub struct RecordingCapture(pub Arc<Mutex<Vec<CaptureCall>>>);

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
}
