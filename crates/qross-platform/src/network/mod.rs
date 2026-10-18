//! Netzwerk, in dem dieser Rechner gerade ist, für Profile je Ort. Erkannt am IPv4-Standard-Gateway
//! (Adresse und, soweit im ARP-Cache, MAC-Adresse) und am eigenen Subnetz. Alles ohne zusätzliche
//! Rechte: keine Administratorrechte, keine Ortungsdienste.
//!
//! Den Namen des Netzwerks liefert nur Windows (Netzwerkliste, bei WLAN meist der WLAN-Name) und
//! Linux mit NetworkManager. macOS gibt den WLAN-Namen ab Version 14 nur mit Ortungsdiensten heraus;
//! Qross fragt nicht danach.

use std::fmt;
use std::net::Ipv4Addr;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use self::win as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use self::linux as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use self::macos as imp;

/// MAC-Adresse, angezeigt als `3c:a6:2f:0a:1b:2c`
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mac(pub [u8; 6]);

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Mac {
    /// Liest `3c:a6:2f:a:1b:2c` (macOS lässt führende Nullen weg) oder `3C-A6-2F-0A-1B-2C`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut bytes = [0u8; 6];
        let mut parts = text.trim().split([':', '-']);
        for byte in &mut bytes {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 2 {
                return None;
            }
            *byte = u8::from_str_radix(part, 16).ok()?;
        }
        (parts.next().is_none()).then_some(Self(bytes)).filter(Mac::usable)
    }

    /// Weder leer noch Broadcast (unvollständige ARP-Einträge stehen als 00:00:00:00:00:00 da)
    pub fn usable(&self) -> bool {
        self.0 != [0; 6] && self.0 != [0xff; 6]
    }
}

/// Ein Netzwerk mit IPv4-Standard-Gateway
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// Netzwerkkarte, z. B. „WLAN“, „wlp2s0“ oder „en0“
    pub interface: String,
    pub gateway: Ipv4Addr,
    /// MAC-Adresse des Gateways; fehlt, solange sie nicht im ARP-Cache steht, und bei VPN
    pub gateway_mac: Option<Mac>,
    /// Eigene Adresse und Präfixlänge in diesem Netz
    pub address: Option<(Ipv4Addr, u8)>,
    /// Name des Netzwerks, wo er ohne Zusatzrechte lesbar ist
    pub name: Option<String>,
    /// Kleiner ist bevorzugt (Metrik der Route bzw. Reihenfolge); 0, wenn unbekannt
    pub metric: u32,
}

impl Network {
    /// Subnetz, z. B. `192.168.178.0/24`
    pub fn subnet(&self) -> Option<String> {
        let (ip, prefix) = self.address?;
        let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix.min(32))) };
        Some(format!("{}/{prefix}", Ipv4Addr::from(u32::from(ip) & mask)))
    }
}

/// Netzwerke mit IPv4-Standard-Gateway, bevorzugtes zuerst. `names`: auch die Namen der Netzwerke
/// ermitteln (unter Linux startet das `nmcli`, deshalb nicht bei jeder Abfrage). Blockiert kurz,
/// unter Windows bis zu einige Sekunden, wenn ein Gateway nicht auf ARP antwortet.
pub fn current(names: bool) -> Vec<Network> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    let mut networks = imp::current(names);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let mut networks: Vec<Network> = {
        let _ = names;
        Vec::new()
    };
    sort(&mut networks);
    networks
}

/// Netzwerke mit bekannter Gateway-MAC (LAN, WLAN) vor solchen ohne (VPN), dann nach Metrik.
fn sort(networks: &mut [Network]) {
    networks.sort_by_key(|n| (n.gateway_mac.is_none(), n.metric));
}

/// Fehlt die MAC eines Gateways im ARP-Cache, löst ein leeres UDP-Paket an den Discard-Port eine
/// ARP-Anfrage aus; bei der nächsten Abfrage steht sie dann dort.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn nudge(gateway: Ipv4Addr) {
    if let Ok(socket) = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        let _ = socket.send_to(&[], (gateway, 9));
    }
}

/// Eigene IPv4-Adresse und Präfixlänge je Netzwerkkarte
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn addresses() -> Vec<(String, Ipv4Addr, u8, bool)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match &iface.addr {
            if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() => {
                Some((iface.name.clone(), v4.ip, v4.prefixlen, iface.is_p2p()))
            }
            _ => None,
        })
        .collect()
}

/// Baut die Netzwerke aus Standard-Routen (Karte, Gateway, Metrik), ARP-Einträgen (Karte, IP, MAC)
/// und Adressen der Karten; Gateways ohne MAC bekommen einen ARP-Anstoß.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn assemble(routes: Vec<(String, Ipv4Addr, u32)>, arp: &[(String, Ipv4Addr, Mac)]) -> Vec<Network> {
    let addresses = addresses();
    let mut networks: Vec<Network> = Vec::new();
    for (interface, gateway, metric) in routes {
        if networks.iter().any(|n| n.interface == interface && n.gateway == gateway) {
            continue;
        }
        let gateway_mac = arp.iter().find(|(i, ip, _)| *i == interface && *ip == gateway).map(|(_, _, mac)| *mac);
        let own = addresses.iter().find(|(name, ..)| *name == interface);
        if gateway_mac.is_none() && own.is_some_and(|(.., p2p)| !p2p) {
            nudge(gateway);
        }
        let address = own.map(|&(_, ip, prefix, _)| (ip, prefix));
        networks.push(Network { interface, gateway, gateway_mac, address, name: None, metric });
    }
    networks
}

