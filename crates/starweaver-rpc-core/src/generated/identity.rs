//! Generated protocol identity.

pub const PROTOCOL_NAME: &str = "starweaver.host";
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_REVISION: &str = "2026-07-24";
pub const SCHEMA_DIGEST: &str =
    "sha256:92ebe8f13baf1e3aced0f0edcae3b2a9a23e2e5b64718e615bb0c6812b01c2bf";
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
