//! TLS-1.3-Konfiguration für QUIC mit gegenseitiger Zertifikats-Authentifizierung.
//!
//! Die Verifier akzeptieren jedes Zertifikat, prüfen aber die Handshake-Signatur (Besitz des
//! privaten Schlüssels). Ob dem Zertifikat vertraut wird, entscheidet danach die Anwendung
//! anhand des Fingerprints – siehe `TrustStore`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use qross_proto::ALPN;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::{Fingerprint, Identity};

/// Servername im Handshake. Wird nicht geprüft, muss aber ein gültiger DNS-Name sein.
pub const SERVER_NAME: &str = "qross";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn server_config(identity: &Identity) -> Result<quinn::ServerConfig> {
    let provider = provider();
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AcceptAnyCert(provider)))
        .with_single_cert(vec![identity.cert()], identity.key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    config.transport_config(Arc::new(transport()));
    Ok(config)
}

pub fn client_config(identity: &Identity) -> Result<quinn::ClientConfig> {
    let provider = provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
        .with_client_auth_cert(vec![identity.cert()], identity.key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    config.transport_config(Arc::new(transport()));
    Ok(config)
}

fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    // Verbindungsabbrüche schnell erkennen, damit der Server die Eingaben zurückholt.
    t.keep_alive_interval(Some(Duration::from_secs(1)));
    t.max_idle_timeout(Some(Duration::from_secs(5).try_into().expect("gültiges Timeout")));
    t.datagram_receive_buffer_size(Some(1 << 20));
    t
}

/// Fingerprint des Zertifikats der Gegenseite.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Result<Fingerprint> {
    let identity = conn.peer_identity().context("Gegenseite hat kein Zertifikat gesendet")?;
    let certs = identity
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| anyhow!("unerwarteter Typ der Peer-Identität"))?;
    let cert = certs.first().context("leere Zertifikatskette")?;
    Ok(Fingerprint::of(cert))
}

#[derive(Debug)]
struct AcceptAnyCert(Arc<CryptoProvider>);

impl AcceptAnyCert {
    fn schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }

    fn tls12(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn tls13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
}

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

impl ClientCertVerifier for AcceptAnyCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}
