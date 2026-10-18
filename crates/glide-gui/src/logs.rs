//! Log-Ansicht: ein `tracing`-Layer, der Meldungen puffert und an das Fenster schickt.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const MAX_LINES: usize = 2000;

#[derive(Serialize, Clone)]
pub struct LogLine {
    /// Unix-Zeit in Millisekunden
    pub ts: u64,
    pub level: &'static str,
    pub message: String,
}

#[derive(Default)]
pub struct LogBuffer {
    lines: Mutex<VecDeque<LogLine>>,
    app: OnceLock<AppHandle>,
}

impl LogBuffer {
    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    pub fn snapshot(&self) -> Vec<LogLine> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }

    fn push(&self, line: LogLine) {
        {
            let mut lines = self.lines.lock().unwrap();
            if lines.len() == MAX_LINES {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        if let Some(app) = self.app.get() {
            let _ = app.emit("log", line);
        }
    }
}

thread_local! {
    /// Verhindert Rekursion, falls beim Senden an das Fenster selbst geloggt wird.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

struct GuiLayer(Arc<LogBuffer>);

impl<S: Subscriber> Layer<S> for GuiLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if BUSY.with(|b| b.replace(true)) {
            return;
        }
        let mut visitor = Message::default();
        event.record(&mut visitor);
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        self.0.push(LogLine { ts, level: event.metadata().level().as_str(), message: visitor.0 });
        BUSY.with(|b| b.set(false));
    }
}

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.insert_str(0, value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0.insert_str(0, &format!("{value:?}"));
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

/// Richtet das Logging ein: Konsole (in Debug-Builds sichtbar) und Fenster.
pub fn init() -> Arc<LogBuffer> {
    let buffer = Arc::new(LogBuffer::default());
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,quinn=warn,tao=warn,wry=warn,tauri=warn"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(GuiLayer(buffer.clone()))
        .init();
    buffer
}
