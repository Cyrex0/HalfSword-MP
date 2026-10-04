//! The default IPv4 gateway (where NAT-PMP / PCP requests go) and this host's LAN address
//! towards it (the internal client of a mapping).

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// The local IPv4 address the OS would use to reach `to` (no packet is sent: a connected UDP
/// socket only picks a route).
pub fn local_ip_towards(to: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddr::from((to, 9))).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v) if !v.is_unspecified() => Some(v),
        _ => None,
    }
}

/// The next hop of the default route, if there is one (`None` on a host with a public
/// address on-link, or where the lookup is not implemented).
pub fn default_gateway() -> Option<Ipv4Addr> {
    imp::default_gateway().filter(|g| !g.is_unspecified() && !g.is_loopback())
}

/// Private, CGNAT (100.64/10) or link-local: an address that is not reachable from the
/// internet.
pub fn is_private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || (o[0] == 100 && (o[1] & 0xC0) == 64)
}

#[cfg(windows)]
mod imp {
    use std::net::Ipv4Addr;

    /// MIB_IPFORWARDROW (iphlpapi.h): 14 DWORDs, addresses in network byte order.
    #[repr(C)]
    #[derive(Default)]
    struct MibIpForwardRow {
        dest: u32,
        mask: u32,
        policy: u32,
        next_hop: u32,
        if_index: u32,
        kind: u32,
        proto: u32,
        age: u32,
        next_hop_as: u32,
        metric: [u32; 5],
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetBestRoute(dest: u32, source: u32, row: *mut MibIpForwardRow) -> u32;
    }

    pub fn default_gateway() -> Option<Ipv4Addr> {
        let mut row = MibIpForwardRow::default();
        // The route to a public address (nothing is sent).
        let dest = u32::from_ne_bytes([1, 1, 1, 1]);
        // SAFETY: `row` is a valid, writable MIB_IPFORWARDROW for the duration of the call.
        let rc = unsafe { GetBestRoute(dest, 0, &mut row) };
        (rc == 0).then(|| Ipv4Addr::from(row.next_hop.to_ne_bytes()))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::net::Ipv4Addr;

    /// /proc/net/route: `Iface Destination Gateway Flags ...`, hex in host (little-endian) order.
    pub fn parse_proc_route(text: &str) -> Option<Ipv4Addr> {
        text.lines().skip(1).find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 3 || f[1] != "00000000" {
                return None;
            }
            let g = u32::from_str_radix(f[2], 16).ok()?;
            Some(Ipv4Addr::from(g.to_le_bytes()))
        })
    }

    pub fn default_gateway() -> Option<Ipv4Addr> {
        parse_proc_route(&std::fs::read_to_string("/proc/net/route").ok()?)
    }

    #[cfg(test)]
    #[test]
    fn proc_route() {
        let t = "Iface\tDestination\tGateway \tFlags\nwlan0\t0000A8C0\t00000000\t0001\nwlan0\t00000000\t0101A8C0\t0003\n";
        assert_eq!(parse_proc_route(t), Some(Ipv4Addr::new(192, 168, 1, 1)));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub fn default_gateway() -> Option<std::net::Ipv4Addr> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges() {
        for a in ["10.1.2.3", "192.168.0.1", "172.16.5.5", "100.64.0.1", "100.127.255.1", "127.0.0.1", "169.254.1.1"] {
            assert!(is_private_v4(a.parse().unwrap()), "{a}");
        }
        for a in ["8.8.8.8", "100.128.0.1", "203.0.113.1"] {
            assert!(!is_private_v4(a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn local_ip_lookup_does_not_send() {
        // loopback always has a route
        assert_eq!(local_ip_towards(Ipv4Addr::LOCALHOST), Some(Ipv4Addr::LOCALHOST));
        // must not panic whatever the machine's routes are
        let _ = default_gateway();
    }
}
