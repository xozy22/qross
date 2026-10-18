use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use crate::{IDENTITY_CERT_FILE, IDENTITY_KEY_FILE};

/// Zertifikat und Schlüssel dieses Rechners.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Identity {
    /// Neue Identität nur im Speicher (Tests).
    pub fn generate() -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(vec!["qross".to_owned()])?;
        Ok(Self {
            cert: ck.cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()),
        })
    }

    /// Lädt die Identität aus `dir` oder legt sie beim ersten Start an.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let cert_path = dir.join(IDENTITY_CERT_FILE);
        let key_path = dir.join(IDENTITY_KEY_FILE);
        if cert_path.exists() && key_path.exists() {
            let cert = fs::read(&cert_path).with_context(|| format!("{} lesen", cert_path.display()))?;
            let key = fs::read(&key_path).with_context(|| format!("{} lesen", key_path.display()))?;
            return Ok(Self { cert: CertificateDer::from(cert), key: PrivatePkcs8KeyDer::from(key) });
        }
        let identity = Self::generate()?;
        fs::create_dir_all(dir).with_context(|| format!("{} anlegen", dir.display()))?;
        fs::write(&key_path, identity.key.secret_pkcs8_der())?;
        fs::write(&cert_path, identity.cert.as_ref())?;
        tracing::info!("neue Identität in {} angelegt", dir.display());
        Ok(identity)
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.cert)
    }

    pub(crate) fn cert(&self) -> CertificateDer<'static> {
        self.cert.clone()
    }

    pub(crate) fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

/// SHA-256 über das DER-Zertifikat.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of(cert: &[u8]) -> Self {
        Self(Sha256::digest(cert).into())
    }
}

impl fmt::Display for Fingerprint {
    /// Format: `ab12:cd34:…` (16 Vierergruppen)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, pair) in self.0.chunks(2).enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{:02x}{:02x}", pair[0], pair[1])?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl FromStr for Fingerprint {
    type Err = anyhow::Error;

    /// Akzeptiert Groß-/Kleinschreibung und beliebige `:`/Leerzeichen als Trenner.
    fn from_str(s: &str) -> Result<Self> {
        let hex: String = s.chars().filter(|c| !matches!(c, ':' | ' ' | '-')).collect();
        if hex.len() != 64 {
            bail!("Fingerprint muss 64 Hex-Zeichen haben, hat {}", hex.len());
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).context("ungültiges Hex-Zeichen im Fingerprint")?;
        }
        Ok(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_roundtrips_through_text() {
        let fp = Identity::generate().unwrap().fingerprint();
        let text = fp.to_string();
        assert_eq!(text.len(), 64 + 15);
        assert_eq!(text.parse::<Fingerprint>().unwrap(), fp);
        assert_eq!(text.to_uppercase().replace(':', " ").parse::<Fingerprint>().unwrap(), fp);
        assert!("abcd".parse::<Fingerprint>().is_err());
    }

    #[test]
    fn identity_is_persisted() {
        let dir = std::env::temp_dir().join(format!("qross-identity-test-{}", std::process::id()));
        let first = Identity::load_or_create(&dir).unwrap().fingerprint();
        let second = Identity::load_or_create(&dir).unwrap().fingerprint();
        assert_eq!(first, second);
        let _ = fs::remove_dir_all(&dir);
    }
}
