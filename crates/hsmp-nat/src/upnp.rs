//! UPnP Internet Gateway Device: SSDP discovery, the device description, and the three SOAP
//! actions of WANIPConnection / WANPPPConnection that are used (AddPortMapping,
//! DeletePortMapping, GetExternalIPAddress). The message builders and parsers are pure; the
//! HTTP goes through reqwest (plain HTTP on the LAN, never a proxy).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

pub const SSDP_ADDR: &str = "239.255.255.250:1900";

/// Search targets, most specific first (IGD v1 and v2 both carry WANIPConnection:1/2).
pub const SEARCH_TARGETS: &[&str] = &[
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

pub fn msearch(st: &str, mx_s: u32) -> String {
    format!("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nST: {st}\r\nMAN: \"ssdp:discover\"\r\nMX: {mx_s}\r\n\r\n")
}

/// The LOCATION header of an SSDP answer (an `http://` URL), if it is a 200 answer.
pub fn ssdp_location(resp: &str) -> Option<String> {
    let mut lines = resp.split("\r\n").flat_map(|l| l.split('\n'));
    let status = lines.next()?;
    if !status.starts_with("HTTP/1.") || !status.contains(" 200") {
        return None;
    }
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("location") {
                let v = v.trim();
                return v.to_ascii_lowercase().starts_with("http://").then(|| v.to_string());
            }
        }
    }
    None
}

/// The text of every element named `name` (any namespace prefix), in document order.
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        let local = tag.split_whitespace().next().unwrap_or("");
        let local = local.rsplit(':').next().unwrap_or(local);
        if local.eq_ignore_ascii_case(name) && !tag.ends_with('/') {
            let body = &rest[end + 1..];
            // the matching close tag, prefix or not
            let mut j = 0;
            let mut found = None;
            while let Some(k) = body[j..].find("</") {
                let at = j + k;
                let close = &body[at + 2..];
                let cend = close.find('>').unwrap_or(close.len());
                let cl = &close[..cend];
                let cl = cl.rsplit(':').next().unwrap_or(cl).trim();
                if cl.eq_ignore_ascii_case(name) {
                    found = Some(at);
                    break;
                }
                j = at + 2;
            }
            if let Some(at) = found {
                out.push(body[..at].trim());
            }
        }
        rest = &rest[end + 1..];
    }
    out
}

/// The first `name` element's text.
pub fn element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    elements(xml, name).into_iter().next()
}

/// A WAN connection service found in a device description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WanService {
    pub service_type: String,
    pub control_url: String,
}

/// `base` is the description's own URL (or its URLBase); `rel` an absolute URL, an
/// absolute path or a relative path.
pub fn join_url(base: &str, rel: &str) -> String {
    let rel = rel.trim();
    if rel.to_ascii_lowercase().starts_with("http://") {
        return rel.to_string();
    }
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    let host_end = base[scheme_end..].find('/').map(|i| i + scheme_end).unwrap_or(base.len());
    if rel.starts_with('/') {
        return format!("{}{}", &base[..host_end], rel);
    }
    let dir_end = base.rfind('/').filter(|i| *i >= host_end).map(|i| i + 1).unwrap_or(base.len());
    let dir = &base[..dir_end];
    if dir.ends_with('/') { format!("{dir}{rel}") } else { format!("{dir}/{rel}") }
}

/// The WANIPConnection (preferred) or WANPPPConnection service of an IGD description.
pub fn find_wan_service(xml: &str, location: &str) -> Option<WanService> {
    let base = element(xml, "URLBase").filter(|b| !b.is_empty()).unwrap_or(location).to_string();
    let mut ppp = None;
    for svc in elements(xml, "service") {
        let Some(ty) = element(svc, "serviceType") else { continue };
        let Some(ctl) = element(svc, "controlURL") else { continue };
        let s = WanService { service_type: ty.to_string(), control_url: join_url(&base, ctl) };
        if ty.contains(":WANIPConnection:") {
            return Some(s);
        }
        if ty.contains(":WANPPPConnection:") && ppp.is_none() {
            ppp = Some(s);
        }
    }
    ppp
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn envelope(service_type: &str, action: &str, args: &[(&str, String)]) -> String {
    let mut a = String::new();
    for (k, v) in args {
        a.push_str(&format!("<{k}>{}</{k}>", xml_escape(v)));
    }
    format!(
        "<?xml version=\"1.0\"?>\r\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service_type}\">{a}</u:{action}></s:Body></s:Envelope>\r\n"
    )
}

/// The SOAPAction header value.
pub fn soap_action(service_type: &str, action: &str) -> String {
    format!("\"{service_type}#{action}\"")
}

pub fn add_port_mapping_body(service_type: &str, external: u16, internal: u16, client: Ipv4Addr, desc: &str, lease_s: u32) -> String {
    envelope(service_type, "AddPortMapping", &[
        ("NewRemoteHost", String::new()),
        ("NewExternalPort", external.to_string()),
        ("NewProtocol", "UDP".into()),
        ("NewInternalPort", internal.to_string()),
        ("NewInternalClient", client.to_string()),
        ("NewEnabled", "1".into()),
        ("NewPortMappingDescription", desc.into()),
        ("NewLeaseDuration", lease_s.to_string()),
    ])
}

pub fn delete_port_mapping_body(service_type: &str, external: u16) -> String {
    envelope(service_type, "DeletePortMapping", &[
        ("NewRemoteHost", String::new()),
        ("NewExternalPort", external.to_string()),
        ("NewProtocol", "UDP".into()),
    ])
}

pub fn get_external_ip_body(service_type: &str) -> String {
    envelope(service_type, "GetExternalIPAddress", &[])
}

/// UPnP error codes worth acting on.
pub mod err {
    /// The external port is mapped to another client.
    pub const CONFLICT: u16 = 718;
    /// This router only accepts permanent (lease 0) mappings.
    pub const ONLY_PERMANENT_LEASES: u16 = 725;
    /// External and internal port must be equal.
    pub const SAME_PORT_VALUES_REQUIRED: u16 = 724;
}

/// The `errorCode` of a SOAP fault body.
pub fn soap_error(body: &str) -> Option<u16> {
    element(body, "errorCode").and_then(|c| c.trim().parse().ok())
}

#[derive(Debug)]
pub enum SoapError {
    /// A UPnP error code (`err`).
    Upnp(u16),
    Http(String),
}

impl std::fmt::Display for SoapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoapError::Upnp(c) => write!(f, "UPnP error {c}"),
            SoapError::Http(e) => write!(f, "{e}"),
        }
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(4)).build().unwrap_or_default()
}