/// Standard-Routen aus `/proc/net/route`: (Karte, Gateway, Metrik). Adressen stehen dort als
/// Hexzahl in der Byte-Reihenfolge des Rechners.
#[cfg(any(test, target_os = "linux"))]
fn parse_proc_route(text: &str) -> Vec<(String, Ipv4Addr, u32)> {
    const RTF_UP: u32 = 0x1;
    const RTF_GATEWAY: u32 = 0x2;
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let (iface, dest, gateway, flags, metric, mask) =
                (cols.first()?, hex(cols.get(1)?)?, hex(cols.get(2)?)?, hex(cols.get(3)?)?, cols.get(6)?, hex(cols.get(7)?)?);
            let default = dest == 0 && mask == 0 && flags & (RTF_UP | RTF_GATEWAY) == RTF_UP | RTF_GATEWAY;
            let gateway = Ipv4Addr::from(gateway.to_ne_bytes());
            (default && !gateway.is_unspecified()).then(|| (iface.to_string(), gateway, metric.parse().unwrap_or(0)))
        })
        .collect()
}

/// Vollständige Einträge aus `/proc/net/arp`: (Karte, IP, MAC)
#[cfg(any(test, target_os = "linux"))]
fn parse_proc_arp(text: &str) -> Vec<(String, Ipv4Addr, Mac)> {
    const ATF_COM: u32 = 0x2;
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let flags = u32::from_str_radix(cols.get(2)?.trim_start_matches("0x"), 16).ok()?;
            if flags & ATF_COM == 0 {
                return None;
            }
            Some((cols.get(5)?.to_string(), cols.first()?.parse().ok()?, Mac::parse(cols.get(3)?)?))
        })
        .collect()
}

/// Ausgabe von `nmcli -t -f DEVICE,STATE,CONNECTION device`: Name der Verbindung je verbundener Karte.
/// Im knappen Format stehen Doppelpunkte in Werten als `\:`.
#[cfg(any(test, target_os = "linux"))]
fn parse_nmcli(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = Vec::new();
            let mut field = String::new();
            let mut chars = line.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => field.extend(chars.next()),
                    ':' => fields.push(std::mem::take(&mut field)),
                    c => field.push(c),
                }
            }
            fields.push(field);
            let [device, state, connection] = <[String; 3]>::try_from(fields).ok()?;
            (state == "connected" && !connection.is_empty()).then_some((device, connection))
        })
        .collect()
}

