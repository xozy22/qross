//! QUIC-Transport und Laufzeit von Server und Client.
//!
//! Sicherheit: Jeder Rechner hat ein selbstsigniertes Zertifikat. Vertrauen entsteht über den
//! SHA-256-Fingerprint, nicht über eine CA:
//! - Der **Server** nimmt nur Clients an, deren Fingerprint er kennt (`glide trust`).
//! - Der **Client** merkt sich den Fingerprint des Servers beim ersten Kontakt (Trust on First
//!   Use) oder prüft gegen einen fest vorgegebenen Wert.

pub mod client;
pub mod discovery;
pub mod files;
mod identity;
pub mod server;
mod streams;
mod tls;
mod trust;

pub use identity::{Fingerprint, Identity};
pub use trust::{TrustOutcome, TrustStore};

/// Alle Ressourcen, die eine Identität beschreiben, liegen hier.
pub const IDENTITY_CERT_FILE: &str = "identity.cert.der";
pub const IDENTITY_KEY_FILE: &str = "identity.key.der";
pub const TRUSTED_CLIENTS_FILE: &str = "trusted-clients.toml";
pub const KNOWN_SERVERS_FILE: &str = "known-servers.toml";
