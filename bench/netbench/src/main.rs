//! netbench – misst, wie sich Paketverlust auf KVM-Eingabeverkehr auswirkt:
//! TCP+TLS mit einem gemeinsamen Stream (wie Synergy) gegen QUIC.

mod netem;
mod quic;
mod relay;
mod stats;
mod tcp;
mod tls;
mod wire;
mod workload;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, ValueEnum};

use stats::Report;
use workload::{Clock, Recorder, Workload};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// TCP + TLS, ein Stream für alles (wie Synergy/Deskflow)
    Tcp,
    /// QUIC, ein Stream für alles
    QuicStream,
    /// QUIC, Maus als Datagramm, Tasten und Clipboard auf getrennten Streams
    Quic,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Tcp => "TCP+TLS (Synergy-Stil)",
            Mode::QuicStream => "QUIC, 1 Stream",
            Mode::Quic => "QUIC, Datagramme+Streams",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Link {
    /// Direkt über Loopback, ohne Störungen
    Direct,
    /// UDP-Relay im Userspace (nur QUIC, läuft ohne Admin-Rechte)
    Relay,
    /// Linux `tc netem` auf `lo` (TCP und QUIC, braucht NET_ADMIN)
    Netem,
}

#[derive(Parser, Debug)]
#[command(about = "Vergleicht TCP+TLS und QUIC für KVM-Eingaben bei Paketverlust")]
struct Cli {
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = [Mode::Tcp, Mode::QuicStream, Mode::Quic])]
    modes: Vec<Mode>,
    #[arg(long, value_enum, default_value_t = Link::Relay)]
    link: Link,
    /// Paketverlust je Richtung in Prozent (Liste)
    #[arg(long, value_delimiter = ',', default_values_t = [0.0, 1.0, 5.0])]
    loss: Vec<f64>,
    /// Einweg-Verzögerung in ms
    #[arg(long, default_value_t = 1.0)]
    delay_ms: f64,
    /// Jitter (±) in ms
    #[arg(long, default_value_t = 0.3)]
    jitter_ms: f64,
    /// Bandbreitengrenze in Mbit/s, nur mit --link netem (0 = unbegrenzt)
    #[arg(long, default_value_t = 0)]
    rate_mbit: u32,
    /// Messdauer je Durchlauf in Sekunden
    #[arg(long, default_value_t = 20)]
    duration_s: u64,
    #[arg(long, default_value_t = 500)]
    mouse_hz: u32,
    #[arg(long, default_value_t = 20)]
    key_hz: u32,
    /// Größe einer Clipboard-Übertragung in Bytes (0 = aus)
    #[arg(long, default_value_t = 0)]
    clipboard_bytes: usize,
    #[arg(long, default_value_t = 5)]
    clipboard_interval_s: u64,
    /// Ergebnisse zusätzlich als CSV anhängen
    #[arg(long)]
    csv: Option<PathBuf>,
    /// Congestion-Controller für QUIC
    #[arg(long, value_enum, default_value_t = tls::CongestionControl::Cubic)]
    cc: tls::CongestionControl,
    /// QUIC: jedes Paket sofort bestätigen (ACK-Frequency-Erweiterung)
    #[arg(long)]
    fast_ack: bool,
}

pub struct RunConfig {
    pub workload: Workload,
    /// Wartezeit nach dem letzten Ereignis, damit Nachzügler noch ankommen
    pub grace: Duration,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = tls::CONGESTION.set(cli.cc);
    let _ = tls::FAST_ACK.set(cli.fast_ack);
    if cli.fast_ack {
        println!("QUIC: sofortige ACKs (ACK-Frequency-Erweiterung)");
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(windows)]
    unsafe {
        // 1-ms-Timerauflösung, sonst schlafen Timer unter Windows in 15,6-ms-Schritten.
        windows_sys::Win32::Media::timeBeginPeriod(1);
    }

    let cfg = RunConfig {
        workload: Workload {
            duration: Duration::from_secs(cli.duration_s),
            mouse_hz: cli.mouse_hz,
            key_hz: cli.key_hz,
            clipboard_bytes: cli.clipboard_bytes,
            clipboard_interval: Duration::from_secs(cli.clipboard_interval_s),
        },
        grace: Duration::from_secs(3),
    };
    let losses = if cli.link == Link::Direct { vec![0.0] } else { cli.loss.clone() };

