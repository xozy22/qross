//! Linux: Standard-Routen aus `/proc/net/route`, Gateway-MAC aus `/proc/net/arp`, Namen der
//! Verbindungen über NetworkManager (`nmcli`, falls vorhanden). Alles ohne Root.

use std::fs;
use std::process::Command;

use super::{assemble, parse_nmcli, parse_proc_arp, parse_proc_route, Network};

pub fn current(names: bool) -> Vec<Network> {
    let routes = fs::read_to_string("/proc/net/route").map(|t| parse_proc_route(&t)).unwrap_or_default();
    if routes.is_empty() {
        return Vec::new();
    }
    let arp = fs::read_to_string("/proc/net/arp").map(|t| parse_proc_arp(&t)).unwrap_or_default();
    let mut networks = assemble(routes, &arp);
    if names {
        let connections = Command::new("nmcli")
            .args(["-t", "-f", "DEVICE,STATE,CONNECTION", "device"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| parse_nmcli(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default();
        for network in &mut networks {
            network.name = connections.iter().find(|(device, _)| *device == network.interface).map(|(_, c)| c.clone());
        }
    }
    networks
}
