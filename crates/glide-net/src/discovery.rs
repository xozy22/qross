//! Server-Suche im LAN: Der Client schickt einen UDP-Broadcast an `DISCOVERY_PORT`, jeder
//! Glide-Server antwortet mit Name, Port und Fingerprint. Dann muss niemand IP-Adressen
//! eintippen. Die Antwort ist unauthentifiziert und dient nur als Vorschlag – vertraut wird
//! erst beim Verbinden über den Fingerprint (TOFU bzw. Vorgabe).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::Result;
use glide_proto::{Discovery, Platform, DISCOVERY_PORT, PROTOCOL_VERSION};
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// Ein gefundener Server
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub name: String,
    pub addr: SocketAddr,
    /// Vom Server angegeben – beim Verbinden wird er tatsächlich geprüft
    pub fingerprint: String,
    pub platform: Platform,
}

/// Beantwortet Suchanfragen, bis die Aufgabe abgebrochen wird.
pub async fn respond(name: String, port: u16, fingerprint: String) {
    match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await {
        Ok(socket) => serve(socket, name, port, fingerprint).await,
        Err(e) => warn!("Server-Suche im LAN nicht verfügbar (UDP {DISCOVERY_PORT}): {e}"),
    }
}

async fn serve(socket: UdpSocket, name: String, port: u16, fingerprint: String) {
    let announce = postcard::to_stdvec(&Discovery::Announce {
        version: PROTOCOL_VERSION,
        name,
        port,
        fingerprint,
        platform: Platform::current(),
    })
    .expect("serialisierbar");
    let mut buf = [0u8; 512];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
        if let Ok(Discovery::Probe { .. }) = postcard::from_bytes(&buf[..n]) {
            debug!("Suchanfrage von {from}");
            let _ = socket.send_to(&announce, from).await;
        }
    }
}

/// Sucht `wait` lang nach Servern – auf jeder IPv4-Schnittstelle per Broadcast und lokal.
pub async fn discover(wait: Duration) -> Result<Vec<DiscoveredServer>> {
    discover_on(wait, DISCOVERY_PORT, true).await
}

async fn discover_on(wait: Duration, port: u16, lan: bool) -> Result<Vec<DiscoveredServer>> {
    let probe = postcard::to_stdvec(&Discovery::Probe { version: PROTOCOL_VERSION })?;
    // Pro Schnittstelle ein eigener Socket: Ein Broadcast an 255.255.255.255 verlässt unter
    // Windows sonst nur die Standard-Schnittstelle (z. B. den WSL-Adapter statt des LANs).
    let mut targets: Vec<(Ipv4Addr, Ipv4Addr)> = vec![(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST)];
    let interfaces = if lan { if_addrs::get_if_addrs().unwrap_or_default() } else { Vec::new() };
    for iface in interfaces {
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            if !v4.ip.is_loopback() {
                targets.push((v4.ip, v4.broadcast.unwrap_or(Ipv4Addr::BROADCAST)));
            }
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tasks = Vec::new();
    for (local, broadcast) in targets {
        let (probe, tx) = (probe.clone(), tx.clone());
        tasks.push(tokio::spawn(async move {
            let Ok(socket) = UdpSocket::bind((local, 0)).await else { return };
            let _ = socket.set_broadcast(true);
            for dest in [broadcast, Ipv4Addr::BROADCAST] {
                let _ = socket.send_to(&probe, (dest, port)).await;
            }
            let mut buf = [0u8; 512];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                if let Ok(Discovery::Announce { name, port, fingerprint, platform, .. }) = postcard::from_bytes(&buf[..n]) {
                    let _ = tx.send(DiscoveredServer { name, addr: SocketAddr::new(from.ip(), port), fingerprint, platform });
                }
            }
        }));
    }
    drop(tx);

    // Je Server (Fingerprint) die passendste Adresse – er antwortet über jede Schnittstelle
    let mut found: BTreeMap<String, DiscoveredServer> = BTreeMap::new();
    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            Some(server) = rx.recv() => {
                let better = found
                    .get(&server.fingerprint)
                    .is_none_or(|known| address_rank(server.addr.ip()) > address_rank(known.addr.ip()));
                if better {
                    found.insert(server.fingerprint.clone(), server);
                }
            }
            _ = &mut deadline => break,
        }
    }
    for task in tasks {
        task.abort();
    }
    Ok(found.into_values().collect())
}

/// Wie gut sich eine Adresse für die Verbindung eignet: echte LAN-Netze vor typischen virtuellen
/// Adaptern (WSL, Hyper-V und Docker nutzen meist 172.16.0.0/12), Link-Local und Loopback zuletzt.
fn address_rank(ip: IpAddr) -> u8 {
    let IpAddr::V4(v4) = ip else { return 1 };
    let [a, b, ..] = v4.octets();
    match (a, b) {
        _ if v4.is_loopback() => 0,
        _ if v4.is_link_local() => 1,
        (172, 16..=31) => 2,
        (192, 168) | (10, _) => 4,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_addresses_win_over_virtual_adapters() {
        let rank = |s: &str| address_rank(s.parse().unwrap());
        assert!(rank("192.168.10.7") > rank("172.28.240.1"), "LAN vor WSL/Hyper-V");
        assert!(rank("10.0.0.5") > rank("172.17.0.1"), "LAN vor Docker");
        assert!(rank("172.28.240.1") > rank("169.254.3.4"));
        assert!(rank("169.254.3.4") > rank("127.0.0.1"));
        assert!(rank("192.168.1.2") > rank("8.8.8.8"));
    }

    #[tokio::test]
    async fn server_is_found_via_loopback() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let responder = tokio::spawn(serve(socket, "pc".into(), 24900, "ab12:cd34".into()));

        let found = discover_on(Duration::from_millis(300), port, false).await.unwrap();
        responder.abort();
        assert_eq!(
            found,
            vec![DiscoveredServer {
                name: "pc".into(),
                addr: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 24900),
                fingerprint: "ab12:cd34".into(),
                platform: Platform::current(),
            }]
        );
    }
}
