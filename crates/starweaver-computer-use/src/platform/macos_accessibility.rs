#![allow(unsafe_code)]

//! Audited macOS Accessibility FFI boundary.
//!
//! All raw Core Foundation ownership conversion is confined to this module.
//! Public-to-the-parent functions return owned Starweaver types only; native
//! handles and process identifiers never cross the boundary.

use std::{collections::VecDeque, ffi::c_void, ptr::NonNull, time::Instant};

use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions, AXUIElement, AXValue, AXValueType,
    kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFIndex, CFRange, CFRetained, CFString,
    CFStringBuiltInEncodings, CFType, CGPoint, CGSize,
};

use crate::{
    AccessibilityGeneration, AccessibilityNode, AccessibilityPolicy, AccessibilitySnapshot,
    AccessibilityState, AccessibilityTruncationReason, ComputerUseError, ComputerUseErrorCode,
    GeometrySnapshot, ModelRect, RetryClassification,
};

const AX_CONTAINS_PROTECTED_CONTENT: &str = "AXContainsProtectedContent";
const AX_SECURE_TEXT_FIELD: &str = "AXSecureTextField";

pub(super) fn is_trusted() -> bool {
    // SAFETY: This process-global query takes no pointers or mutable state.
    unsafe { AXIsProcessTrusted() }
}

pub(super) fn request_trust() -> bool {
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(
        &[unsafe { kAXTrustedCheckOptionPrompt }],
        &[CFBoolean::new(true)],
    );
    // SAFETY: CFDictionary runtime identity does not encode key/value types;
    // this dictionary contains exactly the documented CFString/CFBoolean pair.
    let options = unsafe { CFRetained::cast_unchecked::<CFDictionary>(options) };
    // SAFETY: The immediate result remains authoritative even when macOS
    // presents the settings prompt asynchronously.
    unsafe { AXIsProcessTrustedWithOptions(Some(&options)) }
}

pub(super) fn capture(
    policy: &AccessibilityPolicy,
    geometry: &GeometrySnapshot,
    generation: AccessibilityGeneration,
    captured_at_monotonic_ms: u64,
) -> Result<AccessibilitySnapshot, ComputerUseError> {
    if !is_trusted() {
        return Err(permission_required());
    }
    let started = Instant::now();
    // SAFETY: The create function returns one retained process-local handle.
    let system = unsafe { AXUIElement::new_system_wide() };
    set_messaging_timeout(&system, policy, started)?;
    let (focused, focus_timed_out) = focused_application(&system, policy, started)?;
    if focus_timed_out {
        return Ok(AccessibilitySnapshot {
            generation,
            captured_at_monotonic_ms,
            nodes: Vec::new(),
            truncated: true,
            truncation_reasons: vec![AccessibilityTruncationReason::TimeLimit],
        });
    }
    let focused = focused
        .ok_or_else(|| backend_error("the focused macOS application has no Accessibility tree"))?;

    let mut collector = Collector {
        policy,
        geometry,
        started,
        nodes: Vec::new(),
        queue: VecDeque::from([(focused, None, 0_usize, false)]),
        total_string_bytes: 0,
        reasons: Vec::new(),
    };
    collector.run()?;
    if !is_trusted() {
        return Err(permission_required());
    }
    Ok(AccessibilitySnapshot {
        generation,
        captured_at_monotonic_ms,
        nodes: collector.nodes,
        truncated: !collector.reasons.is_empty(),
        truncation_reasons: collector.reasons,
    })
}

struct Collector<'a> {
    policy: &'a AccessibilityPolicy,
    geometry: &'a GeometrySnapshot,
    started: Instant,
    nodes: Vec<AccessibilityNode>,
    queue: VecDeque<(CFRetained<AXUIElement>, Option<u64>, usize, bool)>,
    total_string_bytes: usize,
    reasons: Vec<AccessibilityTruncationReason>,
}

