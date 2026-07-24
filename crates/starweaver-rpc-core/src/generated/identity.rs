//! Generated protocol identity.

pub const PROTOCOL_NAME: &str = "starweaver.host";
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_REVISION: &str = "2026-07-24";
pub const SCHEMA_DIGEST: &str =
    "sha256:2a9e9fad809f55e34f2b701aed6008b2a91148c19f3988a8e79b5c00d404e6dd";
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
