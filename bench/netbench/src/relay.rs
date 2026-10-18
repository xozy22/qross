//! UDP-Relay im Userspace, das Paketverlust, Verzögerung und Jitter simuliert.
//! Funktioniert ohne Admin-Rechte, aber nur für UDP (also QUIC). Für TCP wird
//! `netem` im Linux-Kernel gebraucht (siehe `--link netem`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::RngExt;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct Impairment {
    /// Verlustwahrscheinlichkeit je Paket und Richtung (0.0 bis 1.0)
    pub loss: f64,
    /// Einweg-Verzögerung
    pub delay: Duration,
    /// Gleichverteilter Jitter (±)
    pub jitter: Duration,
}

impl Impairment {
    fn sample_delay(&self, rng: &mut impl RngExt) -> Duration {
        let jitter = self.jitter.as_secs_f64();
        let offset = if jitter > 0.0 { rng.random_range(-jitter..=jitter) } else { 0.0 };
        Duration::from_secs_f64((self.delay.as_secs_f64() + offset).max(0.0))
    }
}

pub struct Relay {
    pub addr: SocketAddr,
    pub task: JoinHandle<()>,
}

pub async fn spawn(server: SocketAddr, imp: Impairment) -> Result<Relay> {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let addr = sock.local_addr()?;
    let task = tokio::spawn(async move {
        let mut client: Option<SocketAddr> = None;
        // Je Richtung FIFO: Jitter verändert die Abstände, aber nicht die Reihenfolge.
        let mut last_delivery = [Instant::now(), Instant::now()];
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = match sock.recv_from(&mut buf).await {
                Ok(x) => x,
                // Windows meldet ICMP-"Port unreachable" als ConnectionReset; ignorieren.
                Err(_) => continue,
            };
            let (to, dir) = if from == server {
                match client {
                    Some(c) => (c, 1),
                    None => continue,
                }
            } else {
                client = Some(from);
                (server, 0)
            };

            let (dropped, delay) = {
                let mut rng = rand::rng();
                (rng.random_bool(imp.loss.clamp(0.0, 1.0)), imp.sample_delay(&mut rng))
            };
            if dropped {
                continue;
            }

            let deliver_at = (Instant::now() + delay).max(last_delivery[dir]);
            last_delivery[dir] = deliver_at;
            let data = buf[..n].to_vec();
            let sock = sock.clone();
            tokio::spawn(async move {
                tokio::time::sleep_until(deliver_at).await;
                let _ = sock.send_to(&data, to).await;
            });
        }
    });
    Ok(Relay { addr, task })
}
