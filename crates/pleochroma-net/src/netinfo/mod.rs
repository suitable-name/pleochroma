//! The machine's LAN IPv4 addresses, ordered for pairing links and certificate names.
//!
//! [`lan_ipv4`] lists the usable IPv4 address of every interface that is up: the address
//! that carries the default route first, then the other physical adapters, then virtual
//! ones (`Hyper-V`, `WSL`, `Docker`, `VirtualBox`, `VMware`, VPN tunnels). Loopback and link-local
//! (`169.254/16`) addresses never appear.
//!
//! The ordering and classification are pure functions over [`InterfaceEntry`] lists
//! ([`classify_virtual`], [`order_addresses`]), so they are tested without real interfaces.
//! Only [`lan_ipv4`] touches the system: it enumerates interfaces with `if-addrs` and
//! finds the default-route address with a UDP socket "connected" to a public address
//! (connecting a UDP socket sends no packet; it only selects the outgoing address).

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// One usable LAN IPv4 address of this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanAddress {
    /// The IPv4 address.
    pub addr: Ipv4Addr,
    /// The interface name (the friendly name on Windows).
    pub interface: String,
    /// True when the operating system sends internet-bound traffic from this address.
    pub default_route: bool,
    /// True when the interface name looks like a virtual adapter, bridge or VPN tunnel.
    pub virtual_adapter: bool,
}

/// One interface address before filtering and ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceEntry {
    /// The interface name.
    pub name: String,
    /// The interface's IPv4 address.
    pub addr: Ipv4Addr,
    /// Whether the interface is up.
    pub up: bool,
}

/// Name prefixes (lowercase) of virtual adapters, bridges and tunnels.
const VIRTUAL_PREFIXES: &[&str] = &[
    "docker", "br-", "veth", "virbr", "vboxnet", "vmnet", "tun", "tap", "wg", "utun", "ppp",
    "ipsec", "bridge", "lxc", "lxd", "podman", "cni", "flannel", "zt",
];

/// Name fragments (lowercase) of virtual adapters, bridges and tunnels.
const VIRTUAL_FRAGMENTS: &[&str] = &[
    "vethernet",
    "wsl",
    "hyper-v",
    "default switch",
    "docker",
    "virtualbox",
    "vmware",
    "vmnet",
    "tap-",
    "wireguard",
    "zerotier",
    "tailscale",
    "hamachi",
    "openvpn",
    "nordlynx",
    "virtual",
    "loopback",
];

/// Whether an interface name (or description) looks like a virtual adapter or tunnel.
///
/// Case-insensitive. Physical Wi-Fi and Ethernet names ("Ethernet", "Wi-Fi", "eth0",
/// "wlan0", "en0") are not virtual.
#[must_use]
pub fn classify_virtual(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIRTUAL_PREFIXES.iter().any(|p| lower.starts_with(p))
        || VIRTUAL_FRAGMENTS.iter().any(|f| lower.contains(f))
}

/// Whether an address is usable as a LAN address: not loopback, not link-local, not
/// unspecified, not multicast or broadcast.
const fn usable(addr: Ipv4Addr) -> bool {
    !(addr.is_loopback()
        || addr.is_link_local()
        || addr.is_unspecified()
        || addr.is_multicast()
        || addr.is_broadcast())
}

/// Filters and orders interface entries.
///
/// Down interfaces and unusable addresses are dropped, duplicate addresses collapse to the
/// first entry, and the result is sorted default route first, then physical adapters, then
/// virtual ones. The sort is stable, so the enumeration order is kept within each group.
/// `default_addr` is the local address of the default route, if known.
#[must_use]
pub fn order_addresses(
    entries: &[InterfaceEntry],
    default_addr: Option<Ipv4Addr>,
) -> Vec<LanAddress> {
    let mut out: Vec<LanAddress> = Vec::new();
    for entry in entries {
        if !entry.up || !usable(entry.addr) || out.iter().any(|a| a.addr == entry.addr) {
            continue;
        }
        out.push(LanAddress {
            addr: entry.addr,
            interface: entry.name.clone(),
            default_route: default_addr == Some(entry.addr),
            virtual_adapter: classify_virtual(&entry.name),
        });
    }
    out.sort_by_key(|a| (!a.default_route, a.virtual_adapter));
    out
}

/// The local IPv4 address the system would use to reach the internet, or `None` when
/// the machine has no default route. No packet is sent.
fn default_route_addr() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(v4) if usable(*v4.ip()) => Some(*v4.ip()),
        _ => None,
    }
}

