//! Referenz wie Synergy/Deskflow: eine TCP-Verbindung mit TLS, TCP_NODELAY,
//! alle Nachrichtenarten (Maus, Tasten, Clipboard) auf demselben Stream.

use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::pki_types::ServerName;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::tls::Identity;
use crate::workload::{spawn_generator, Clock, Recorder};
use crate::{wire, RunConfig};

pub async fn run(cfg: &RunConfig, clock: Clock, rec: Arc<Recorder>) -> Result<()> {
    let id = Identity::self_signed()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let acceptor = TlsAcceptor::from(Arc::new(id.tcp_server()?));

    let server_rec = rec.clone();
    let server = tokio::spawn(async move {
        let (sock, _) = listener.accept().await?;
        sock.set_nodelay(true)?;
        let mut tls = acceptor.accept(sock).await?;
        wire::read_frames(&mut tls, clock, &server_rec).await?;
        anyhow::Ok(())
    });

    let sock = TcpStream::connect(addr).await?;
    sock.set_nodelay(true)?;
    let connector = TlsConnector::from(Arc::new(id.tcp_client()?));
    let mut tls = connector
        .connect(ServerName::try_from("localhost")?, sock)
        .await
        .context("TLS-Handshake")?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let generator = spawn_generator(cfg.workload.clone(), clock, tx, rec);
    while let Some(ev) = rx.recv().await {
        wire::write_event(&mut tls, &ev).await?;
    }
    generator.join().expect("Generator-Thread");

    tokio::time::sleep(cfg.grace).await;
    let _ = tls.shutdown().await;
    let _ = tokio::time::timeout(cfg.grace, server).await;
    Ok(())
}
