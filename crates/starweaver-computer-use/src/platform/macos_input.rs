#![allow(unsafe_code)]

//! Audited macOS CoreGraphics input boundary.
//!
//! Event construction and posting use the public safe wrappers exposed by
//! `objc2-core-graphics`. The sole unsafe operation is
//! `CGEventKeyboardSetUnicodeString`: every call receives a non-null pointer
//! into an immutable, live UTF-16 slice and the exact slice length. No native
//! handle, pointer, or borrowed UTF-16 storage leaves this module.

use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
    CGPreflightPostEventAccess, CGScrollEventUnit,
};

use crate::{CanonicalKey, ModifierKey, NativePoint, PointerButton};

const MAX_UNICODE_CHUNK_UNITS: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TextInputPart {
    Unicode(Vec<u16>),
    Key(CanonicalKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputError {
    PermissionRequired,
    EventCreationFailed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CleanupResult {
    pub event_count: u32,
    pub required: bool,
    pub complete: bool,
}

/// Process-local event sender for one serialized action.
///
/// The sender records every logically held key and mouse button. Callers must
/// invoke `cleanup` on all terminal paths; cleanup releases in reverse order.
pub(super) struct NativeInput {
    held_keys: Vec<CGKeyCode>,
    held_mouse: Option<(PointerButton, NativePoint)>,
    active_flags: CGEventFlags,
    event_count: u32,
    #[cfg(test)]
    forced_cleanup_complete: Option<bool>,
}

impl NativeInput {
    pub(super) fn new() -> Result<Self, InputError> {
        if !CGPreflightPostEventAccess() {
            return Err(InputError::PermissionRequired);
        }
        Ok(Self {
            held_keys: Vec::new(),
            held_mouse: None,
            active_flags: CGEventFlags::empty(),
            event_count: 0,
            #[cfg(test)]
            forced_cleanup_complete: None,
        })
    }

    #[cfg(test)]
    pub(super) fn with_forced_cleanup(complete: bool) -> Self {
        Self {
            held_keys: vec![key_code(CanonicalKey::A)],
            held_mouse: None,
            active_flags: CGEventFlags::empty(),
            event_count: 0,
            forced_cleanup_complete: Some(complete),
        }
    }

    pub(super) const fn event_count(&self) -> u32 {
        self.event_count
    }

    pub(super) fn pointer_location() -> Result<NativePoint, InputError> {
        let event = CGEvent::new(None).ok_or(InputError::EventCreationFailed)?;
        let point = CGEvent::location(Some(&event));
        Ok(NativePoint {
            x: point.x,
            y: point.y,
        })
    }

    pub(super) fn move_pointer(&mut self, point: NativePoint) -> Result<(), InputError> {
        self.post_mouse(CGEventType::MouseMoved, point, PointerButton::Left)
    }

    pub(super) fn mouse_down(
        &mut self,
        button: PointerButton,
        point: NativePoint,
        modifiers: &[ModifierKey],
        click_state: u8,
    ) -> Result<(), InputError> {
        let event = Self::mouse_event(mouse_down_type(button), point, button)?;
        CGEvent::set_flags(Some(&event), modifier_flags(modifiers));
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventClickState,
            i64::from(click_state),
        );
        self.post(&event)?;
        self.held_mouse = Some((button, point));
        Ok(())
    }

    pub(super) fn mouse_drag(
        &mut self,
        button: PointerButton,
        point: NativePoint,
        modifiers: &[ModifierKey],
    ) -> Result<(), InputError> {
        let event = Self::mouse_event(mouse_drag_type(button), point, button)?;
        CGEvent::set_flags(Some(&event), modifier_flags(modifiers));
        self.post(&event)?;
        if let Some((_, held_point)) = self.held_mouse.as_mut() {
            *held_point = point;
        }
        Ok(())
    }

    pub(super) fn mouse_up(
        &mut self,
        button: PointerButton,
        point: NativePoint,
        modifiers: &[ModifierKey],
        click_state: u8,
    ) -> Result<(), InputError> {
        let event = Self::mouse_event(mouse_up_type(button), point, button)?;
        CGEvent::set_flags(Some(&event), modifier_flags(modifiers));
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventClickState,
            i64::from(click_state),
        );
        self.post(&event)?;
        self.held_mouse = None;
        Ok(())
    }

    pub(super) fn scroll(
        &mut self,
        point: NativePoint,
        wheel_x: i32,
        wheel_y: i32,
        modifiers: &[ModifierKey],
    ) -> Result<(), InputError> {
        let event = CGEvent::new_scroll_wheel_event2(
            None,
            CGScrollEventUnit::Pixel,
            2,
            wheel_y,
            wheel_x,
            0,
        )
        .ok_or(InputError::EventCreationFailed)?;
        CGEvent::set_location(
            Some(&event),
            CGPoint {
                x: point.x,
                y: point.y,
            },
        );
        CGEvent::set_flags(Some(&event), modifier_flags(modifiers));
        self.post(&event)
    }

    pub(super) fn key_down(&mut self, key: CanonicalKey) -> Result<(), InputError> {
        let code = key_code(key);
        let event = Self::keyboard_event(code, true)?;
        if let Some(flag) = key_modifier_flag(key) {
            self.active_flags.insert(flag);
        }
        CGEvent::set_flags(Some(&event), self.active_flags);
        if let Err(error) = self.post(&event) {
            if let Some(flag) = key_modifier_flag(key) {
                self.active_flags.remove(flag);
            }
            return Err(error);
        }
        self.held_keys.push(code);
        Ok(())
    }

    pub(super) fn key_up(&mut self, key: CanonicalKey) -> Result<(), InputError> {
        let code = key_code(key);
        if let Some(flag) = key_modifier_flag(key) {
            self.active_flags.remove(flag);
        }
        let event = Self::keyboard_event(code, false)?;
        CGEvent::set_flags(Some(&event), self.active_flags);
        self.post(&event)?;
        if let Some(position) = self.held_keys.iter().rposition(|held| *held == code) {
            self.held_keys.remove(position);
        }
        Ok(())
    }

    pub(super) fn type_unicode_chunk(&mut self, utf16: &[u16]) -> Result<(), InputError> {
        debug_assert!(!utf16.is_empty());
        debug_assert!(utf16.len() <= MAX_UNICODE_CHUNK_UNITS);
        let down = Self::keyboard_event(0, true)?;
        set_unicode(&down, utf16);
        self.post(&down)?;
        self.held_keys.push(0);

        let up = Self::keyboard_event(0, false)?;
        // Preserve the Unicode payload on both halves of the synthetic
        // keystroke. This is the behavior expected by the CoreGraphics text
        // event path and avoids layout-dependent virtual-key interpretation.
        set_unicode(&up, utf16);
        self.post(&up)?;
        let _ = self.held_keys.pop();
        Ok(())
    }

    /// Release all state that this sender may have left held.
    ///
    /// Cleanup posts releases even after permission preflight changes, because
    /// a best-effort release is safer than abandoning known held state.
    pub(super) fn cleanup(&mut self) -> CleanupResult {
        let required = self.held_mouse.is_some() || !self.held_keys.is_empty();
        #[cfg(test)]
        if let Some(complete) = self.forced_cleanup_complete {
            if complete {
                self.held_mouse = None;
                self.held_keys.clear();
                self.active_flags = CGEventFlags::empty();
            }
            return CleanupResult {
                event_count: 0,
                required,
                complete,
            };
        }
        let permission_available = CGPreflightPostEventAccess();
        let mut releases_created = true;
        let before = self.event_count;

        if let Some((button, point)) = self.held_mouse {
            let result = Self::mouse_event(mouse_up_type(button), point, button).map(|event| {
                CGEvent::set_flags(Some(&event), self.active_flags);
                self.post_unchecked(&event);
            });
            if result.is_err() {
                releases_created = false;
            }
        }

        let mut release_flags = self.active_flags;
        let held_keys = self.held_keys.clone();
        for code in held_keys.into_iter().rev() {
            remove_code_modifier_flag(&mut release_flags, code);
            let result = Self::keyboard_event(code, false).map(|event| {
                CGEvent::set_flags(Some(&event), release_flags);
                self.post_unchecked(&event);
            });
            if result.is_err() {
                releases_created = false;
            }
        }

        let permission_still_available = CGPreflightPostEventAccess();
        let complete =
            !required || (releases_created && permission_available && permission_still_available);
        if complete {
            self.held_mouse = None;
            self.held_keys.clear();
            self.active_flags = CGEventFlags::empty();
        }

        CleanupResult {
            event_count: self.event_count.saturating_sub(before),
            required,
            complete,
        }
    }

    fn post_mouse(
        &mut self,
        event_type: CGEventType,
        point: NativePoint,
        button: PointerButton,
    ) -> Result<(), InputError> {
        let event = Self::mouse_event(event_type, point, button)?;
        self.post(&event)
    }

    fn mouse_event(
        event_type: CGEventType,
        point: NativePoint,
        button: PointerButton,
    ) -> Result<CFRetained<CGEvent>, InputError> {
        CGEvent::new_mouse_event(
            None,
            event_type,
            CGPoint {
                x: point.x,
                y: point.y,
            },
            mouse_button(button),
        )
        .ok_or(InputError::EventCreationFailed)
    }

    fn keyboard_event(code: CGKeyCode, down: bool) -> Result<CFRetained<CGEvent>, InputError> {
        CGEvent::new_keyboard_event(None, code, down).ok_or(InputError::EventCreationFailed)
    }

    fn post(&mut self, event: &CGEvent) -> Result<(), InputError> {
        if !CGPreflightPostEventAccess() {
            return Err(InputError::PermissionRequired);
        }
        self.post_unchecked(event);
        Ok(())
    }

    fn post_unchecked(&mut self, event: &CGEvent) {
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
        self.event_count = self.event_count.saturating_add(1);
    }
}

pub(super) fn text_input_parts(text: &str) -> Vec<TextInputPart> {
    let mut parts = Vec::new();
    let mut current = Vec::with_capacity(MAX_UNICODE_CHUNK_UNITS);
    for scalar in text.chars() {
        let control_key = match scalar {
            '\t' => Some(CanonicalKey::Tab),
            '\n' | '\r' => Some(CanonicalKey::Enter),
            _ => None,
        };
        if let Some(key) = control_key {
            if !current.is_empty() {
                parts.push(TextInputPart::Unicode(std::mem::take(&mut current)));
                current = Vec::with_capacity(MAX_UNICODE_CHUNK_UNITS);
            }
            parts.push(TextInputPart::Key(key));
            continue;
        }

        let mut encoded = [0_u16; 2];
        let units = scalar.encode_utf16(&mut encoded);
        if !current.is_empty() && current.len() + units.len() > MAX_UNICODE_CHUNK_UNITS {
            parts.push(TextInputPart::Unicode(std::mem::take(&mut current)));
            current = Vec::with_capacity(MAX_UNICODE_CHUNK_UNITS);
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        parts.push(TextInputPart::Unicode(current));
    }
    parts
}

impl Drop for NativeInput {
    fn drop(&mut self) {
        if self.held_mouse.is_some() || !self.held_keys.is_empty() {
            let _ = self.cleanup();
        }
    }
}

fn set_unicode(event: &CGEvent, utf16: &[u16]) {
    // SAFETY: `utf16.as_ptr()` is non-null for the required non-empty slice,
    // points to `utf16.len()` initialized `u16` values, and remains live and
    // immutable for the complete synchronous CoreGraphics call.
    unsafe {
        CGEvent::keyboard_set_unicode_string(
            Some(event),
            u64::try_from(utf16.len()).unwrap_or(0),
            utf16.as_ptr(),
        );
    }
}

const fn mouse_button(button: PointerButton) -> CGMouseButton {
    match button {
        PointerButton::Left => CGMouseButton::Left,
        PointerButton::Right => CGMouseButton::Right,
        PointerButton::Middle => CGMouseButton::Center,
    }
}

const fn mouse_down_type(button: PointerButton) -> CGEventType {
    match button {
        PointerButton::Left => CGEventType::LeftMouseDown,
        PointerButton::Right => CGEventType::RightMouseDown,
        PointerButton::Middle => CGEventType::OtherMouseDown,
    }
}

const fn mouse_up_type(button: PointerButton) -> CGEventType {
    match button {
        PointerButton::Left => CGEventType::LeftMouseUp,
        PointerButton::Right => CGEventType::RightMouseUp,
        PointerButton::Middle => CGEventType::OtherMouseUp,
    }
}

const fn mouse_drag_type(button: PointerButton) -> CGEventType {
    match button {
        PointerButton::Left => CGEventType::LeftMouseDragged,
        PointerButton::Right => CGEventType::RightMouseDragged,
        PointerButton::Middle => CGEventType::OtherMouseDragged,
    }
}

fn modifier_flags(modifiers: &[ModifierKey]) -> CGEventFlags {
    let mut flags = CGEventFlags::empty();
    for modifier in modifiers {
        flags.insert(match modifier {
            ModifierKey::Shift => CGEventFlags::MaskShift,
            ModifierKey::Control => CGEventFlags::MaskControl,
            ModifierKey::Alt => CGEventFlags::MaskAlternate,
            ModifierKey::Meta => CGEventFlags::MaskCommand,
        });
    }
    flags
}

const fn key_modifier_flag(key: CanonicalKey) -> Option<CGEventFlags> {
    match key {
        CanonicalKey::Shift => Some(CGEventFlags::MaskShift),
        CanonicalKey::Control => Some(CGEventFlags::MaskControl),
        CanonicalKey::Alt => Some(CGEventFlags::MaskAlternate),
        CanonicalKey::Meta => Some(CGEventFlags::MaskCommand),
        _ => None,
    }
}

fn remove_code_modifier_flag(flags: &mut CGEventFlags, code: CGKeyCode) {
    match code {
        56 => flags.remove(CGEventFlags::MaskShift),
        59 => flags.remove(CGEventFlags::MaskControl),
        58 => flags.remove(CGEventFlags::MaskAlternate),
        55 => flags.remove(CGEventFlags::MaskCommand),
        _ => {}
    }
}

pub(super) const fn key_code(key: CanonicalKey) -> CGKeyCode {
    match key {
        CanonicalKey::A => 0,
        CanonicalKey::S => 1,
        CanonicalKey::D => 2,
        CanonicalKey::F => 3,
        CanonicalKey::H => 4,
        CanonicalKey::G => 5,
        CanonicalKey::Z => 6,
        CanonicalKey::X => 7,
        CanonicalKey::C => 8,
        CanonicalKey::V => 9,
        CanonicalKey::B => 11,
        CanonicalKey::Q => 12,
        CanonicalKey::W => 13,
        CanonicalKey::E => 14,
        CanonicalKey::R => 15,
        CanonicalKey::Y => 16,
        CanonicalKey::T => 17,
        CanonicalKey::Digit1 => 18,
        CanonicalKey::Digit2 => 19,
        CanonicalKey::Digit3 => 20,
        CanonicalKey::Digit4 => 21,
        CanonicalKey::Digit6 => 22,
        CanonicalKey::Digit5 => 23,
        CanonicalKey::Digit9 => 25,
        CanonicalKey::Digit7 => 26,
        CanonicalKey::Digit8 => 28,
        CanonicalKey::Digit0 => 29,
        CanonicalKey::O => 31,
        CanonicalKey::U => 32,
        CanonicalKey::I => 34,
        CanonicalKey::P => 35,
        CanonicalKey::Enter => 36,
        CanonicalKey::L => 37,
        CanonicalKey::J => 38,
        CanonicalKey::K => 40,
        CanonicalKey::N => 45,
        CanonicalKey::M => 46,
        CanonicalKey::Tab => 48,
        CanonicalKey::Space => 49,
        CanonicalKey::Backspace => 51,
        CanonicalKey::Escape => 53,
        CanonicalKey::Meta => 55,
        CanonicalKey::Shift => 56,
        CanonicalKey::Alt => 58,
        CanonicalKey::Control => 59,
        CanonicalKey::F5 => 96,
        CanonicalKey::F6 => 97,
        CanonicalKey::F7 => 98,
        CanonicalKey::F3 => 99,
        CanonicalKey::F8 => 100,
        CanonicalKey::F9 => 101,
        CanonicalKey::F11 => 103,
        CanonicalKey::F10 => 109,
        CanonicalKey::F12 => 111,
        CanonicalKey::Home => 115,
        CanonicalKey::PageUp => 116,
        CanonicalKey::Delete => 117,
        CanonicalKey::F4 => 118,
        CanonicalKey::End => 119,
        CanonicalKey::F2 => 120,
        CanonicalKey::PageDown => 121,
        CanonicalKey::F1 => 122,
        CanonicalKey::ArrowLeft => 123,
        CanonicalKey::ArrowRight => 124,
        CanonicalKey::ArrowDown => 125,
        CanonicalKey::ArrowUp => 126,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{TextInputPart, key_code, text_input_parts};
    use crate::CanonicalKey;

    #[test]
    fn canonical_keys_map_to_stable_macos_virtual_codes() {
        assert_eq!(key_code(CanonicalKey::A), 0);
        assert_eq!(key_code(CanonicalKey::Meta), 55);
        assert_eq!(key_code(CanonicalKey::F1), 122);
        assert_eq!(key_code(CanonicalKey::ArrowUp), 126);
        assert_eq!(key_code(CanonicalKey::Delete), 117);
    }

    #[test]
    fn text_parts_do_not_split_surrogate_pairs() {
        let text = format!("{}😀{}", "a".repeat(19), "b".repeat(20));
        let parts = text_input_parts(&text);
        let chunks = parts
            .iter()
            .filter_map(|part| match part {
                TextInputPart::Unicode(value) => Some(value),
                TextInputPart::Key(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).collect::<Vec<_>>(),
            [19, 20, 2]
        );
        let decoded = chunks
            .iter()
            .flat_map(|chunk| char::decode_utf16(chunk.iter().copied()))
            .collect::<Result<String, _>>()
            .expect("chunks preserve valid UTF-16");
        assert_eq!(decoded, text);
    }

    #[test]
    fn text_parts_isolate_leading_control_characters_as_keys() {
        let parts = text_input_parts("\nalpha\tbeta\r");
        assert_eq!(
            parts,
            [
                TextInputPart::Key(CanonicalKey::Enter),
                TextInputPart::Unicode("alpha".encode_utf16().collect()),
                TextInputPart::Key(CanonicalKey::Tab),
                TextInputPart::Unicode("beta".encode_utf16().collect()),
                TextInputPart::Key(CanonicalKey::Enter),
            ]
        );
    }
}
