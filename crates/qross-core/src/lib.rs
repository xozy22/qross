//! Plattformunabhängige Logik: Bildschirm-Layout und die Zustandsmaschinen von Server und
//! Client. Bewusst ohne I/O, damit sie vollständig per Unit-Test prüfbar ist.

pub mod client;
pub mod input;
pub mod layout;
pub mod server;

pub use input::{CaptureEvent, Inject};
pub use layout::{Layout, LayoutError, LinkConfig};
