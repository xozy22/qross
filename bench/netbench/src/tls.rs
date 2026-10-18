//! Selbstsigniertes Zertifikat und TLS-/QUIC-Konfigurationen für beide Seiten.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::RootCertStore;

pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    pub fn self_signed() -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
        Ok(Self {
            cert: ck.cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()).into(),
        })
    }

    fn roots(&self) -> Result<RootCertStore> {
        let mut roots = RootCertStore::empty();
        roots.add(self.cert.clone())?;
        Ok(roots)
    }

    pub fn tcp_server(&self) -> Result<rustls::ServerConfig> {
        Ok(rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())?)
    }

    pub fn tcp_client(&self) -> Result<rustls::ClientConfig> {
        Ok(rustls::ClientConfig::builder()
            .with_root_certificates(self.roots()?)
            .with_no_client_auth())
    }

    pub fn quic_server(&self) -> Result<quinn::ServerConfig> {
        let mut config = quinn::ServerConfig::with_single_cert(vec![self.cert.clone()], self.key.clone_key())?;
        config.transport_config(Arc::new(transport()));
        Ok(config)
    }

    pub fn quic_client(&self) -> Result<quinn::ClientConfig> {
        let mut config = quinn::ClientConfig::with_root_certificates(Arc::new(self.roots()?))?;
        config.transport_config(Arc::new(transport()));
        Ok(config)
    }
}

/// Congestion-Controller für QUIC (per `--cc` gewählt).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CongestionControl {
    Cubic,
    Bbr,
    NewReno,
}

pub static CONGESTION: std::sync::OnceLock<CongestionControl> = std::sync::OnceLock::new();
/// ACK-Frequency-Erweiterung: jedes Paket sofort bestätigen (schnellere Verlusterkennung)
pub static FAST_ACK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    match CONGESTION.get().copied().unwrap_or(CongestionControl::Cubic) {
        CongestionControl::Cubic => {}
        CongestionControl::Bbr => {
            t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        }
        CongestionControl::NewReno => {
            t.congestion_controller_factory(Arc::new(quinn::congestion::NewRenoConfig::default()));
        }
    }
    if FAST_ACK.get().copied().unwrap_or(false) {
        let mut ack = quinn::AckFrequencyConfig::default();
        ack.ack_eliciting_threshold(0u32.into())
            .max_ack_delay(Some(Duration::from_millis(1)))
            .reordering_threshold(1u32.into());
        t.ack_frequency_config(Some(ack));
    }
    t.keep_alive_interval(Some(Duration::from_secs(1)));
    t.max_idle_timeout(Some(Duration::from_secs(15).try_into().unwrap()));
    t.datagram_receive_buffer_size(Some(1 << 20));
    t
}
