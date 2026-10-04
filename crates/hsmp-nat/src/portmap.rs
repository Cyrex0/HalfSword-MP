//! Open the server's UDP port on the home router: UPnP-IGD first, then PCP, then NAT-PMP.
//! A mapping has a lease that [`renew`] refreshes and [`unmap`] removes on a clean shutdown.
//!
//! Every endpoint comes from [`Targets`]: the real network (`Targets::system`) or mocks on
//! 127.0.0.1 in the tests. Nothing here retries forever; the caller decides when to try again.

use crate::{gateway, natpmp, pcp, upnp};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tracing::debug;

/// Where the mapping protocols are spoken.
#[derive(Debug, Clone)]
pub struct Targets {
    /// SSDP search address (the multicast group, or a test responder).
    pub ssdp: SocketAddr,
    pub ssdp_wait: Duration,
    /// The NAT-PMP / PCP server (the default gateway, port 5351).
    pub pmp: Option<SocketAddr>,
    /// This host's LAN address (the mapping's internal client).
    pub local_ip: Option<Ipv4Addr>,
    /// Per-attempt timeouts of the PCP / NAT-PMP exchange (doubled each retry).
    pub pmp_first_wait: Duration,
    pub pmp_tries: u32,
}

impl Targets {
    /// The real network: SSDP multicast, the default gateway's port 5351.
    pub fn system() -> Targets {
        let gw = gateway::default_gateway();
        let local_ip = gw.and_then(gateway::local_ip_towards).or_else(|| gateway::local_ip_towards(Ipv4Addr::new(1, 1, 1, 1)));
        Targets {
            ssdp: upnp::SSDP_ADDR.parse().expect("constant"),
            ssdp_wait: Duration::from_millis(2500),
            pmp: gw.map(|g| SocketAddr::from((g, natpmp::PORT))),
            local_ip,
            pmp_first_wait: Duration::from_millis(250),
            pmp_tries: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Upnp(upnp::WanService),
    Pcp { server: SocketAddr, nonce: pcp::Nonce },
    NatPmp { server: SocketAddr },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub method: Method,
    pub internal: u16,
    pub external: u16,
    /// The router's WAN address, when it told us. A private one means a second NAT
    /// (CGNAT, or a router behind another router): the mapping alone does not make the
    /// server reachable.
    pub external_ip: Option<Ipv4Addr>,
    /// 0 = permanent (UPnP routers that only accept those).
    pub lease_s: u32,
    pub local_ip: Ipv4Addr,
}

impl Mapping {
    pub fn method_name(&self) -> &'static str {
        match self.method {
            Method::Upnp(_) => "upnp",
            Method::Pcp { .. } => "pcp",
            Method::NatPmp { .. } => "natpmp",
        }
    }

    pub fn double_nat(&self) -> bool {
        self.external_ip.is_some_and(gateway::is_private_v4)
    }

    /// When to renew (half the lease; permanent UPnP mappings are re-added every 30 min in
    /// case the router rebooted and forgot them).
    pub fn renew_after(&self) -> Duration {
        if self.lease_s == 0 { Duration::from_secs(1800) } else { Duration::from_secs((self.lease_s as u64 / 2).max(30)) }
    }
}

pub const DESCRIPTION: &str = "HalfSword-MP";

/// Try every method in turn. `Err` lists why each one failed (for the log).
pub async fn map(t: &Targets, port: u16, lease_s: u32) -> Result<Mapping, String> {
    let Some(local_ip) = t.local_ip else { return Err("no LAN address (offline?)".into()) };
    let mut why = Vec::new();
    match map_upnp(t, local_ip, port, lease_s).await {
        Ok(m) => return Ok(m),
        Err(e) => why.push(format!("UPnP: {e}")),
    }
    match t.pmp {
        Some(server) => match map_pcp(t, server, local_ip, port, lease_s, None).await {
            Ok(m) => return Ok(m),
            Err(e) => {
                why.push(format!("PCP: {e}"));
                match map_natpmp(t, server, local_ip, port, lease_s).await {
                    Ok(m) => return Ok(m),
                    Err(e) => why.push(format!("NAT-PMP: {e}")),
                }
            }
        },
        None => why.push("PCP / NAT-PMP: no default gateway".into()),
    }
    Err(why.join("; "))
}

/// Refresh the lease (same method, same ports). An error means the router no longer accepts
/// it: the caller maps again from scratch.
pub async fn renew(t: &Targets, m: &mut Mapping) -> Result<(), String> {
    let fresh = match &m.method {
        Method::Upnp(svc) => add_upnp(svc, m.local_ip, m.internal, m.external, m.lease_s).await.map(|lease| Mapping { lease_s: lease, ..m.clone() }).map_err(|e| e.to_string()),
        Method::Pcp { server, nonce } => map_pcp(t, *server, m.local_ip, m.internal, m.lease_s.max(120), Some((*nonce, m.external))).await,
        Method::NatPmp { server } => map_natpmp(t, *server, m.local_ip, m.internal, m.lease_s.max(120)).await,
    }?;
    if fresh.external != m.external {
        return Err(format!("the router moved the mapping to port {}", fresh.external));
    }
    *m = Mapping { external_ip: fresh.external_ip.or(m.external_ip), ..fresh };
    Ok(())
}

/// Remove the mapping (clean shutdown). Best effort.
pub async fn unmap(t: &Targets, m: &Mapping) {
    let r = match &m.method {
        Method::Upnp(svc) => upnp::soap(svc, "DeletePortMapping", upnp::delete_port_mapping_body(&svc.service_type, m.external)).await.map(|_| ()).map_err(|e| e.to_string()),
        Method::Pcp { server, nonce } => {
            let req = pcp::map_request(m.local_ip, nonce, m.internal, 0, 0);
            exchange(t, *server, m.local_ip, &req).await.map(|_| ())
        }
        Method::NatPmp { server } => exchange(t, *server, m.local_ip, &natpmp::map_request(m.internal, 0, 0)).await.map(|_| ()),
    };
    if let Err(e) = r {
        debug!(error = %e, "port mapping removal failed");
    }
}

async fn map_upnp(t: &Targets, local_ip: Ipv4Addr, port: u16, lease_s: u32) -> Result<Mapping, String> {
    let (svc, _dev) = upnp::discover(t.ssdp, local_ip, t.ssdp_wait).await.ok_or("no UPnP gateway answered")?;
    // Another device holding the port (718) moves us to the next few ports.
    let mut last = String::new();
    for external in [port, port.wrapping_add(1), port.wrapping_add(2), port.wrapping_add(3)] {
        if external < 1024 {
            break;
        }
        match add_upnp(&svc, local_ip, port, external, lease_s).await {
            Ok(lease) => {
                let ip = upnp::soap(&svc, "GetExternalIPAddress", upnp::get_external_ip_body(&svc.service_type))
                    .await
                    .ok()
                    .and_then(|b| upnp::element(&b, "NewExternalIPAddress").and_then(|s| s.parse().ok()));
                return Ok(Mapping { method: Method::Upnp(svc), internal: port, external, external_ip: ip, lease_s: lease, local_ip });
            }
            Err(upnp::SoapError::Upnp(upnp::err::CONFLICT)) => last = format!("port {external} is mapped to another device"),
            Err(upnp::SoapError::Upnp(c)) if c == upnp::err::SAME_PORT_VALUES_REQUIRED && external != port => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(last)
}

/// AddPortMapping; a router that only takes permanent leases (725) gets lease 0. Returns the
/// lease that was accepted.
async fn add_upnp(svc: &upnp::WanService, local_ip: Ipv4Addr, internal: u16, external: u16, lease_s: u32) -> Result<u32, upnp::SoapError> {
    let body = |lease| upnp::add_port_mapping_body(&svc.service_type, external, internal, local_ip, DESCRIPTION, lease);
    match upnp::soap(svc, "AddPortMapping", body(lease_s)).await {
        Ok(_) => Ok(lease_s),
        Err(upnp::SoapError::Upnp(upnp::err::ONLY_PERMANENT_LEASES)) if lease_s != 0 => {
            upnp::soap(svc, "AddPortMapping", body(0)).await.map(|_| 0)
        }
        Err(e) => Err(e),
    }
}

/// One request / answer with retries. Answers from anyone but `server` are ignored.
async fn exchange(t: &Targets, server: SocketAddr, local_ip: Ipv4Addr, req: &[u8]) -> Result<Vec<u8>, String> {
    let bind_ip = if server.ip().is_loopback() { Ipv4Addr::LOCALHOST } else { local_ip };
    let sock = UdpSocket::bind((bind_ip, 0)).await.map_err(|e| e.to_string())?;
    let mut wait = t.pmp_first_wait;
    let mut buf = [0u8; 1100];
    for _ in 0..t.pmp_tries.max(1) {
        sock.send_to(req, server).await.map_err(|e| e.to_string())?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, from))) if from == server => return Ok(buf[..n].to_vec()),
                Ok(Ok(_)) => continue,
                // ICMP unreachable (Windows reports it as a receive error) = nobody listens
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => break,
            }
        }
        wait *= 2;
    }
    Err("no answer".into())
}

