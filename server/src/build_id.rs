//! This binary's build identity (hsmp_net::build): release version, protocol range,
//! IPC ABI and layout hash, and the content hash `build.rs` embedded.

// Shared by several binaries; each uses a different subset.
#![allow(dead_code)]

use hsmp_net::build::Identity;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONTENT_HASH_HEX: &str = env!("HSMP_CONTENT_HASH");

pub fn content_hash() -> [u8; 32] {
    hsmp_net::build::parse_hash(CONTENT_HASH_HEX).expect("build.rs writes 64 hex chars")
}

pub fn identity() -> Identity {
    Identity {
        version: VERSION.to_string(),
        protocol: hsmp_net::net::PROTOCOL_VERSION,
        proto_min: hsmp_net::net::VERSION_MIN,
        proto_max: hsmp_net::net::VERSION_MAX,
        ipc_abi_major: hsmp_ipc::ABI_MAJOR,
        ipc_abi_minor: hsmp_ipc::ABI_MINOR,
        ipc_layout: hsmp_ipc::segment::LAYOUT_HASH,
        content_hash: content_hash(),
    }
}

/// The Hello's build string: "<binary> <version>" (at most 32 bytes on the wire).
pub fn build_tag(binary: &str) -> String {
    format!("{binary} {VERSION}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn identity_is_this_workspace() {
        let id = super::identity();
        assert_eq!(id.version, hsmp_net::build::RELEASE_VERSION);
        assert!(hsmp_net::build::version_is_clean(&id.version));
        assert_eq!(id.protocol, hsmp_net::net::PROTOCOL_VERSION);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(Some(id.content_hash), hsmp_net::build::content::content_hash_of_dir(&root), "build.rs hashed this checkout");
        assert_eq!(hsmp_net::build::version_of_build(&super::build_tag("hsmp-sidecar")), Some(super::VERSION));
        assert!(super::build_tag("hsmp-sidecar").len() <= 32);
    }
}
