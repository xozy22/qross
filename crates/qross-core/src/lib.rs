//! Plattformunabhängige Logik: Bildschirm-Layout und die Zustandsmaschinen von Server und
//! Client. Bewusst ohne I/O, damit sie vollständig per Unit-Test prüfbar ist.

pub mod chars;
pub mod client;
pub mod finder;
pub mod input;
pub mod keys;
pub mod layout;
mod screen;
pub mod server;
pub mod sharing;
pub mod text;

pub use input::{CaptureEvent, Inject};
pub use layout::{Layout, LayoutError, LinkConfig};