async fn map_pcp(t: &Targets, server: SocketAddr, local_ip: Ipv4Addr, port: u16, lease_s: u32, keep: Option<(pcp::Nonce, u16)>) -> Result<Mapping, String> {
    let (nonce, suggest) = keep.unwrap_or_else(|| (rand::random(), port));
    let req = pcp::map_request(local_ip, &nonce, port, suggest, lease_s.max(1));
    let ans = exchange(t, server, local_ip, &req).await?;
    match pcp::parse(&ans) {
        Some(pcp::Reply::Mapped { nonce: n, internal, external, external_ip, lifetime_s }) if n == nonce && internal == port && external != 0 => Ok(Mapping {
            method: Method::Pcp { server, nonce },
            internal,
            external,
            external_ip: (!external_ip.is_unspecified()).then_some(external_ip),
            lease_s: lifetime_s,
            local_ip,
        }),
        Some(pcp::Reply::Error(c)) => Err(format!("result code {c}")),
        Some(pcp::Reply::NatPmpOnly) => Err("gateway speaks NAT-PMP only".into()),
        _ => Err("bad answer".into()),
    }
}

async fn map_natpmp(t: &Targets, server: SocketAddr, local_ip: Ipv4Addr, port: u16, lease_s: u32) -> Result<Mapping, String> {
    let ans = exchange(t, server, local_ip, &natpmp::map_request(port, port, lease_s.max(1))).await?;
    let (external, lifetime_s) = match natpmp::parse(&ans) {
        Some(natpmp::Reply::Mapped { internal, external, lifetime_s }) if internal == port && external != 0 => (external, lifetime_s),
        Some(natpmp::Reply::Error(c)) => return Err(format!("result code {c}")),
        _ => return Err("bad answer".into()),
    };
    let ip = match exchange(t, server, local_ip, &natpmp::external_request()).await.ok().and_then(|a| natpmp::parse(&a)) {
        Some(natpmp::Reply::External { ip }) => Some(ip),
        _ => None,
    };
    Ok(Mapping { method: Method::NatPmp { server }, internal: port, external, external_ip: ip, lease_s: lifetime_s, local_ip })
}

