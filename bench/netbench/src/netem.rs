//! Paketverlust im Linux-Kernel über `tc netem` auf dem Loopback-Interface.
//! Wirkt auf TCP und UDP gleichermaßen und ist damit der faire Vergleich.
//! Braucht root bzw. CAP_NET_ADMIN (z. B. `docker run --cap-add NET_ADMIN`).

use std::process::Command;

use anyhow::{bail, Context, Result};

/// `rate_mbit = 0` lässt die Bandbreite unbegrenzt.
pub fn apply(loss_pct: f64, delay_ms: f64, jitter_ms: f64, rate_mbit: u32) -> Result<()> {
    // Loopback hat 64-KiB-MTU; echte Netze (Ethernet/WLAN) haben 1500.
    run("ip", &["link".into(), "set".into(), "lo".into(), "mtu".into(), "1500".into()])?;

    let mut args = vec![
        "qdisc".to_string(),
        "replace".into(),
        "dev".into(),
        "lo".into(),
        "root".into(),
        "netem".into(),
        "delay".into(),
        format!("{delay_ms}ms"),
        format!("{jitter_ms}ms"),
        "loss".into(),
        format!("{loss_pct}%"),
    ];
    if rate_mbit > 0 {
        args.extend(["rate".into(), format!("{rate_mbit}mbit")]);
    }
    run("tc", &args)
}

fn run(cmd: &str, args: &[String]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("{cmd} nicht gefunden (iproute2)"))?;
    if !status.success() {
        bail!("{cmd} {} fehlgeschlagen – läuft der Prozess mit NET_ADMIN?", args.join(" "));
    }
    Ok(())
}

pub fn clear() {
    let _ = Command::new("tc").args(["qdisc", "del", "dev", "lo", "root"]).status();
}
