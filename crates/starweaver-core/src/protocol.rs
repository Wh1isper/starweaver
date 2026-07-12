//! Shared protocol identity and versioned durable-record codecs.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Transport-neutral identity negotiated by Starweaver-owned protocols.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolIdentity {
    /// Stable protocol family name.
    pub name: String,
    /// Breaking compatibility generation.
    pub major: u32,
    /// Documentation and fixture revision.
    pub revision: String,
    /// Implemented, negotiable feature names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

impl ProtocolIdentity {
    /// Build a protocol identity.
    #[must_use]
    pub fn new(name: impl Into<String>, major: u32, revision: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            major,
            revision: revision.into(),
            features: Vec::new(),
        }
    }

    /// Attach implemented features.
    #[must_use]
    pub fn with_features(mut self, features: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.features = features.into_iter().map(Into::into).collect();
        self
    }

    /// Validate a peer identity against an expected protocol family and major.
    ///
    /// # Errors
    ///
    /// Returns a compatibility error for a different name or major version.
    pub fn validate(&self, expected_name: &str, expected_major: u32) -> Result<(), ProtocolError> {
        if self.name != expected_name {
            return Err(ProtocolError::UnexpectedProtocol {
                expected: expected_name.to_string(),
                actual: self.name.clone(),
            });
        }
        if self.major != expected_major {
            return Err(ProtocolError::UnsupportedMajor {
                protocol: self.name.clone(),
                expected: expected_major,
                actual: self.major,
            });
        }
        Ok(())
    }
}

/// Version envelope written around stable durable JSON records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VersionedEnvelope<T> {
    /// Stable schema identifier.
    pub schema: String,
    /// Positive schema version.
    pub version: u32,
    /// Typed record payload.
    pub payload: T,
}

impl<T> VersionedEnvelope<T> {
    /// Build an envelope.
    #[must_use]
    pub fn new(schema: impl Into<String>, version: u32, payload: T) -> Self {
        Self {
            schema: schema.into(),
            version,
            payload,
        }
    }
}

/// Schema declaration implemented by durable record types.
pub trait VersionedRecord: Sized {
    /// Stable schema identifier.
    const SCHEMA: &'static str;
    /// Current schema version.
    const VERSION: u32 = 1;
    /// Whether this record explicitly accepts a previous bare-JSON v0 shape.
    const ALLOW_BARE_V0: bool = false;

    /// Decode one enveloped payload version.
    ///
    /// Records that advance beyond v1 override this method to dispatch older
    /// versions through dedicated legacy DTOs before decoding the current type.
    ///
    /// # Errors
    ///
    /// Returns an unsupported-version error or a payload decoding error.
    fn decode_version(version: u32, payload: Value) -> Result<Self, VersionedRecordError>
    where
        Self: DeserializeOwned,
    {
        if version == 0 || version != Self::VERSION {
            return Err(VersionedRecordError::UnsupportedVersion {
                schema: Self::SCHEMA,
                supported: Self::VERSION,
                actual: version,
            });
        }
        serde_json::from_value(payload).map_err(VersionedRecordError::Json)
    }

    /// Decode the explicitly supported bare-JSON v0 shape.
    ///
    /// # Errors
    ///
    /// Returns an unsupported-v0 error or a payload decoding error.
    fn decode_bare_v0(payload: Value) -> Result<Self, VersionedRecordError>
    where
        Self: DeserializeOwned,
    {
        if !Self::ALLOW_BARE_V0 {
            return Err(VersionedRecordError::BareV0Unsupported {
                schema: Self::SCHEMA,
            });
        }
        serde_json::from_value(payload).map_err(VersionedRecordError::Json)
    }
}

/// Encode a durable record as its current versioned JSON envelope.
///
/// # Errors
///
/// Returns a JSON serialization error when the record cannot be encoded.
pub fn to_versioned_json<T>(value: &T) -> Result<String, serde_json::Error>
where
    T: Serialize + VersionedRecord,
{
    serde_json::to_string(&VersionedEnvelope::new(T::SCHEMA, T::VERSION, value))
}