impl Collector<'_> {
    #[allow(clippy::too_many_lines)]
    fn run(&mut self) -> Result<(), ComputerUseError> {
        'nodes: while let Some((element, parent_local_id, depth, inherited_protection)) =
            self.queue.pop_front()
        {
            if self.deadline_exceeded() {
                break;
            }
            set_messaging_timeout(&element, self.policy, self.started)?;
            if self.nodes.len() >= self.policy.max_nodes {
                self.truncate(AccessibilityTruncationReason::NodeLimit);
                break;
            }
            let role = self
                .bounded_attribute_string(&element, "AXRole")?
                .unwrap_or_else(|| self.bounded_string("AXUnknown".into()).unwrap_or_default());
            if role.is_empty() {
                self.truncate(AccessibilityTruncationReason::TotalStringLimit);
                break;
            }
            if self.deadline_exceeded() {
                break;
            }
            let subrole = self.bounded_attribute_string(&element, "AXSubrole")?;
            if self.deadline_exceeded() {
                break;
            }
            set_messaging_timeout(&element, self.policy, self.started)?;
            let protected_attribute = attribute_bool(&element, AX_CONTAINS_PROTECTED_CONTENT)?;
            let (protected, protected_projection) = protection_state(
                inherited_protection,
                protected_attribute,
                subrole.as_deref(),
            );
            let title = self.bounded_attribute_string(&element, "AXTitle")?;
            if self.deadline_exceeded() {
                break;
            }
            let description = self.bounded_attribute_string(&element, "AXDescription")?;
            if self.deadline_exceeded() {
                break;
            }
            let value_summary = if protected {
                None
            } else {
                self.bounded_attribute_string(&element, "AXValue")?
            };
            if self.deadline_exceeded() {
                break;
            }
            set_messaging_timeout(&element, self.policy, self.started)?;
            let enabled = attribute_bool(&element, "AXEnabled")?;
            if self.deadline_exceeded() {
                break;
            }
            set_messaging_timeout(&element, self.policy, self.started)?;
            let focused = attribute_bool(&element, "AXFocused")?;
            if self.deadline_exceeded() {
                break;
            }
            set_messaging_timeout(&element, self.policy, self.started)?;
            let selected = attribute_bool(&element, "AXSelected")?;
            if self.deadline_exceeded() {
                break;
            }
            let model_bounds = model_bounds(&element, self.geometry, self.policy, self.started)?;
            if self.deadline_exceeded() {
                break;
            }
            let local_id = u64::try_from(self.nodes.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            self.nodes.push(AccessibilityNode {
                local_id,
                parent_local_id,
                role,
                name: title.or(description),
                value_summary,
                state: AccessibilityState {
                    enabled,
                    focused,
                    selected,
                    protected: protected_projection,
                },
                model_bounds,
            });

            let (children, child_limit_exceeded, child_read_timed_out) = copy_children(
                &element,
                self.policy.max_children_per_node,
                self.policy,
                self.started,
            )?;
            if child_limit_exceeded {
                self.truncate(AccessibilityTruncationReason::ChildLimit);
            }
            if child_read_timed_out {
                self.truncate(AccessibilityTruncationReason::TimeLimit);
                break;
            }
            let Some(children) = children else {
                continue;
            };
            if depth >= self.policy.max_depth {
                if !children.is_empty() {
                    self.truncate(AccessibilityTruncationReason::DepthLimit);
                }
                continue;
            }
            for child in children
                .iter()
                .filter_map(|value| value.downcast::<AXUIElement>().ok())
            {
                if self.deadline_exceeded() {
                    break 'nodes;
                }
                self.queue
                    .push_back((child, Some(local_id), depth + 1, protected));
            }
        }
        Ok(())
    }

    fn bounded_attribute_string(
        &mut self,
        element: &AXUIElement,
        attribute: &'static str,
    ) -> Result<Option<String>, ComputerUseError> {
        set_messaging_timeout(element, self.policy, self.started)?;
        let Some(value) = copy_attribute(element, attribute)? else {
            return Ok(None);
        };
        let Some(value) = value.downcast::<CFString>().ok() else {
            return Ok(None);
        };
        let (value, truncated) = bounded_cf_string(&value, self.policy.max_string_bytes);
        if truncated {
            self.truncate(AccessibilityTruncationReason::StringLimit);
        }
        Ok(self.bounded_string(value))
    }

    fn deadline_exceeded(&mut self) -> bool {
        if self.started.elapsed() < self.policy.capture_timeout {
            return false;
        }
        self.truncate(AccessibilityTruncationReason::TimeLimit);
        true
    }

    fn bounded_string(&mut self, mut string: String) -> Option<String> {
        if string.len() > self.policy.max_string_bytes {
            truncate_utf8(&mut string, self.policy.max_string_bytes);
            self.truncate(AccessibilityTruncationReason::StringLimit);
        }
        let remaining = self
            .policy
            .max_total_string_bytes
            .saturating_sub(self.total_string_bytes);
        if string.len() > remaining {
            truncate_utf8(&mut string, remaining);
            self.truncate(AccessibilityTruncationReason::TotalStringLimit);
        }
        self.total_string_bytes = self.total_string_bytes.saturating_add(string.len());
        (!string.is_empty()).then_some(string)
    }

    fn truncate(&mut self, reason: AccessibilityTruncationReason) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}

