//! macOS: Standard-Routen und ARP-Cache aus `netstat -rn -f inet`. Den WLAN-Namen gibt macOS ab
//! Version 14 nur mit Ortungsdiensten heraus; Qross verzichtet darauf.

use std::process::Command;

use super::{assemble, parse_netstat, Network};

pub fn current(_names: bool) -> Vec<Network> {
    let Ok(out) = Command::new("/usr/sbin/netstat").args(["-rn", "-f", "inet"]).output() else {
        return Vec::new();
    };
    let (routes, arp) = parse_netstat(&String::from_utf8_lossy(&out.stdout));
    assemble(routes, &arp)
}