/// Encode a durable record as its current versioned JSON value.
///
/// # Errors
///
/// Returns a JSON serialization error when the record cannot be encoded.
pub fn to_versioned_value<T>(value: &T) -> Result<Value, serde_json::Error>
where
    T: Serialize + VersionedRecord,
{
    serde_json::to_value(VersionedEnvelope::new(T::SCHEMA, T::VERSION, value))
}

/// Decode a current envelope or an explicitly supported legacy bare JSON record.
///
/// Bare JSON is treated as legacy version zero. A JSON object carrying `schema`
/// or `version` is always treated as an envelope and fails closed when malformed.
///
/// # Errors
///
/// Returns a compatibility or JSON error for malformed, mismatched, or unknown records.
pub fn from_versioned_json<T>(input: &str) -> Result<T, VersionedRecordError>
where
    T: DeserializeOwned + VersionedRecord,
{
    let value = serde_json::from_str(input).map_err(VersionedRecordError::Json)?;
    from_versioned_value(value)
}

/// Decode a current envelope or an explicitly supported legacy bare JSON value.
///
/// # Errors
///
/// Returns a compatibility or JSON error for malformed, mismatched, or unknown records.
pub fn from_versioned_value<T>(value: Value) -> Result<T, VersionedRecordError>
where
    T: DeserializeOwned + VersionedRecord,
{
    let looks_enveloped = value
        .as_object()
        .is_some_and(|object| object.contains_key("schema") || object.contains_key("version"));
    if !looks_enveloped {
        return T::decode_bare_v0(value);
    }

    let envelope = serde_json::from_value::<VersionedEnvelope<Value>>(value)
        .map_err(VersionedRecordError::Json)?;
    if envelope.schema != T::SCHEMA {
        return Err(VersionedRecordError::WrongSchema {
            expected: T::SCHEMA,
            actual: envelope.schema,
        });
    }
    T::decode_version(envelope.version, envelope.payload)
}

/// Protocol compatibility failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// Peer selected another protocol family.
    UnexpectedProtocol {
        /// Expected protocol name.
        expected: String,
        /// Received protocol name.
        actual: String,
    },
    /// Peer selected an unsupported breaking generation.
    UnsupportedMajor {
        /// Protocol family.
        protocol: String,
        /// Supported major.
        expected: u32,
        /// Received major.
        actual: u32,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedProtocol { expected, actual } => {
                write!(formatter, "expected protocol {expected}, received {actual}")
            }
            Self::UnsupportedMajor {
                protocol,
                expected,
                actual,
            } => write!(
                formatter,
                "unsupported {protocol} major {actual}; supported major is {expected}"
            ),
        }
    }
}

impl Error for ProtocolError {}

/// Durable-record envelope failure.
#[derive(Debug)]
pub enum VersionedRecordError {
    /// JSON parsing or typed payload decoding failed.
    Json(serde_json::Error),
    /// Bare legacy JSON was supplied to a record that did not opt in.
    BareV0Unsupported {
        /// Schema id.
        schema: &'static str,
    },
    /// Envelope names another schema.
    WrongSchema {
        /// Expected schema id.
        expected: &'static str,
        /// Received schema id.
        actual: String,
    },
    /// Envelope version is unknown or invalid.
    UnsupportedVersion {
        /// Schema id.
        schema: &'static str,
        /// Current supported version.
        supported: u32,
        /// Received version.
        actual: u32,
    },
}

impl fmt::Display for VersionedRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid durable JSON: {error}"),
            Self::BareV0Unsupported { schema } => {
                write!(
                    formatter,
                    "bare v0 JSON is not supported for durable schema {schema}"
                )
            }
            Self::WrongSchema { expected, actual } => {
                write!(
                    formatter,
                    "expected durable schema {expected}, received {actual}"
                )
            }
            Self::UnsupportedVersion {
                schema,
                supported,
                actual,
            } => write!(
                formatter,
                "unsupported {schema} version {actual}; supported version is {supported}"
            ),
        }
    }
}

impl Error for VersionedRecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BareV0Unsupported { .. }
            | Self::WrongSchema { .. }
            | Self::UnsupportedVersion { .. } => None,
        }
    }
}
