//! Source-address keys for limits.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// IPv4-mapped IPv6 is the IPv4 host.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// One host: an IPv4 address, or an IPv6 /64 (a host normally owns a whole /64).
pub fn host_key(ip: IpAddr) -> IpAddr {
    match canonical(ip) {
        IpAddr::V6(v) => {
            let s = v.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// One network: an IPv4 /24 or an IPv6 /48 (a site). Caps per network stop someone with a
/// block of addresses from filling the list.
pub fn net_key(ip: IpAddr) -> IpAddr {
    match canonical(ip) {
        IpAddr::V4(v) => {
            let o = v.octets();
            IpAddr::V4(Ipv4Addr::new(o[0], o[1], o[2], 0))
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn keys() {
        assert_eq!(canonical(ip("::ffff:203.0.113.5")), ip("203.0.113.5"));
        assert_eq!(host_key(ip("2001:db8:1:2:aaaa::1")), ip("2001:db8:1:2::"));
        assert_eq!(host_key(ip("203.0.113.5")), ip("203.0.113.5"));
        assert_eq!(net_key(ip("203.0.113.77")), ip("203.0.113.0"));
        assert_eq!(net_key(ip("2001:db8:1:2::1")), ip("2001:db8:1::"));
        assert_eq!(net_key(ip("::ffff:198.51.100.9")), ip("198.51.100.0"));
    }
}