    println!(
        "QUIC-CC: {:?}, Link: {:?}, Verzögerung {} ms ± {} ms, Bandbreite {}, Maus {} Hz, Tasten {} Hz, Clipboard {} B, {} s je Lauf\n",
        cli.cc,
        cli.link,
        cli.delay_ms,
        cli.jitter_ms,
        if cli.rate_mbit > 0 { format!("{} Mbit/s", cli.rate_mbit) } else { "unbegrenzt".into() },
        cli.mouse_hz,
        cli.key_hz,
        cli.clipboard_bytes,
        cli.duration_s
    );
    print_header();

    let mut rows = Vec::new();
    for &loss in &losses {
        if cli.link == Link::Netem {
            netem::apply(loss, cli.delay_ms, cli.jitter_ms, cli.rate_mbit)?;
        }
        for &mode in &cli.modes {
            if mode == Mode::Tcp && cli.link == Link::Relay {
                eprintln!("(übersprungen: {} – Paketverlust für TCP geht nur mit --link netem)", mode.label());
                continue;
            }
            let impair = (cli.link == Link::Relay).then(|| relay::Impairment {
                loss: loss / 100.0,
                delay: Duration::from_secs_f64(cli.delay_ms / 1000.0),
                jitter: Duration::from_secs_f64(cli.jitter_ms / 1000.0),
            });

            let clock = Clock::new();
            let rec = Arc::new(Recorder::default());
            let result = match mode {
                Mode::Tcp => tcp::run(&cfg, clock, rec.clone()).await,
                Mode::QuicStream => quic::run(&cfg, clock, rec.clone(), true, impair).await,
                Mode::Quic => quic::run(&cfg, clock, rec.clone(), false, impair).await,
            };
            if let Err(e) = result {
                eprintln!("{} bei {loss} % fehlgeschlagen: {e:#}", mode.label());
                continue;
            }
            let report = stats::evaluate(rec.take());
            print_row(mode, loss, &report);
            rows.push((mode, loss, report));
        }
    }
    if cli.link == Link::Netem {
        netem::clear();
    }

    if let Some(path) = &cli.csv {
        write_csv(path, cli.link, &rows)?;
    }
    Ok(())
}

fn print_header() {
    println!(
        "| Modus | Verlust | Cursor p50 | p99 | p99.9 | max | >50 ms | Maus verloren | Taste p50 | p99 | max | Clipboard Ø |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
}

fn print_row(mode: Mode, loss: f64, r: &Report) {
    let clip = if r.clip.n > 0 { format!("{:.1} ms", r.clip.p50) } else { "–".into() };
    println!(
        "| {} | {loss} % | {:.2} | {:.2} | {:.2} | {:.1} | {} | {}/{} | {:.2} | {:.2} | {:.1} | {} |",
        mode.label(),
        r.cursor.p50,
        r.cursor.p99,
        r.cursor.p999,
        r.cursor.max,
        r.stalls_50ms,
        r.mouse_lost,
        r.mouse_sent,
        r.key.p50,
        r.key.p99,
        r.key.max,
        clip,
    );
    if r.keys_missing > 0 || r.clips_missing > 0 {
        eprintln!("  Warnung: {} Tasten, {} Clipboards nicht angekommen", r.keys_missing, r.clips_missing);
    }
}

fn write_csv(path: &PathBuf, link: Link, rows: &[(Mode, f64, Report)]) -> Result<()> {
    let new = !path.exists();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    if new {
        writeln!(
            f,
            "link,mode,loss_pct,cursor_p50_ms,cursor_p99_ms,cursor_p999_ms,cursor_max_ms,stalls_50ms,mouse_lost,mouse_sent,key_p50_ms,key_p99_ms,key_max_ms,keys_missing,clip_p50_ms"
        )?;
    }
    for (mode, loss, r) in rows {
        writeln!(
            f,
            "{link:?},{mode:?},{loss},{:.3},{:.3},{:.3},{:.3},{},{},{},{:.3},{:.3},{:.3},{},{:.3}",
            r.cursor.p50,
            r.cursor.p99,
            r.cursor.p999,
            r.cursor.max,
            r.stalls_50ms,
            r.mouse_lost,
            r.mouse_sent,
            r.key.p50,
            r.key.p99,
            r.key.max,
            r.keys_missing,
            r.clip.p50,
        )?;
    }
    Ok(())
}
