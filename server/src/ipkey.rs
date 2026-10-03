//! Per-source-address keys for rate limits and per-IP caps.
//!
//! An IPv6 host normally owns a whole /64 (SLAAC, privacy addresses, ISP
//! delegations), so keying a limit by the exact /128 lets one host present
//! 2^64 "different" sources. Every per-IP limit keys IPv6 by its /64 prefix
//! instead. IPv4 (and IPv4-mapped IPv6, which is the same host as the plain
//! IPv4 address) is keyed by the exact address.

use std::net::{IpAddr, Ipv6Addr};

/// The rate-limit / per-IP-cap key of `ip`: IPv4 as is, IPv4-mapped IPv6 as
/// its IPv4 address, other IPv6 masked to its /64.
pub fn ip_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return IpAddr::V4(m);
            }
            let s = v.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
    }
}

/// Do `a` and `b` share a rate-limit key?
#[allow(dead_code)] // not every binary that includes this module uses it
pub fn same_key(a: IpAddr, b: IpAddr) -> bool {
    ip_key(a) == ip_key(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_keyed_by_slash_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert!(same_key(a, b), "one /64 = one host");
        assert!(!same_key(a, c));
        assert_eq!(ip_key(a), "2001:db8:1:2::".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn ipv4_exact_and_mapped_is_ipv4() {
        let a: IpAddr = "203.0.113.5".parse().unwrap();
        let b: IpAddr = "203.0.113.6".parse().unwrap();
        let m: IpAddr = "::ffff:203.0.113.5".parse().unwrap();
        assert!(!same_key(a, b));
        assert!(same_key(a, m));
        assert_eq!(ip_key(a), a);
    }
}
