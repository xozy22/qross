//! Vertrauensliste: Name bzw. Adresse → erwarteter Fingerprint, als TOML-Datei.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Fingerprint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustOutcome {
    /// Fingerprint stimmt mit dem gespeicherten überein.
    Known,
    /// Noch nichts gespeichert.
    Unknown,
    /// Anderer Fingerprint gespeichert – möglicher Angriff oder neu installierter Rechner.
    Mismatch,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    peers: BTreeMap<String, String>,
}

pub struct TrustStore {
    path: Option<PathBuf>,
    peers: BTreeMap<String, String>,
}

impl TrustStore {
    pub fn in_memory() -> Self {
        Self { path: None, peers: BTreeMap::new() }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let peers = if path.exists() {
            let text = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
            toml::from_str::<File>(&text).with_context(|| format!("{} is invalid", path.display()))?.peers
        } else {
            BTreeMap::new()
        };
        Ok(Self { path: Some(path.to_owned()), peers })
    }

    /// Liest die Datei neu ein (z. B. nachdem `qross trust` in einem anderen Prozess lief).
    pub fn reload(&mut self) -> Result<()> {
        if let Some(path) = self.path.clone() {
            *self = Self::load(&path)?;
        }
        Ok(())
    }

    pub fn check(&self, name: &str, fingerprint: &Fingerprint) -> TrustOutcome {
        match self.peers.get(name).map(|s| s.parse::<Fingerprint>()) {
            None => TrustOutcome::Unknown,
            Some(Ok(known)) if known == *fingerprint => TrustOutcome::Known,
            Some(_) => TrustOutcome::Mismatch,
        }
    }

    pub fn expected(&self, name: &str) -> Option<&str> {
        self.peers.get(name).map(String::as_str)
    }

    /// Speichert (oder ersetzt) den Fingerprint und schreibt die Datei sofort.
    pub fn trust(&mut self, name: &str, fingerprint: &Fingerprint) -> Result<()> {
        self.peers.insert(name.to_owned(), fingerprint.to_string());
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let text = toml::to_string(&File { peers: self.peers.clone() })?;
            fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identity;

    #[test]
    fn trust_is_checked_and_persisted() {
        let path = std::env::temp_dir().join(format!("qross-trust-test-{}.toml", std::process::id()));
        let a = Identity::generate().unwrap().fingerprint();
        let b = Identity::generate().unwrap().fingerprint();

        let mut store = TrustStore::load(&path).unwrap();
        assert_eq!(store.check("laptop", &a), TrustOutcome::Unknown);
        store.trust("laptop", &a).unwrap();

        let reloaded = TrustStore::load(&path).unwrap();
        assert_eq!(reloaded.check("laptop", &a), TrustOutcome::Known);
        assert_eq!(reloaded.check("laptop", &b), TrustOutcome::Mismatch);
        let _ = fs::remove_file(&path);
    }
}
