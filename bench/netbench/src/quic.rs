//! QUIC-Varianten:
//! - `single_stream = true`: alles auf einem Stream (wie TCP, nur über QUIC) – trennt
//!   den Effekt des Transports vom Effekt des Protokolldesigns.
//! - `single_stream = false`: Mausbewegung als unzuverlässiges Datagramm, Tasten auf einem
//!   eigenen zuverlässigen Stream, jede Clipboard-Übertragung auf einem eigenen Stream.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::tls::Identity;
use crate::wire::{self, Kind};
use crate::workload::{spawn_generator, Clock, Recorder};
use crate::{relay, RunConfig};

pub async fn run(
    cfg: &RunConfig,
    clock: Clock,
    rec: Arc<Recorder>,
    single_stream: bool,
    impair: Option<relay::Impairment>,
) -> Result<()> {
    let id = Identity::self_signed()?;
    let server_ep = quinn::Endpoint::server(id.quic_server()?, "127.0.0.1:0".parse()?)?;
    let server_addr = server_ep.local_addr()?;

    let relay = match impair {
        Some(imp) => Some(relay::spawn(server_addr, imp).await?),
        None => None,
    };
    let target: SocketAddr = relay.as_ref().map_or(server_addr, |r| r.addr);

    let server_rec = rec.clone();
    let server = tokio::spawn(async move {
        let incoming = server_ep.accept().await.context("keine eingehende Verbindung")?;
        let conn = incoming.await?;
        if single_stream {
            let mut stream = conn.accept_uni().await?;
            wire::read_frames(&mut stream, clock, &server_rec).await?;
        } else {
            let dgram_conn = conn.clone();
            let dgram_rec = server_rec.clone();
            tokio::spawn(async move {
                while let Ok(data) = dgram_conn.read_datagram().await {
                    let now = clock.now_ns();
                    if let Some(msg) = wire::decode(&data) {
                        dgram_rec.on_message(&msg, now);
                    }
                }
            });
            while let Ok(mut stream) = conn.accept_uni().await {
                let stream_rec = server_rec.clone();
                tokio::spawn(async move {
                    let _ = wire::read_frames(&mut stream, clock, &stream_rec).await;
                });
            }
        }
        // Endpoint bis zum Ende am Leben halten
        drop(server_ep);
        anyhow::Ok(())
    });

    let mut client_ep = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    client_ep.set_default_client_config(id.quic_client()?);
    let conn = client_ep
        .connect(target, "localhost")?
        .await
        .context("QUIC-Handshake")?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let generator = spawn_generator(cfg.workload.clone(), clock, tx, rec);

    if single_stream {
        let mut stream = conn.open_uni().await?;
        while let Some(ev) = rx.recv().await {
            wire::write_event(&mut stream, &ev).await?;
        }
        let _ = stream.finish();
    } else {
        // Tasten: eigener geordneter Stream, unabhängig von Maus und Clipboard.
        let (key_tx, mut key_rx) = mpsc::unbounded_channel();
        let key_conn = conn.clone();
        let key_writer = tokio::spawn(async move {
            let mut stream = key_conn.open_uni().await?;
            while let Some(ev) = key_rx.recv().await {
                wire::write_event(&mut stream, &ev).await?;
            }
            let _ = stream.finish();
            anyhow::Ok(())
        });

        while let Some(ev) = rx.recv().await {
            match ev.kind {
                Kind::Mouse => {
                    // Verlorene Bewegungen werden nicht wiederholt – die nächste überholt sie.
                    let _ = conn.send_datagram(wire::encode_input(&ev).into());
                }
                Kind::Key => {
                    let _ = key_tx.send(ev);
                }
                Kind::Clip => {
                    let clip_conn = conn.clone();
                    tokio::spawn(async move {
                        let mut stream = clip_conn.open_uni().await?;
                        wire::write_event(&mut stream, &ev).await?;
                        let _ = stream.finish();
                        anyhow::Ok(())
                    });
                }
            }
        }
        drop(key_tx);
        let _ = key_writer.await;
    }
    generator.join().expect("Generator-Thread");

    tokio::time::sleep(cfg.grace).await;
    conn.close(0u32.into(), b"fertig");
    let _ = tokio::time::timeout(cfg.grace, server).await;
    client_ep.wait_idle().await;
    if let Some(relay) = relay {
        relay.task.abort();
    }
    Ok(())
}