fn focused_application(
    system: &AXUIElement,
    policy: &AccessibilityPolicy,
    started: Instant,
) -> Result<(Option<CFRetained<AXUIElement>>, bool), ComputerUseError> {
    if started.elapsed() >= policy.capture_timeout {
        return Ok((None, true));
    }
    let focused_element = element_attribute(system, "AXFocusedUIElement")?;
    let mut current = if let Some(focused_element) = focused_element {
        focused_element
    } else {
        if started.elapsed() >= policy.capture_timeout {
            return Ok((None, true));
        }
        set_messaging_timeout(system, policy, started)?;
        let Some(focused_application) = element_attribute(system, "AXFocusedApplication")? else {
            return Ok((None, false));
        };
        focused_application
    };
    // Some applications expose only the focused element at the system-wide
    // root. Walk a fixed parent bound and prefer the AXApplication ancestor.
    for _ in 0..64 {
        if started.elapsed() >= policy.capture_timeout {
            return Ok((None, true));
        }
        set_messaging_timeout(&current, policy, started)?;
        let is_application = copy_attribute(&current, "AXRole")?
            .and_then(|value| value.downcast::<CFString>().ok())
            .is_some_and(|role| bounded_cf_string(&role, 64).0 == "AXApplication");
        if is_application {
            return Ok((Some(current), false));
        }
        if started.elapsed() >= policy.capture_timeout {
            return Ok((None, true));
        }
        set_messaging_timeout(&current, policy, started)?;
        let Some(parent) = element_attribute(&current, "AXParent")? else {
            return Ok((Some(current), false));
        };
        current = parent;
    }
    Ok((Some(current), false))
}

fn set_messaging_timeout(
    element: &AXUIElement,
    policy: &AccessibilityPolicy,
    started: Instant,
) -> Result<(), ComputerUseError> {
    let remaining = policy.capture_timeout.saturating_sub(started.elapsed());
    let timeout = policy
        .messaging_timeout
        .min(remaining)
        .as_secs_f32()
        .max(0.001);
    // SAFETY: The finite timeout applies only to this process-local handle.
    if unsafe { element.set_messaging_timeout(timeout) } == AXError::Success {
        Ok(())
    } else {
        Err(backend_error(
            "failed to set the macOS Accessibility messaging timeout",
        ))
    }
}

