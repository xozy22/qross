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

pub(crate) fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    // Verbindungsabbrüche schnell erkennen, damit der Server die Eingaben zurückholt: Jede Seite meldet
    // sich spätestens nach 1 s, nach 5 s Stille gilt die Verbindung als tot. Nicht kürzer: WLAN hakt
    // (Kanalsuche, AirDrop, Energiesparen) gern 1–2 s, ohne dass etwas kaputt ist, und ein falscher
    // Abbruch nimmt dem Nutzer den Cursor weg. Nach dem Aufwachen, vor dem Ruhezustand und bei einem
    // Netzwechsel warten Client und Server ohnehin nicht darauf, sondern prüfen selbst (siehe `wake`).
    t.keep_alive_interval(Some(Duration::from_secs(1)));
    t.max_idle_timeout(Some(Duration::from_secs(5).try_into().expect("valid timeout")));
    // Laufzeit, bis die erste Messung da ist. Davon hängt ab, wann quinn ein verlorenes Paket des
    // Verbindungsaufbaus wiederholt: mit den üblichen 333 ms erst nach 1 s, dann nach 3 s, mit 100 ms
    // nach 0,3 s und 0,9 s. Gerade nach dem Aufwachen und einem Netzwechsel geht das erste Paket gern
    // verloren. Im LAN liegt die Laufzeit weit darunter; über ein langsames VPN kostet zu frühes
    // Wiederholen nur ein Paket.
    t.initial_rtt(Duration::from_millis(100));
    t.datagram_receive_buffer_size(Some(1 << 20));
    // Mausbewegungen, die nicht gleich hinauskönnen (Funkloch, volle Leitung), nur 4 KB lang aufheben:
    // rund 100 Datagramme, bei 1000 Hz eine Zehntelsekunde; ältere verdrängt die neueste. Mit quinns
    // Standard von 1 MB waren es über 20 s, und nach einem Hänger fuhr der Cursor drüben die alte Bahn
    // nach.
    t.datagram_send_buffer_size(DATAGRAM_SEND_BUFFER);
    t
}

/// Warteschlange für ausgehende Datagramme (siehe `transport`)
const DATAGRAM_SEND_BUFFER: usize = 4 * 1024;

/// Fingerprint des Zertifikats der Gegenseite.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Result<Fingerprint> {
    let identity = conn.peer_identity().context("peer sent no certificate")?;
    let certs = identity
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| anyhow!("unexpected type of peer identity"))?;
    let cert = certs.first().context("empty certificate chain")?;
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;

    /// Der Server im eigenen Thread steht `stall` lang still, sobald der Client verbunden ist (wie ein
    /// Laptop im Funkloch), und liest danach eine Weile alle Datagramme. Liefert ihre Nummern.
    fn stalled_server(identity: &Identity, stall: Duration) -> (std::net::SocketAddr, mpsc::Receiver<Vec<u32>>) {
        let config = server_config(identity).unwrap();
        let (addr_tx, addr) = mpsc::channel();
        let (numbers_tx, numbers) = mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
                addr_tx.send(endpoint.local_addr().unwrap()).unwrap();
                let conn = endpoint.accept().await.unwrap().await.unwrap();
                std::thread::sleep(stall);
                let mut received = Vec::new();
                let reading = async {
                    while let Ok(bytes) = conn.read_datagram().await {
                        received.push(u32::from_be_bytes(bytes[..4].try_into().unwrap()));
                    }
                };
                let _ = tokio::time::timeout(Duration::from_millis(1500), reading).await;
                numbers_tx.send(received).unwrap();
            });
        });
        (addr.recv().unwrap(), numbers)
    }

    #[tokio::test]
    async fn after_a_hang_only_the_latest_moves_arrive() {
        let (server_id, client_id) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (addr, numbers) = stalled_server(&server_id, Duration::from_millis(400));
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(client_config(&client_id).unwrap());
        let conn = endpoint.connect(addr, SERVER_NAME).unwrap().await.unwrap();
        assert!(conn.datagram_send_buffer_space() <= DATAGRAM_SEND_BUFFER);
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Bewegungen, die während des Hängers anfallen
        const MOVES: u32 = 5000;
        let started = Instant::now();
        for n in 0..MOVES {
            conn.send_datagram(n.to_be_bytes().repeat(2).into()).unwrap();
        }
        assert!(started.elapsed() < Duration::from_millis(300), "Senden wartet nie");
        let received = tokio::task::spawn_blocking(move || numbers.recv().unwrap()).await.unwrap();
        assert!(received.contains(&(MOVES - 1)), "die neueste Bewegung kommt an");
        assert!(received.len() < 2500, "{} von {MOVES} alten Bewegungen kamen noch an", received.len());
    }
}
