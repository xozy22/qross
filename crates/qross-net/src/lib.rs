//! QUIC-Transport und Laufzeit von Server und Client.
//!
//! Sicherheit: Jeder Rechner hat ein selbstsigniertes Zertifikat. Vertrauen entsteht über den
//! SHA-256-Fingerprint, nicht über eine CA:
//! - Der **Server** nimmt nur Clients an, deren Fingerprint er kennt (`qross trust`).
//! - Der **Client** merkt sich den Fingerprint des Servers beim ersten Kontakt (Trust on First
//!   Use) oder prüft gegen einen fest vorgegebenen Wert.

pub mod client;
mod clipboard;
pub mod clipfiles;
pub mod discovery;
mod drag_preview;
pub mod files;
pub mod game;
mod identity;
pub mod latency;
pub mod progress;
pub mod quality;
pub mod secure_input;
pub mod screensaver;
pub mod server;
mod switch_trace;
mod rights;
mod streams;
/// TLS-Konfiguration; öffentlich auch für Tests, die ältere Gegenstellen nachbauen
pub mod tls;
mod trust;
pub mod wake;

pub use identity::{Fingerprint, Identity};
pub use trust::{TrustOutcome, TrustStore};

/// Alle Ressourcen, die eine Identität beschreiben, liegen hier.
pub const IDENTITY_CERT_FILE: &str = "identity.cert.der";
pub const IDENTITY_KEY_FILE: &str = "identity.key.der";
pub const TRUSTED_CLIENTS_FILE: &str = "trusted-clients.toml";
pub const KNOWN_SERVERS_FILE: &str = "known-servers.toml";