fn element_attribute(
    element: &AXUIElement,
    attribute: &'static str,
) -> Result<Option<CFRetained<AXUIElement>>, ComputerUseError> {
    let Some(value) = copy_attribute(element, attribute)? else {
        return Ok(None);
    };
    value
        .downcast::<AXUIElement>()
        .map(Some)
        .map_err(|_| backend_error("macOS Accessibility returned a non-element focus attribute"))
}

fn copy_attribute(
    element: &AXUIElement,
    attribute: &'static str,
) -> Result<Option<CFRetained<CFType>>, ComputerUseError> {
    let attribute = CFString::from_static_str(attribute);
    let mut raw: *const CFType = std::ptr::null();
    // SAFETY: `raw` is a valid out pointer. Success transfers one retained
    // Core Foundation object under the Copy naming rule.
    let result = unsafe { element.copy_attribute_value(&attribute, NonNull::from(&mut raw)) };
    if result == AXError::NoValue || result == AXError::AttributeUnsupported {
        return Ok(None);
    }
    if result == AXError::APIDisabled {
        return Err(permission_required());
    }
    if result != AXError::Success {
        return Err(backend_error(
            "macOS Accessibility attribute read did not complete",
        ));
    }
    let raw = NonNull::new(raw.cast_mut())
        .ok_or_else(|| backend_error("macOS Accessibility returned a null attribute value"))?;
    // SAFETY: The successful Copy call returned this object at +1 retain count.
    Ok(Some(unsafe { CFRetained::from_raw(raw) }))
}

fn attribute_bool(
    element: &AXUIElement,
    attribute: &'static str,
) -> Result<Option<bool>, ComputerUseError> {
    Ok(copy_attribute(element, attribute)?
        .and_then(|value| value.downcast::<CFBoolean>().ok())
        .map(|value| value.as_bool()))
}

type ChildrenBatch = (Option<CFRetained<CFArray<CFType>>>, bool, bool);

fn copy_children(
    element: &AXUIElement,
    max_children: usize,
    policy: &AccessibilityPolicy,
    started: Instant,
) -> Result<ChildrenBatch, ComputerUseError> {
    if started.elapsed() >= policy.capture_timeout {
        return Ok((None, false, true));
    }
    let attribute = CFString::from_static_str("AXChildren");
    set_messaging_timeout(element, policy, started)?;
    let mut count: CFIndex = 0;
    // SAFETY: `count` is a valid out pointer for this synchronous call.
    let result = unsafe { element.attribute_value_count(&attribute, NonNull::from(&mut count)) };
    if result == AXError::NoValue || result == AXError::AttributeUnsupported {
        return Ok((None, false, started.elapsed() >= policy.capture_timeout));
    }
    if result == AXError::APIDisabled {
        return Err(permission_required());
    }
    if result != AXError::Success {
        return Err(backend_error(
            "macOS Accessibility child count did not complete",
        ));
    }
    let count = usize::try_from(count.max(0)).unwrap_or(usize::MAX);
    let truncated = count > max_children;
    if started.elapsed() >= policy.capture_timeout {
        return Ok((None, truncated, true));
    }
    let requested = count.min(max_children);
    if requested == 0 {
        return Ok((None, truncated, false));
    }
    set_messaging_timeout(element, policy, started)?;
    let requested = CFIndex::try_from(requested).unwrap_or(CFIndex::MAX);
    let mut raw: *const CFArray = std::ptr::null();
    // SAFETY: `raw` is a valid out pointer. The successful Copy call returns
    // at most `requested` retained AXChildren values in one retained array.
    let result =
        unsafe { element.copy_attribute_values(&attribute, 0, requested, NonNull::from(&mut raw)) };
    if result == AXError::NoValue || result == AXError::AttributeUnsupported {
        return Ok((None, truncated, started.elapsed() >= policy.capture_timeout));
    }
    if result == AXError::APIDisabled {
        return Err(permission_required());
    }
    if result != AXError::Success {
        return Err(backend_error(
            "macOS Accessibility child read did not complete",
        ));
    }
    let raw = NonNull::new(raw.cast_mut())
        .ok_or_else(|| backend_error("macOS Accessibility returned a null child array"))?;
    // SAFETY: The successful Copy call returned this CFArray at +1. CFArray's
    // runtime identity erases its element type; AXChildren contains CFType
    // values that are individually downcast before use.
    let array = unsafe { CFRetained::from_raw(raw) };
    Ok((
        Some(unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) }),
        truncated,
        started.elapsed() >= policy.capture_timeout,
    ))
}