#[cfg(test)]
mod tests {
    //! Mock routers on 127.0.0.1: an SSDP responder + IGD (HTTP), and a PCP / NAT-PMP server.
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    struct Igd {
        /// SOAP actions received, in order (action, body).
        calls: Vec<(String, String)>,
        /// External ports that answer 718.
        taken: Vec<u16>,
        only_permanent: bool,
    }

    /// Start an SSDP responder and an IGD on loopback; returns the SSDP address.
    async fn mock_igd(state: Arc<Mutex<Igd>>) -> SocketAddr {
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hport = http.local_addr().unwrap().port();
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = http.accept().await else { return };
                let st = st.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    // read headers + the declared body
                    loop {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 { break; }
                        buf.extend_from_slice(&chunk[..n]);
                        let t = String::from_utf8_lossy(&buf).to_string();
                        if let Some(h) = t.find("\r\n\r\n") {
                            let len = t[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                            if buf.len() >= h + 4 + len { break; }
                        }
                    }
                    let t = String::from_utf8_lossy(&buf).to_string();
                    let (status, body) = if t.starts_with("GET /rootDesc.xml") {
                        (200, upnp::tests::MINIUPNPD.to_string())
                    } else {
                        let action = t.lines().find_map(|l| l.to_ascii_lowercase().starts_with("soapaction:").then(|| l.split('#').nth(1).unwrap_or("").trim_matches(|c| c == '"' || c == ' ').to_string())).unwrap_or_default();
                        let body = t.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                        let mut g = st.lock().unwrap();
                        g.calls.push((action.clone(), body.clone()));
                        let ext: u16 = upnp::element(&body, "NewExternalPort").and_then(|p| p.parse().ok()).unwrap_or(0);
                        let lease: u32 = upnp::element(&body, "NewLeaseDuration").and_then(|p| p.parse().ok()).unwrap_or(0);
                        let fault = |c: u16| (500, format!("<s:Envelope><s:Body><s:Fault><detail><UPnPError><errorCode>{c}</errorCode></UPnPError></detail></s:Fault></s:Body></s:Envelope>"));
                        match action.as_str() {
                            "AddPortMapping" if g.taken.contains(&ext) => fault(718),
                            "AddPortMapping" if g.only_permanent && lease != 0 => fault(725),
                            "GetExternalIPAddress" => (200, "<s:Envelope><s:Body><u:R><NewExternalIPAddress>203.0.113.40</NewExternalIPAddress></u:R></s:Body></s:Envelope>".into()),
                            _ => (200, "<s:Envelope><s:Body/></s:Envelope>".into()),
                        }
                    };
                    let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        let ssdp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = ssdp.local_addr().unwrap();
        tokio::spawn(async move {
            let mut b = [0u8; 2048];
            loop {
                let Ok((n, from)) = ssdp.recv_from(&mut b).await else { return };
                if String::from_utf8_lossy(&b[..n]).starts_with("M-SEARCH") {
                    let ans = format!("HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nLOCATION: http://127.0.0.1:{hport}/rootDesc.xml\r\n\r\n");
                    let _ = ssdp.send_to(ans.as_bytes(), from).await;
                }
            }
        });
        addr
    }

    /// A PCP server (or, with `natpmp_only`, a NAT-PMP-only one) that maps `port` to
    /// `port + shift`.
    async fn mock_pmp(natpmp_only: bool, shift: u16) -> (SocketAddr, Arc<Mutex<Vec<Vec<u8>>>>) {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = s.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let mut b = [0u8; 1100];
            loop {
                let Ok((n, from)) = s.recv_from(&mut b).await else { return };
                let req = b[..n].to_vec();
                log.lock().unwrap().push(req.clone());
                let ans: Vec<u8> = match (req[0], natpmp_only) {
                    (2, true) => vec![0, 0x80, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0],
                    (2, false) if n == pcp::LEN => {
                        let r: [u8; pcp::LEN] = req.clone().try_into().unwrap();
                        let int = u16::from_be_bytes([r[40], r[41]]);
                        let life = u32::from_be_bytes(r[4..8].try_into().unwrap());
                        pcp::encode_reply(&r, if life == 0 { 0 } else { int + shift }, Ipv4Addr::new(198, 51, 100, 9), life, 0).to_vec()
                    }
                    (0, _) if req.len() == 2 => natpmp::encode_external_reply(1, Ipv4Addr::new(198, 51, 100, 9)).to_vec(),
                    (0, _) if req.len() == 12 => {
                        let int = u16::from_be_bytes([req[4], req[5]]);
                        let life = u32::from_be_bytes(req[8..12].try_into().unwrap());
                        natpmp::encode_map_reply(1, int, if life == 0 { 0 } else { int + shift }, life, 0).to_vec()
                    }
                    _ => continue,
                };
                let _ = s.send_to(&ans, from).await;
            }
        });
        (addr, seen)
    }

    /// A UDP port nobody listens on.
    async fn dead_addr() -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.local_addr().unwrap()
    }

    fn targets(ssdp: SocketAddr, pmp: Option<SocketAddr>) -> Targets {
        Targets {
            ssdp,
            ssdp_wait: Duration::from_millis(400),
            pmp,
            local_ip: Some(Ipv4Addr::LOCALHOST),
            pmp_first_wait: Duration::from_millis(80),
            pmp_tries: 2,
        }
    }

    #[tokio::test]
    async fn upnp_maps_renews_and_removes() {
        let igd = Arc::new(Mutex::new(Igd::default()));
        let t = targets(mock_igd(igd.clone()).await, None);
        let mut m = map(&t, 7777, 3600).await.unwrap();
        assert_eq!((m.method_name(), m.internal, m.external, m.lease_s), ("upnp", 7777, 7777, 3600));
        assert_eq!(m.external_ip, Some(Ipv4Addr::new(203, 0, 113, 40)));
        assert!(!m.double_nat());
        assert_eq!(m.renew_after(), Duration::from_secs(1800));
        renew(&t, &mut m).await.unwrap();
        unmap(&t, &m).await;
        let calls: Vec<String> = igd.lock().unwrap().calls.iter().map(|c| c.0.clone()).collect();
        assert_eq!(calls, ["AddPortMapping", "GetExternalIPAddress", "AddPortMapping", "DeletePortMapping"]);
        let add = &igd.lock().unwrap().calls[0].1;
        assert!(add.contains("<NewInternalClient>127.0.0.1</NewInternalClient>") && add.contains("<NewProtocol>UDP</NewProtocol>"));
    }

    #[tokio::test]
    async fn upnp_conflict_moves_the_port_and_permanent_only_routers_get_lease_0() {
        let igd = Arc::new(Mutex::new(Igd { taken: vec![7777], only_permanent: true, ..Default::default() }));
        let t = targets(mock_igd(igd.clone()).await, None);
        let m = map(&t, 7777, 3600).await.unwrap();
        assert_eq!((m.internal, m.external, m.lease_s), (7777, 7778, 0));
        assert_eq!(m.renew_after(), Duration::from_secs(1800));
        let n = igd.lock().unwrap().calls.iter().filter(|c| c.0 == "AddPortMapping").count();
        assert_eq!(n, 3, "7777 taken, 7778 refused with 725, 7778 permanent");
    }

    #[tokio::test]
    async fn pcp_when_there_is_no_upnp() {
        let (pmp, seen) = mock_pmp(false, 0).await;
        let t = targets(dead_addr().await, Some(pmp));
        let mut m = map(&t, 7777, 3600).await.unwrap();
        assert_eq!((m.method_name(), m.external, m.external_ip), ("pcp", 7777, Some(Ipv4Addr::new(198, 51, 100, 9))));
        renew(&t, &mut m).await.unwrap();
        unmap(&t, &m).await;
        let s = seen.lock().unwrap();
        assert_eq!(s.len(), 3);
        // renewal reuses the nonce (RFC 6887 11.2), the delete asks for lifetime 0
        assert_eq!(s[0][24..36], s[1][24..36]);
        assert_eq!(u32::from_be_bytes(s[2][4..8].try_into().unwrap()), 0);
    }

    #[tokio::test]
    async fn natpmp_when_the_gateway_only_speaks_that() {
        let (pmp, seen) = mock_pmp(true, 3).await;
        let t = targets(dead_addr().await, Some(pmp));
        let m = map(&t, 7777, 3600).await.unwrap();
        assert_eq!((m.method_name(), m.external, m.lease_s), ("natpmp", 7780, 3600));
        assert_eq!(m.external_ip, Some(Ipv4Addr::new(198, 51, 100, 9)));
        unmap(&t, &m).await;
        assert_eq!(seen.lock().unwrap().last().unwrap()[8..12], [0, 0, 0, 0], "lifetime 0 removes");
    }

    #[tokio::test]
    async fn nothing_answers_lists_every_reason() {
        let t = targets(dead_addr().await, Some(dead_addr().await));
        let e = map(&t, 7777, 3600).await.unwrap_err();
        assert!(e.contains("UPnP:") && e.contains("PCP:") && e.contains("NAT-PMP:"), "{e}");
        let t = Targets { local_ip: None, ..t };
        assert!(map(&t, 7777, 3600).await.is_err());
    }

    #[test]
    fn double_nat_is_spotted() {
        let m = Mapping { method: Method::NatPmp { server: "127.0.0.1:1".parse().unwrap() }, internal: 1, external: 1,
                          external_ip: Some(Ipv4Addr::new(100, 70, 0, 1)), lease_s: 60, local_ip: Ipv4Addr::LOCALHOST };
        assert!(m.double_nat());
        assert_eq!(m.renew_after(), Duration::from_secs(30));
    }
}
