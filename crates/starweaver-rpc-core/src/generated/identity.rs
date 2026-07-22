//! Generated protocol identity.

pub const PROTOCOL_NAME: &str = "starweaver.host";
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_REVISION: &str = "2026-07-21";
pub const SCHEMA_DIGEST: &str =
    "sha256:69d2b33653ad2c5eed6b23afb4e19abd14c431240634f30ac3fe756cd4a907b5";
pub const PROTOCOL_IDENTITY: ProtocolIdentityRef = ProtocolIdentityRef {
    name: PROTOCOL_NAME,
    major: PROTOCOL_MAJOR,
    revision: PROTOCOL_REVISION,
    schema_digest: SCHEMA_DIGEST,
};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolIdentityRef {
    pub name: &'static str,
    pub major: u32,
    pub revision: &'static str,
    pub schema_digest: &'static str,
}