fn protection_state(
    inherited: bool,
    attribute: Option<bool>,
    subrole: Option<&str>,
) -> (bool, Option<bool>) {
    let protected = inherited || attribute == Some(true) || subrole == Some(AX_SECURE_TEXT_FIELD);
    (protected, protected.then_some(true).or(attribute))
}

fn bounded_cf_string(value: &CFString, max_bytes: usize) -> (String, bool) {
    let length = value.length().max(0);
    if length == 0 {
        return (String::new(), false);
    }
    if max_bytes == 0 {
        return (String::new(), true);
    }
    let capacity = max_bytes.min(isize::MAX as usize);
    let mut bytes = vec![0_u8; capacity];
    let mut used: CFIndex = 0;
    // SAFETY: `bytes` and `used` remain valid for the synchronous conversion.
    // CFStringGetBytes writes at most `capacity` bytes and does not split a
    // character that cannot fit in the destination buffer.
    let converted = unsafe {
        value.bytes(
            CFRange::new(0, length),
            CFStringBuiltInEncodings::EncodingUTF8.0,
            0,
            false,
            bytes.as_mut_ptr(),
            CFIndex::try_from(capacity).unwrap_or(CFIndex::MAX),
            std::ptr::from_mut(&mut used),
        )
    };
    let used = usize::try_from(used.max(0)).unwrap_or(0).min(bytes.len());
    bytes.truncate(used);
    let string = String::from_utf8(bytes).unwrap_or_default();
    (string, converted < length)
}

fn model_bounds(
    element: &AXUIElement,
    geometry: &GeometrySnapshot,
    policy: &AccessibilityPolicy,
    started: Instant,
) -> Result<Option<ModelRect>, ComputerUseError> {
    set_messaging_timeout(element, policy, started)?;
    let Some(position) = copy_point(element, "AXPosition")? else {
        return Ok(None);
    };
    if started.elapsed() >= policy.capture_timeout {
        return Ok(None);
    }
    set_messaging_timeout(element, policy, started)?;
    let Some(size) = copy_size(element, "AXSize")? else {
        return Ok(None);
    };
    if !position.x.is_finite()
        || !position.y.is_finite()
        || !size.width.is_finite()
        || !size.height.is_finite()
        || size.width <= 0.0
        || size.height <= 0.0
    {
        return Ok(None);
    }
    let transform = geometry.native_to_model.values;
    let transform_point = |x: f64, y: f64| {
        (
            transform[0].mul_add(x, transform[1] * y) + transform[2],
            transform[3].mul_add(x, transform[4] * y) + transform[5],
        )
    };
    let corners = [
        transform_point(position.x, position.y),
        transform_point(position.x + size.width, position.y),
        transform_point(position.x, position.y + size.height),
        transform_point(position.x + size.width, position.y + size.height),
    ];
    let min_x = corners
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::INFINITY, f64::min)
        .max(0.0);
    let min_y = corners
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::INFINITY, f64::min)
        .max(0.0);
    let max_x = corners
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::NEG_INFINITY, f64::max)
        .min(f64::from(geometry.model_size_px.width));
    let max_y = corners
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::NEG_INFINITY, f64::max)
        .min(f64::from(geometry.model_size_px.height));
    if max_x <= min_x || max_y <= min_y {
        return Ok(None);
    }
    let Some((x, y, right, bottom)) = checked_model_coordinate(min_x, false)
        .zip(checked_model_coordinate(min_y, false))
        .zip(checked_model_coordinate(max_x, true))
        .zip(checked_model_coordinate(max_y, true))
        .map(|(((x, y), right), bottom)| (x, y, right, bottom))
    else {
        return Ok(None);
    };
    Ok(Some(ModelRect {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    }))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn checked_model_coordinate(value: f64, round_up: bool) -> Option<u32> {
    let rounded = if round_up {
        value.ceil()
    } else {
        value.floor()
    };
    if !rounded.is_finite() || rounded < 0.0 || rounded > f64::from(u32::MAX) {
        return None;
    }
    Some(rounded as u32)
}

