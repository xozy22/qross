//! Windows: Netzwerkkarten samt Gateway über `GetAdaptersAddresses`, Gateway-MAC aus dem
//! Nachbar-Cache (`GetIpNetTable2`, sonst `SendARP`), Name über die Netzwerkliste (NLM), so wie
//! ihn die Windows-Einstellungen zeigen. Bei WLAN ist das meist der WLAN-Name; anders als die
//! WLAN-Schnittstelle selbst verlangt die Netzwerkliste dafür keine Freigabe der Position.

use std::collections::HashMap;
use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetAdaptersAddresses, GetIpNetTable2, SendARP, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST,
    GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211,
    IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH, MIB_IPNET_TABLE2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{NlnsProbe, AF_INET, SOCKADDR_IN, SOCKET_ADDRESS};

use super::{Mac, Network};

pub fn current(names: bool) -> Vec<Network> {
    let adapters = adapters();
    if adapters.is_empty() {
        return Vec::new();
    }
    let neighbors = neighbors();
    let names = if names { network_names() } else { HashMap::new() };
    adapters
        .into_iter()
        .map(|a| {
            let cached = neighbors.get(&(a.index, a.gateway)).copied();
            // Nur Ethernet und WLAN kennen ARP; ein VPN-Gateway würde SendARP bloß warten lassen.
            let gateway_mac = cached.or_else(|| if a.arp { send_arp(a.gateway) } else { None });
            Network {
                name: names.get(&a.guid).cloned(),
                interface: a.friendly_name,
                gateway: a.gateway,
                gateway_mac,
                address: a.address,
                metric: a.metric,
            }
        })
        .collect()
}

struct Adapter {
    /// IPv4-Schnittstellenindex (wie in der Nachbartabelle)
    index: u32,
    /// GUID der Karte, Großbuchstaben ohne Klammern (wie `Debug` von `windows::core::GUID`)
    guid: String,
    friendly_name: String,
    gateway: Ipv4Addr,
    address: Option<(Ipv4Addr, u8)>,
    metric: u32,
    arp: bool,
}

/// Aktive Karten mit IPv4-Gateway
fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    // u64 für die Ausrichtung der Strukturen
    let mut buffer: Vec<u64> = Vec::new();
    for _ in 0..4 {
        buffer.resize((size as usize).div_ceil(8), 0);
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        match unsafe { GetAdaptersAddresses(u32::from(AF_INET), flags, null(), first, &mut size) } {
            NO_ERROR => return unsafe { collect(first) },
            ERROR_BUFFER_OVERFLOW => continue,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// # Safety
/// `first` ist die von `GetAdaptersAddresses` gefüllte Liste.
unsafe fn collect(first: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<Adapter> {
    let mut list = Vec::new();
    let mut next = first;
    while let Some(a) = next.as_ref() {
        next = a.Next;
        if a.OperStatus != IfOperStatusUp || a.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
            continue;
        }
        let mut gateway = None;
        let mut entry = a.FirstGatewayAddress;
        while let Some(g) = entry.as_ref() {
            gateway = gateway.or(ipv4(&g.Address).filter(|ip| !ip.is_unspecified()));
            entry = g.Next;
        }
        let Some(gateway) = gateway else { continue };
        let mut address = None;
        let mut entry = a.FirstUnicastAddress;
        while let Some(u) = entry.as_ref() {
            address = address.or(ipv4(&u.Address).map(|ip| (ip, u.OnLinkPrefixLength)));
            entry = u.Next;
        }
        let guid = if a.AdapterName.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(a.AdapterName.cast()).to_string_lossy().trim_matches(['{', '}']).to_uppercase()
        };
        list.push(Adapter {
            index: a.Anonymous1.Anonymous.IfIndex,
            guid,
            friendly_name: wide(a.FriendlyName),
            gateway,
            address,
            metric: a.Ipv4Metric,
            arp: a.IfType == IF_TYPE_ETHERNET_CSMACD || a.IfType == IF_TYPE_IEEE80211,
        });
    }
    list
}

unsafe fn ipv4(address: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
    let sockaddr = address.lpSockaddr.as_ref()?;
    if sockaddr.sa_family != AF_INET {
        return None;
    }
    let v4 = &*(address.lpSockaddr as *const SOCKADDR_IN);
    // S_addr liegt in Netzwerk-Byte-Reihenfolge im Speicher
    Some(Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes()))
}

unsafe fn wide(text: *const u16) -> String {
    if text.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| *text.add(i) != 0).count();
    String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
}

/// MAC-Adressen aus dem IPv4-Nachbar-Cache je (Schnittstelle, IP)
fn neighbors() -> HashMap<(u32, Ipv4Addr), Mac> {
    let mut map = HashMap::new();
    let mut table: *mut MIB_IPNET_TABLE2 = null_mut();
    if unsafe { GetIpNetTable2(AF_INET, &mut table) } != NO_ERROR || table.is_null() {
        return map;
    }
    unsafe {
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        for row in rows {
            // Ab „Probe“ ist eine Adresse bekannt; „veraltet“ (Stale) heißt nur: lange nicht bestätigt.
            if row.PhysicalAddressLength != 6 || row.State < NlnsProbe {
                continue;
            }
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&row.PhysicalAddress[..6]);
            let mac = Mac(mac);
            if mac.usable() {
                let ip = Ipv4Addr::from(row.Address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes());
                map.insert((row.InterfaceIndex, ip), mac);
            }
        }
        FreeMibTable(table as *const c_void);
    }
    map
}

/// Fragt die MAC per ARP ab (ohne Administratorrechte). Wartet, falls niemand antwortet.
fn send_arp(ip: Ipv4Addr) -> Option<Mac> {
    let mut buffer = [0u8; 8];
    let mut len = buffer.len() as u32;
    let result = unsafe { SendARP(u32::from_ne_bytes(ip.octets()), 0, buffer.as_mut_ptr().cast(), &mut len) };
    if result != NO_ERROR || len < 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&buffer[..6]);
    Some(Mac(mac)).filter(Mac::usable)
}

/// Namen der verbundenen Netzwerke je Karten-GUID aus der Netzwerkliste von Windows
fn network_names() -> HashMap<String, String> {
    use windows::Win32::Networking::NetworkListManager::{INetworkListManager, NetworkListManager};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

    let mut names = HashMap::new();
    unsafe {
        // Auch ein bereits anders eingerichteter Thread (RPC_E_CHANGED_MODE) kann COM benutzen.
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<()> {
            let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
            let connections = manager.GetNetworkConnections()?;
            loop {
                let mut item = [None];
                let mut fetched = 0u32;
                connections.Next(&mut item, Some(&mut fetched))?;
                let Some(connection) = item[0].take().filter(|_| fetched == 1) else { break };
                let (Ok(adapter), Ok(network)) = (connection.GetAdapterId(), connection.GetNetwork()) else { continue };
                if let Ok(name) = network.GetName() {
                    names.insert(format!("{adapter:?}"), name.to_string());
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            tracing::debug!("cannot read the network list: {e}");
        }
        if initialized {
            CoUninitialize();
        }
    }
    names
}
