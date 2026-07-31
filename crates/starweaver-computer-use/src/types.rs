use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! string_id {
    ($name:ident) => {
        #[doc = concat!("Process-local ", stringify!($name), ".")]
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            /// Parse a canonical UUID string.
            ///
            /// # Errors
            ///
            /// Returns an error when the value is not a canonical UUID.
            pub fn parse(value: impl Into<String>) -> Result<Self, uuid::Error> {
                let value = value.into();
                Uuid::parse_str(&value)?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_id!(ProcessInstanceId);
string_id!(ComputerSessionId);
string_id!(ObservationId);
string_id!(OperationId);
string_id!(InvocationId);

impl OperationId {
    #[must_use]
    pub(crate) fn from_uuid(value: Uuid) -> Self {
        Self(value.to_string())
    }
}

impl InvocationId {
    /// Derive a bounded invocation identity from stable adapter-owned parts.
    #[must_use]
    pub fn from_stable_parts<'a>(domain: &str, parts: impl IntoIterator<Item = &'a str>) -> Self {
        use sha2::{Digest as _, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        for part in parts {
            hasher.update([0]);
            hasher.update(part.as_bytes());
        }
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(Uuid::from_bytes(bytes).to_string())
    }
}

macro_rules! generation {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Serialize,
            Deserialize,
            JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}

generation!(TargetGeneration);
generation!(LayoutGeneration);
generation!(FrameGeneration);
generation!(AccessibilityGeneration);
generation!(EffectEpoch);
generation!(OperationSequence);
generation!(TakeoverEpoch);

/// Version of the process-local typed service contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseContractVersion {
    pub major: u16,
    pub minor: u16,
}

impl ComputerUseContractVersion {
    pub const V1: Self = Self { major: 1, minor: 1 };
}

/// Version of the canonical function-tool catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCatalogVersion {
    pub major: u16,
    pub minor: u16,
}

impl ToolCatalogVersion {
    pub const V1: Self = Self { major: 1, minor: 1 };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DesktopSurfaceScope {
    #[default]
    PrimaryDisplay,
    VisibleDesktop,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeDesktopPlatform {
    Macos,
    Windows,
    Linux,
    #[default]
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeBackendKind {
    MacosCoreGraphics,
    Fake,
    #[default]
    Unsupported,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerCapabilityGrant {
    pub observe: bool,
    pub pointer: bool,
    pub keyboard: bool,
    pub accessibility_snapshot: bool,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveComputerCapabilities {
    pub observe: bool,
    pub pointer: bool,
    pub keyboard: bool,
    pub accessibility_snapshot: bool,
}

impl EffectiveComputerCapabilities {
    #[must_use]
    pub const fn intersect(self, grant: ComputerCapabilityGrant) -> Self {
        Self {
            observe: self.observe && grant.observe,
            pointer: self.pointer && grant.pointer && grant.observe,
            keyboard: self.keyboard && grant.keyboard && grant.observe,
            accessibility_snapshot: self.observe
                && self.accessibility_snapshot
                && grant.accessibility_snapshot
                && grant.observe,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCapabilityStatus {
    Granted,
    Required,
    Denied,
    Unavailable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActiveSessionStatus {
    Active,
    Inactive,
    Locked,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserPresenceStatus {
    Unavailable,
    Disarmed,
    Armed,
    Active,
    Paused,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionReport {
    pub platform: NativeDesktopPlatform,
    pub backend: NativeBackendKind,
    pub active_session: ActiveSessionStatus,
    pub capture: PermissionCapabilityStatus,
    pub pointer_input: PermissionCapabilityStatus,
    pub keyboard_input: PermissionCapabilityStatus,
    pub accessibility: PermissionCapabilityStatus,
    pub user_presence: PermissionCapabilityStatus,
    pub restart_required: bool,
    #[serde(default)]
    pub remediation: Vec<String>,
    pub diagnostics_code: String,
}

impl PermissionReport {
    #[must_use]
    pub fn unsupported(platform: NativeDesktopPlatform) -> Self {
        Self {
            platform,
            backend: NativeBackendKind::Unsupported,
            active_session: ActiveSessionStatus::Unknown,
            capture: PermissionCapabilityStatus::Unavailable,
            pointer_input: PermissionCapabilityStatus::Unavailable,
            keyboard_input: PermissionCapabilityStatus::Unavailable,
            accessibility: PermissionCapabilityStatus::Unavailable,
            user_presence: PermissionCapabilityStatus::Unavailable,
            restart_required: false,
            remediation: Vec::new(),
            diagnostics_code: "unsupported_platform".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComputerSessionState {
    Created,
    Probing,
    ReadyObserveOnly,
    ReadyControl,
    Operating,
    Paused,
    SessionUnavailable,
    Closing,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerStatus {
    pub contract_version: ComputerUseContractVersion,
    pub process_instance_id: ProcessInstanceId,
    pub session_id: Option<ComputerSessionId>,
    pub state: ComputerSessionState,
    pub platform: NativeDesktopPlatform,
    pub backend: NativeBackendKind,
    pub desktop_scope: DesktopSurfaceScope,
    pub active_session: ActiveSessionStatus,
    pub permissions: PermissionReport,
    pub effective_capabilities: EffectiveComputerCapabilities,
    pub target_generation: Option<TargetGeneration>,
    pub layout_generation: Option<LayoutGeneration>,
    pub effect_epoch: Option<EffectEpoch>,
    pub user_presence: UserPresenceStatus,
    pub diagnostics_code: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

impl PixelSize {
    #[must_use]
    pub const fn contains(self, point: ModelPoint) -> bool {
        point.x < self.width && point.y < self.height
    }

    #[must_use]
    pub fn pixels(self) -> Option<u64> {
        u64::from(self.width).checked_mul(u64::from(self.height))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelPoint {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NativePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NativeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AffineTransform2D {
    pub values: [f64; 9],
}

impl AffineTransform2D {
    pub const IDENTITY: Self = Self {
        values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// Build a finite, invertible affine transform.
    ///
    /// # Errors
    ///
    /// Returns an error for non-finite, non-affine, or singular matrices.
    pub fn checked(values: [f64; 9]) -> Result<Self, &'static str> {
        if !values.iter().all(|value| value.is_finite()) {
            return Err("transform contains a non-finite value");
        }
        if values[6].abs() > f64::EPSILON
            || values[7].abs() > f64::EPSILON
            || (values[8] - 1.0).abs() > f64::EPSILON
        {
            return Err("transform is not affine");
        }
        let determinant = values[0].mul_add(values[4], -(values[1] * values[3]));
        if determinant.abs() <= 1.0e-12 {
            return Err("transform is not invertible");
        }
        Ok(Self { values })
    }

    /// Compute the checked inverse transform.
    ///
    /// # Errors
    ///
    /// Returns an error when the transform is singular or invalid.
    pub fn inverse(self) -> Result<Self, &'static str> {
        let [
            scale_x,
            shear_x,
            translate_x,
            shear_y,
            scale_y,
            translate_y,
            _,
            _,
            _,
        ] = self.values;
        let determinant = scale_x.mul_add(scale_y, -(shear_x * shear_y));
        if !determinant.is_finite() || determinant.abs() <= 1.0e-12 {
            return Err("transform is not invertible");
        }
        let inverse = [
            scale_y / determinant,
            -shear_x / determinant,
            (shear_x.mul_add(translate_y, -(scale_y * translate_x))) / determinant,
            -shear_y / determinant,
            scale_x / determinant,
            (shear_y.mul_add(translate_x, -(scale_x * translate_y))) / determinant,
            0.0,
            0.0,
            1.0,
        ];
        Self::checked(inverse)
    }

    #[must_use]
    pub fn apply(self, point: ModelPoint) -> NativePoint {
        NativePoint {
            x: self.values[0].mul_add(f64::from(point.x), self.values[1] * f64::from(point.y))
                + self.values[2],
            y: self.values[3].mul_add(f64::from(point.x), self.values[4] * f64::from(point.y))
                + self.values[5],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayGeometry {
    pub model_rect: ModelRect,
    pub native_rect: NativeRect,
    pub scale_factor: f64,
    pub rotation_degrees: u16,
    pub primary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeometrySnapshot {
    pub target_generation: TargetGeneration,
    pub layout_generation: LayoutGeneration,
    pub model_size_px: PixelSize,
    pub native_desktop_rect: NativeRect,
    pub model_to_native: AffineTransform2D,
    pub native_to_model: AffineTransform2D,
    pub displays: Vec<DisplayGeometry>,
    pub cursor_embedded: bool,
}

impl GeometrySnapshot {
    /// Validate image dimensions and the transform pair.
    ///
    /// # Errors
    ///
    /// Returns an error when dimensions or transforms are inconsistent.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.model_size_px.width == 0 || self.model_size_px.height == 0 {
            return Err("model image dimensions must be non-zero");
        }
        let inverse = self.model_to_native.inverse()?;
        let tolerance = 1.0e-8;
        if inverse
            .values
            .iter()
            .zip(self.native_to_model.values)
            .any(|(left, right)| (left - right).abs() > tolerance)
        {
            return Err("geometry transforms are not inverses");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DesktopImageMime {
    ImagePng,
    ImageJpeg,
}

impl DesktopImageMime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImagePng => "image/png",
            Self::ImageJpeg => "image/jpeg",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameRedactionStatus {
    Complete,
    Redacted,
    Protected,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedDesktopImage {
    pub mime_type: DesktopImageMime,
    pub bytes: Vec<u8>,
    pub size_px: PixelSize,
    pub sha256: [u8; 32],
    pub color_space: Option<String>,
    pub redaction: FrameRedactionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessibilitySnapshot {
    pub generation: AccessibilityGeneration,
    pub captured_at_monotonic_ms: u64,
    pub nodes: Vec<AccessibilityNode>,
    pub truncated: bool,
    pub truncation_reasons: Vec<AccessibilityTruncationReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityTruncationReason {
    NodeLimit,
    DepthLimit,
    ChildLimit,
    StringLimit,
    TotalStringLimit,
    TimeLimit,
    AttributeUnavailable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessibilityState {
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    pub selected: Option<bool>,
    pub protected: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessibilityNode {
    pub local_id: u64,
    pub parent_local_id: Option<u64>,
    pub role: String,
    pub name: Option<String>,
    pub value_summary: Option<String>,
    pub state: AccessibilityState,
    pub model_bounds: Option<ModelRect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputerObservation {
    pub process_instance_id: ProcessInstanceId,
    pub session_id: ComputerSessionId,
    pub observation_id: ObservationId,
    pub target_generation: TargetGeneration,
    pub layout_generation: LayoutGeneration,
    pub frame_generation: FrameGeneration,
    pub effect_epoch: EffectEpoch,
    pub captured_at_monotonic_ms: u64,
    pub geometry: GeometrySnapshot,
    pub image: EncodedDesktopImage,
    pub accessibility: Option<AccessibilitySnapshot>,
    pub capabilities: EffectiveComputerCapabilities,
    pub session_state: ComputerSessionState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveRequest {
    pub operation_id: OperationId,
    #[serde(default)]
    pub include_accessibility: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationRef {
    pub observation_id: ObservationId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKey {
    Shift,
    Control,
    Alt,
    Meta,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Shift,
    Control,
    Alt,
    Meta,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyMode {
    Chord,
    Sequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum ComputerAction {
    Click(ClickAction),
    MovePointer(MovePointerAction),
    Drag(DragAction),
    Scroll(ScrollAction),
    TypeText(TypeTextAction),
    PressKeys(PressKeysAction),
}

impl ComputerAction {
    #[must_use]
    pub const fn kind(&self) -> ComputerActionKind {
        match self {
            Self::Click(_) => ComputerActionKind::Click,
            Self::MovePointer(_) => ComputerActionKind::MovePointer,
            Self::Drag(_) => ComputerActionKind::Drag,
            Self::Scroll(_) => ComputerActionKind::Scroll,
            Self::TypeText(_) => ComputerActionKind::TypeText,
            Self::PressKeys(_) => ComputerActionKind::PressKeys,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComputerActionKind {
    Click,
    MovePointer,
    Drag,
    Scroll,
    TypeText,
    PressKeys,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClickAction {
    pub point: ModelPoint,
    #[serde(default)]
    pub button: PointerButton,
    #[serde(default = "default_click_count")]
    pub click_count: u8,
    #[serde(default)]
    pub modifiers: Vec<ModifierKey>,
}

const fn default_click_count() -> u8 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MovePointerAction {
    pub point: ModelPoint,
    #[serde(default)]
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DragAction {
    pub path: Vec<ModelPoint>,
    #[serde(default)]
    pub button: PointerButton,
    pub duration_ms: u32,
    #[serde(default)]
    pub modifiers: Vec<ModifierKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollAction {
    pub anchor: ModelPoint,
    pub delta_x_model_px: i32,
    pub delta_y_model_px: i32,
    #[serde(default)]
    pub modifiers: Vec<ModifierKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeTextAction {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PressKeysAction {
    pub keys: Vec<CanonicalKey>,
    pub mode: KeyMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComputerActionRequest {
    pub operation_id: OperationId,
    pub observation: ObservationRef,
    pub action: ComputerAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    NotExecuted,
    Executed,
    PartiallyExecuted,
    DeliveryUncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputCleanupStatus {
    NotRequired,
    Complete,
    BestEffort,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StabilityCheckStatus {
    NotPerformed,
    Passed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerActionReceipt {
    pub operation_id: OperationId,
    pub sequence: OperationSequence,
    pub request_digest_hex: String,
    pub effect_status: EffectStatus,
    pub action_kind: ComputerActionKind,
    pub process_instance_id: ProcessInstanceId,
    pub session_id: ComputerSessionId,
    pub target_generation: TargetGeneration,
    pub basis_observation_id: ObservationId,
    pub basis_layout_generation: LayoutGeneration,
    pub basis_effect_epoch: EffectEpoch,
    pub resulting_effect_epoch: EffectEpoch,
    pub native_event_count: u32,
    pub transformed_points: Vec<NativePoint>,
    pub cleanup: InputCleanupStatus,
    pub stability_check: StabilityCheckStatus,
    pub started_at_monotonic_ms: u64,
    pub completed_at_monotonic_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputerActionResult {
    pub receipt: ComputerActionReceipt,
    pub observation: ComputerObservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    UserTakeover,
    EmergencyStop,
    HostRequest,
    CleanupFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    HostShutdown,
    ClientDisconnected,
    SessionInvalidated,
    Replaced,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PauseReceipt {
    pub state: ComputerSessionState,
    pub takeover_epoch: TakeoverEpoch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloseReceipt {
    pub state: ComputerSessionState,
    pub cleanup: InputCleanupStatus,
}

pub type ShutdownReceipt = CloseReceipt;

#[derive(Clone, Debug)]
pub struct ScreenshotPolicy {
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_encoded_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ActionPolicy {
    pub max_click_count: u8,
    pub max_path_points: usize,
    pub max_duration_ms: u32,
    pub max_scroll_abs: i32,
    pub max_text_bytes: usize,
    pub max_text_scalars: usize,
    pub max_keys: usize,
    pub max_modifiers: usize,
}

#[derive(Clone, Debug)]
pub struct IdempotencyPolicy {
    pub max_entries: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PermissionPromptPolicy {
    pub capture_on_open: bool,
    pub accessibility_on_observe: bool,
}

#[derive(Clone, Debug)]
pub struct AccessibilityPolicy {
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_children_per_node: usize,
    pub max_string_bytes: usize,
    pub max_total_string_bytes: usize,
    pub capture_timeout: Duration,
    pub messaging_timeout: Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequest {
    #[serde(default)]
    pub screen_recording: bool,
    #[serde(default)]
    pub accessibility: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRequestOutcome {
    pub requested: PermissionRequest,
    pub permissions: PermissionReport,
    pub effective_capabilities: EffectiveComputerCapabilities,
    pub diagnostics_code: String,
}

#[derive(Clone, Debug)]
pub struct ComputerUsePolicy {
    pub desktop_scope: DesktopSurfaceScope,
    pub allowed_capabilities: ComputerCapabilityGrant,
    pub permission_prompts: PermissionPromptPolicy,
    pub screenshot: ScreenshotPolicy,
    pub accessibility: AccessibilityPolicy,
    pub action: ActionPolicy,
    pub queue_wait_timeout: Duration,
    pub operation_timeout: Duration,
    pub cancellation_cleanup_timeout: Duration,
    pub post_action_settle: Duration,
    pub observation_max_age: Duration,
    pub max_observations: usize,
    pub idempotency: IdempotencyPolicy,
}

impl Default for ComputerUsePolicy {
    fn default() -> Self {
        Self {
            desktop_scope: DesktopSurfaceScope::PrimaryDisplay,
            allowed_capabilities: ComputerCapabilityGrant {
                observe: true,
                pointer: false,
                keyboard: false,
                accessibility_snapshot: false,
            },
            permission_prompts: PermissionPromptPolicy::default(),
            screenshot: ScreenshotPolicy {
                max_width: 4096,
                max_height: 4096,
                max_pixels: 16_777_216,
                max_encoded_bytes: 16 * 1024 * 1024,
            },
            accessibility: AccessibilityPolicy {
                max_nodes: 512,
                max_depth: 12,
                max_children_per_node: 64,
                max_string_bytes: 2 * 1024,
                max_total_string_bytes: 128 * 1024,
                capture_timeout: Duration::from_secs(3),
                messaging_timeout: Duration::from_millis(250),
            },
            action: ActionPolicy {
                max_click_count: 3,
                max_path_points: 64,
                max_duration_ms: 10_000,
                max_scroll_abs: 100_000,
                max_text_bytes: 16_384,
                max_text_scalars: 8_192,
                max_keys: 32,
                max_modifiers: 4,
            },
            queue_wait_timeout: Duration::from_secs(5),
            operation_timeout: Duration::from_secs(15),
            cancellation_cleanup_timeout: Duration::from_secs(2),
            post_action_settle: Duration::from_millis(250),
            observation_max_age: Duration::from_secs(30),
            max_observations: 16,
            idempotency: IdempotencyPolicy { max_entries: 256 },
        }
    }
}