fn copy_point(
    element: &AXUIElement,
    attribute: &'static str,
) -> Result<Option<CGPoint>, ComputerUseError> {
    let mut output = CGPoint::default();
    Ok(copy_ax_value(
        element,
        attribute,
        AXValueType::CGPoint,
        NonNull::from(&mut output).cast(),
    )?
    .then_some(output))
}

fn copy_size(
    element: &AXUIElement,
    attribute: &'static str,
) -> Result<Option<CGSize>, ComputerUseError> {
    let mut output = CGSize::default();
    Ok(copy_ax_value(
        element,
        attribute,
        AXValueType::CGSize,
        NonNull::from(&mut output).cast(),
    )?
    .then_some(output))
}

fn copy_ax_value(
    element: &AXUIElement,
    attribute: &'static str,
    value_type: AXValueType,
    output: NonNull<c_void>,
) -> Result<bool, ComputerUseError> {
    let Some(value) = copy_attribute(element, attribute)? else {
        return Ok(false);
    };
    let Some(value) = value.downcast::<AXValue>().ok() else {
        return Ok(false);
    };
    // SAFETY: Callers provide an output pointer matching `value_type` and it
    // remains valid for the duration of this call.
    Ok(unsafe { value.value(value_type, output) })
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn permission_required() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::PermissionRequired,
        "Accessibility permission is required for this executable identity",
        RetryClassification::AfterPermissionChange,
    )
}

fn backend_error(message: &'static str) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        message,
        RetryClassification::AfterFreshObservation,
    )
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CFString;

    use super::{
        AX_CONTAINS_PROTECTED_CONTENT, AX_SECURE_TEXT_FIELD, bounded_cf_string, protection_state,
        truncate_utf8,
    };

    #[test]
    fn public_protection_attribute_names_are_exact() {
        assert_eq!(AX_CONTAINS_PROTECTED_CONTENT, "AXContainsProtectedContent");
        assert_eq!(AX_SECURE_TEXT_FIELD, "AXSecureTextField");
    }

    #[test]
    fn protection_is_inherited_and_unknown_is_not_projected_as_false() {
        assert_eq!(protection_state(false, None, None), (false, None));
        assert_eq!(
            protection_state(false, Some(false), None),
            (false, Some(false))
        );
        assert_eq!(
            protection_state(false, Some(true), None),
            (true, Some(true))
        );
        assert_eq!(protection_state(true, None, None), (true, Some(true)));
        assert_eq!(
            protection_state(false, None, Some(AX_SECURE_TEXT_FIELD)),
            (true, Some(true))
        );
    }

    #[test]
    fn native_string_conversion_is_bounded_before_rust_allocation() {
        let value = CFString::from_static_str("abcdef");
        assert_eq!(bounded_cf_string(&value, 3), ("abc".into(), true));
        assert_eq!(bounded_cf_string(&value, 8), ("abcdef".into(), false));
    }

    #[test]
    fn utf8_truncation_preserves_character_boundaries() {
        let mut value = "aéz".to_owned();
        truncate_utf8(&mut value, 2);
        assert_eq!(value, "a");
    }
}