/// Ausgabe von `netstat -rn -f inet` unter macOS: Standard-Routen (Karte, Gateway, Reihenfolge) und
/// ARP-Einträge (Karte, IP, MAC), denn macOS führt den ARP-Cache als Routen mit MAC als Gateway.
#[cfg(any(test, target_os = "macos"))]
#[allow(clippy::type_complexity)]
fn parse_netstat(text: &str) -> (Vec<(String, Ipv4Addr, u32)>, Vec<(String, Ipv4Addr, Mac)>) {
    let mut routes = Vec::new();
    let mut arp = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let (Some(dest), Some(gateway), Some(flags), Some(iface)) = (cols.first(), cols.get(1), cols.get(2), cols.get(3))
        else {
            continue;
        };
        if *dest == "default" {
            if let Ok(gateway) = gateway.parse::<Ipv4Addr>() {
                if flags.contains('G') {
                    routes.push((iface.to_string(), gateway, routes.len() as u32));
                }
            }
        } else if let (Ok(ip), Some(mac)) = (dest.parse::<Ipv4Addr>(), Mac::parse(gateway)) {
            if flags.contains('L') {
                arp.push((iface.to_string(), ip, mac));
            }
        }
    }
    (routes, arp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_addresses_are_read_and_shown_uniformly() {
        let mac = Mac::parse("3c:a6:2f:a:1b:2c").unwrap();
        assert_eq!(mac.to_string(), "3c:a6:2f:0a:1b:2c");
        assert_eq!(Mac::parse("3C-A6-2F-0A-1B-2C"), Some(mac));
        assert_eq!(Mac::parse("00:00:00:00:00:00"), None, "unvollständiger ARP-Eintrag");
        assert_eq!(Mac::parse("3c:a6:2f:0a:1b"), None);
        assert_eq!(Mac::parse("3c:a6:2f:0a:1b:2c:01"), None);
        assert_eq!(Mac::parse("link#6"), None);
    }

    #[test]
    fn subnet_from_address_and_prefix() {
        let net = |address| Network {
            interface: "wlan0".into(),
            gateway: Ipv4Addr::new(192, 168, 178, 1),
            gateway_mac: None,
            address,
            name: None,
            metric: 0,
        };
        assert_eq!(net(Some((Ipv4Addr::new(192, 168, 178, 23), 24))).subnet().as_deref(), Some("192.168.178.0/24"));
        assert_eq!(net(Some((Ipv4Addr::new(10, 1, 200, 7), 16))).subnet().as_deref(), Some("10.1.0.0/16"));
        assert_eq!(net(Some((Ipv4Addr::new(10, 1, 200, 7), 0))).subnet().as_deref(), Some("0.0.0.0/0"));
        assert_eq!(net(None).subnet(), None);
    }

    #[test]
    fn networks_with_known_gateway_come_first() {
        let net = |interface: &str, mac: Option<Mac>, metric| Network {
            interface: interface.into(),
            gateway: Ipv4Addr::new(10, 0, 0, 1),
            gateway_mac: mac,
            address: None,
            name: None,
            metric,
        };
        let mut list = vec![net("vpn", None, 1), net("wlan", Some(Mac([1, 2, 3, 4, 5, 6])), 50), net("lan", Some(Mac([1; 6])), 10)];
        sort(&mut list);
        let order: Vec<&str> = list.iter().map(|n| n.interface.as_str()).collect();
        assert_eq!(order, ["lan", "wlan", "vpn"]);
    }

    #[test]
    fn linux_routes_and_arp_cache() {
        let route = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
            wlp2s0\t00000000\t01B2A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
            wlp2s0\t00B2A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n\
            tun0\t00000000\t00000000\t0001\t0\t0\t50\t00000000\t0\t0\t0\n\
            enp3s0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0\n";
        let routes = parse_proc_route(route);
        let expected_gateway = Ipv4Addr::from(0x01B2_A8C0u32.to_ne_bytes());
        assert_eq!(routes.len(), 2, "nur Standard-Routen über ein Gateway: {routes:?}");
        assert_eq!(routes[0], ("wlp2s0".into(), expected_gateway, 600));
        if cfg!(target_endian = "little") {
            assert_eq!(expected_gateway, Ipv4Addr::new(192, 168, 178, 1));
            assert_eq!(routes[1].1, Ipv4Addr::new(10, 0, 0, 1));
        }

        let arp = "IP address       HW type     Flags       HW address            Mask     Device\n\
            192.168.178.1    0x1         0x2         3c:a6:2f:0a:1b:2c     *        wlp2s0\n\
            192.168.178.30   0x1         0x0         00:00:00:00:00:00     *        wlp2s0\n";
        assert_eq!(
            parse_proc_arp(arp),
            [("wlp2s0".to_string(), Ipv4Addr::new(192, 168, 178, 1), Mac([0x3c, 0xa6, 0x2f, 0x0a, 0x1b, 0x2c]))]
        );
    }

    #[test]
    fn nmcli_names_with_escaped_colons() {
        let out = "wlp2s0:connected:Büro\\:5GHz\nenp3s0:unavailable:\nlo:connected (externally):lo\ndocker0:connected:docker0\n";
        assert_eq!(
            parse_nmcli(out),
            [("wlp2s0".to_string(), "Büro:5GHz".to_string()), ("docker0".to_string(), "docker0".to_string())]
        );
    }

    #[test]
    fn macos_netstat_routes_and_arp_cache() {
        let out = "Routing tables\n\nInternet:\n\
            Destination        Gateway            Flags               Netif Expire\n\
            default            192.168.178.1      UGScg                 en0       \n\
            default            link#17            UCSIg               utun3       \n\
            default            10.0.0.1           UGScIg                en7       \n\
            127                127.0.0.1          UCS                   lo0       \n\
            192.168.178        link#6             UCS                   en0      !\n\
            192.168.178.1/32   link#6             UCS                   en0      !\n\
            192.168.178.1      3c:a6:2f:a:1b:2c   UHLWIir               en0   1189\n\
            192.168.178.44     link#6             UHLWIi                en0      !\n";
        let (routes, arp) = parse_netstat(out);
        assert_eq!(
            routes,
            [("en0".to_string(), Ipv4Addr::new(192, 168, 178, 1), 0), ("en7".to_string(), Ipv4Addr::new(10, 0, 0, 1), 1)]
        );
        assert_eq!(arp, [("en0".to_string(), Ipv4Addr::new(192, 168, 178, 1), Mac([0x3c, 0xa6, 0x2f, 0x0a, 0x1b, 0x2c]))]);
    }

    /// Echte Abfrage auf diesem Rechner (ohne Netzwerk leer); zeigt mit `--nocapture`, was erkannt wird.
    #[test]
    fn current_networks_can_be_queried() {
        let networks = current(true);
        for n in &networks {
            println!(
                "{} über {}: Gateway {} ({}), Subnetz {}, Name {:?}, Metrik {}",
                n.interface,
                n.address.map(|(ip, _)| ip.to_string()).unwrap_or_default(),
                n.gateway,
                n.gateway_mac.map(|m| m.to_string()).unwrap_or_else(|| "MAC unbekannt".into()),
                n.subnet().unwrap_or_default(),
                n.name,
                n.metric
            );
            assert!(!n.gateway.is_unspecified());
        }
    }
}