/// Enumerates the system's IPv4 interface addresses. An enumeration failure gives an
/// empty list.
fn system_entries() -> Vec<InterfaceEntry> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            std::net::IpAddr::V4(addr) => Some(InterfaceEntry {
                name: iface.name,
                addr,
                // `if-addrs` lists only addresses of interfaces the system reports as
                // usable; there is no separate down flag to read.
                up: true,
            }),
            std::net::IpAddr::V6(_) => None,
        })
        .collect()
}

/// The usable LAN IPv4 addresses of this machine, default route first, then physical
/// adapters, then virtual ones. Loopback, link-local and down interfaces are excluded.
///
/// When interface enumeration fails or finds nothing but the default-route probe works,
/// the probe address alone is returned. A machine with no network gives an empty list.
#[must_use]
pub fn lan_ipv4() -> Vec<LanAddress> {
    let default_addr = default_route_addr();
    let mut list = order_addresses(&system_entries(), default_addr);
    match default_addr {
        Some(addr) if list.is_empty() => list.push(LanAddress {
            addr,
            interface: String::new(),
            default_route: true,
            virtual_adapter: false,
        }),
        _ => {}
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, addr: [u8; 4], up: bool) -> InterfaceEntry {
        InterfaceEntry {
            name: name.to_string(),
            addr: Ipv4Addr::from(addr),
            up,
        }
    }

    #[test]
    fn classifies_virtual_names() {
        for name in [
            "vEthernet (WSL)",
            "vEthernet (Default Switch)",
            "docker0",
            "br-1a2b3c4d5e6f",
            "VirtualBox Host-Only Network",
            "VMware Network Adapter VMnet8",
            "vmnet1",
            "TAP-Windows Adapter V9",
            "tun0",
            "wg0",
            "utun3",
            "WireGuard Tunnel",
            "veth1234",
            "virbr0",
        ] {
            assert!(classify_virtual(name), "{name} should be virtual");
        }
    }

    #[test]
    fn classifies_physical_names() {
        for name in [
            "Ethernet",
            "Ethernet 2",
            "Wi-Fi",
            "eth0",
            "wlan0",
            "en0",
            "enp3s0",
        ] {
            assert!(!classify_virtual(name), "{name} should be physical");
        }
    }

    #[test]
    fn excludes_loopback_link_local_and_down() {
        let entries = [
            entry("lo", [127, 0, 0, 1], true),
            entry("Ethernet", [169, 254, 10, 20], true),
            entry("Wi-Fi", [192, 168, 1, 5], false),
            entry("Ethernet 2", [192, 168, 1, 6], true),
        ];
        let list = order_addresses(&entries, None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].addr, Ipv4Addr::new(192, 168, 1, 6));
    }

    #[test]
    fn orders_default_then_physical_then_virtual() {
        let entries = [
            entry("vEthernet (WSL)", [172, 20, 0, 1], true),
            entry("Ethernet", [10, 0, 0, 5], true),
            entry("Wi-Fi", [192, 168, 1, 7], true),
            entry("docker0", [172, 17, 0, 1], true),
        ];
        let list = order_addresses(&entries, Some(Ipv4Addr::new(192, 168, 1, 7)));
        let addrs: Vec<Ipv4Addr> = list.iter().map(|a| a.addr).collect();
        assert_eq!(
            addrs,
            vec![
                Ipv4Addr::new(192, 168, 1, 7),
                Ipv4Addr::new(10, 0, 0, 5),
                Ipv4Addr::new(172, 20, 0, 1),
                Ipv4Addr::new(172, 17, 0, 1),
            ]
        );
        assert!(list[0].default_route && !list[0].virtual_adapter);
        assert!(!list[1].default_route && !list[1].virtual_adapter);
        assert!(list[2].virtual_adapter && list[3].virtual_adapter);
    }

    #[test]
    fn no_default_route_keeps_physical_before_virtual() {
        let entries = [
            entry("tun0", [10, 8, 0, 2], true),
            entry("eth0", [192, 168, 0, 9], true),
        ];
        let list = order_addresses(&entries, None);
        assert_eq!(list[0].interface, "eth0");
        assert_eq!(list[1].interface, "tun0");
        assert!(list.iter().all(|a| !a.default_route));
    }

    #[test]
    fn duplicate_addresses_collapse() {
        let entries = [
            entry("eth0", [192, 168, 0, 9], true),
            entry("eth0:1", [192, 168, 0, 9], true),
        ];
        assert_eq!(order_addresses(&entries, None).len(), 1);
    }

    #[test]
    fn empty_list_gives_empty_result() {
        assert!(order_addresses(&[], None).is_empty());
        assert!(order_addresses(&[], Some(Ipv4Addr::new(192, 168, 1, 7))).is_empty());
    }
}