/// POST one SOAP action; the response body on 200.
pub async fn soap(svc: &WanService, action: &str, body: String) -> Result<String, SoapError> {
    let r = http()
        .post(&svc.control_url)
        .header("content-type", "text/xml; charset=\"utf-8\"")
        .header("soapaction", soap_action(&svc.service_type, action))
        .body(body)
        .send()
        .await
        .map_err(|e| SoapError::Http(e.to_string()))?;
    let ok = r.status().is_success();
    let status = r.status();
    let text = r.text().await.map_err(|e| SoapError::Http(e.to_string()))?;
    if ok {
        return Ok(text);
    }
    Err(match soap_error(&text) {
        Some(c) => SoapError::Upnp(c),
        None => SoapError::Http(format!("HTTP {status}")),
    })
}

/// Find an IGD: M-SEARCH to `ssdp` (the multicast group, or a test responder) from a socket
/// bound on `local`, then fetch each answering device's description until one has a WAN
/// connection service. Returns the service and the address of the device that answered.
pub async fn discover(ssdp: SocketAddr, local: Ipv4Addr, wait: Duration) -> Option<(WanService, Ipv4Addr)> {
    let sock = tokio::net::UdpSocket::bind((local, 0)).await.ok()?;
    let _ = sock.set_multicast_ttl_v4(2);
    for st in SEARCH_TARGETS {
        let _ = sock.send_to(msearch(st, 2).as_bytes(), ssdp).await;
    }
    let deadline = tokio::time::Instant::now() + wait;
    let mut tried: Vec<String> = Vec::new();
    let mut buf = vec![0u8; 2048];
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        let Ok(Ok((n, from))) = tokio::time::timeout(left, sock.recv_from(&mut buf)).await else { return None };
        let Some(loc) = ssdp_location(&String::from_utf8_lossy(&buf[..n])) else { continue };
        if tried.contains(&loc) || tried.len() >= 8 {
            continue;
        }
        tried.push(loc.clone());
        let Ok(r) = http().get(&loc).send().await else { continue };
        let Ok(xml) = r.text().await else { continue };
        if let Some(svc) = find_wan_service(&xml, &loc) {
            let dev = match from.ip() {
                std::net::IpAddr::V4(v) => v,
                std::net::IpAddr::V6(_) => continue,
            };
            return Some((svc, dev));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A miniupnpd description (UniFi, OpenWrt, pfSense all serve this shape), trimmed.
    pub(crate) const MINIUPNPD: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><specVersion><major>1</major><minor>0</minor></specVersion>
<device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
<controlURL>/ctl/L3F</controlURL></service></serviceList>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
<serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId><controlURL>/ctl/IPConn</controlURL>
<eventSubURL>/evt/IPConn</eventSubURL><SCPDURL>/WANIPCn.xml</SCPDURL></service></serviceList>
</device></deviceList></device></deviceList></device></root>"#;

    #[test]
    fn msearch_and_ssdp_answers() {
        let m = msearch(SEARCH_TARGETS[0], 2);
        assert!(m.starts_with("M-SEARCH * HTTP/1.1\r\n"));
        assert!(m.contains("MAN: \"ssdp:discover\"\r\n") && m.ends_with("\r\n\r\n"));
        let ans = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
                   Location: http://192.168.1.1:5000/rootDesc.xml\r\nSERVER: OpenWRT/18.06 UPnP/1.1 MiniUPnPd/2.1\r\n\r\n";
        assert_eq!(ssdp_location(ans).as_deref(), Some("http://192.168.1.1:5000/rootDesc.xml"));
        assert_eq!(ssdp_location("NOTIFY * HTTP/1.1\r\nLOCATION: http://x/\r\n\r\n"), None);
        assert_eq!(ssdp_location("HTTP/1.1 200 OK\r\nLOCATION: https://x/\r\n\r\n"), None);
    }

    #[test]
    fn description_gives_the_wan_ip_control_url() {
        let s = find_wan_service(MINIUPNPD, "http://192.168.1.1:5000/rootDesc.xml").unwrap();
        assert_eq!(s.service_type, "urn:schemas-upnp-org:service:WANIPConnection:1");
        assert_eq!(s.control_url, "http://192.168.1.1:5000/ctl/IPConn");
    }

    #[test]
    fn ppp_relative_urls_urlbase_and_prefixes() {
        // Fritz!Box style: WANPPPConnection only, a URLBase, relative control path
        let x = r#"<root><URLBase>http://192.168.178.1:49000/</URLBase><device><serviceList>
            <service><serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
            <controlURL>igdupnp/control/WANPPPConn1</controlURL></service></serviceList></device></root>"#;
        let s = find_wan_service(x, "http://192.168.178.1:49000/igddesc.xml").unwrap();
        assert_eq!(s.control_url, "http://192.168.178.1:49000/igdupnp/control/WANPPPConn1");
        // WANIPConnection wins over PPP; namespace prefixes are accepted
        let y = r#"<d:root xmlns:d="x"><d:service><d:serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</d:serviceType>
            <d:controlURL>/ppp</d:controlURL></d:service><d:service><d:serviceType>urn:schemas-upnp-org:service:WANIPConnection:2</d:serviceType>
            <d:controlURL>/ip</d:controlURL></d:service></d:root>"#;
        let s = find_wan_service(y, "http://10.0.0.1/desc.xml").unwrap();
        assert_eq!((s.service_type.as_str(), s.control_url.as_str()), ("urn:schemas-upnp-org:service:WANIPConnection:2", "http://10.0.0.1/ip"));
        assert_eq!(find_wan_service("<root><service><serviceType>urn:x:Other:1</serviceType><controlURL>/a</controlURL></service></root>", "http://h/"), None);
    }

    #[test]
    fn url_joining() {
        assert_eq!(join_url("http://h:1/a/b.xml", "/c"), "http://h:1/c");
        assert_eq!(join_url("http://h:1/a/b.xml", "c"), "http://h:1/a/c");
        assert_eq!(join_url("http://h:1", "c"), "http://h:1/c");
        assert_eq!(join_url("http://h:1/x/", "http://other/y"), "http://other/y");
    }

    #[test]
    fn soap_bodies() {
        let st = "urn:schemas-upnp-org:service:WANIPConnection:1";
        let b = add_port_mapping_body(st, 7777, 7777, Ipv4Addr::new(192, 168, 1, 20), "HalfSword-MP <UDP>", 3600);
        assert!(b.starts_with("<?xml version=\"1.0\"?>"));
        assert!(b.contains(&format!("<u:AddPortMapping xmlns:u=\"{st}\">")));
        for part in ["<NewRemoteHost></NewRemoteHost>", "<NewExternalPort>7777</NewExternalPort>", "<NewProtocol>UDP</NewProtocol>",
                     "<NewInternalPort>7777</NewInternalPort>", "<NewInternalClient>192.168.1.20</NewInternalClient>",
                     "<NewEnabled>1</NewEnabled>", "<NewPortMappingDescription>HalfSword-MP &lt;UDP&gt;</NewPortMappingDescription>",
                     "<NewLeaseDuration>3600</NewLeaseDuration>"] {
            assert!(b.contains(part), "missing {part}");
        }
        // argument order is the one the service description declares (some routers insist)
        assert!(b.find("NewRemoteHost").unwrap() < b.find("NewExternalPort").unwrap());
        assert!(b.find("NewInternalClient").unwrap() < b.find("NewLeaseDuration").unwrap());
        let d = delete_port_mapping_body(st, 7777);
        assert!(d.contains("<u:DeletePortMapping ") && d.contains("<NewExternalPort>7777</NewExternalPort>") && !d.contains("NewInternalClient"));
        assert_eq!(soap_action(st, "AddPortMapping"), format!("\"{st}#AddPortMapping\""));
        assert!(get_external_ip_body(st).contains("<u:GetExternalIPAddress xmlns:u=\""));
    }

    #[test]
    fn soap_answers_and_faults() {
        let ok = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>
            <u:GetExternalIPAddressResponse xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1">
            <NewExternalIPAddress>203.0.113.40</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>"#;
        assert_eq!(element(ok, "NewExternalIPAddress"), Some("203.0.113.40"));
        let fault = r#"<s:Envelope><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>
            <detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>718</errorCode>
            <errorDescription>ConflictInMappingEntry</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#;
        assert_eq!(soap_error(fault), Some(err::CONFLICT));
        assert_eq!(soap_error("<html>500</html>"), None);
    }
}
