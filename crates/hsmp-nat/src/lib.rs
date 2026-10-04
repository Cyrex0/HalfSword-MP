//! NAT traversal for player-hosted servers (docs/development/protocol.md, "NAT traversal"):
//!
//! * [`stun`]: learn a socket's public endpoint and the NAT's mapping behaviour.
//! * [`probe`]: the punch probe a host sends to a joiner's public endpoint.
//! * [`portmap`]: open the server's port on the router (UPnP-IGD, PCP, NAT-PMP).

pub mod gateway;
pub mod natpmp;
pub mod pcp;
pub mod portmap;
pub mod probe;
pub mod stun;
pub mod upnp;
