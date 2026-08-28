use std::collections::BTreeMap;
use std::ffi::c_void;

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use nfidb_core::{InputError, InputSink};
use nfidb_protocol::{
    Action, CommandInput, DeviceType, KeyAction, KeyboardInput, NormalizedPoint, PointerBatch,
    PointerSample, RemoteCommand, TargetGeometry, TextInput, WheelInput,
};
use parking_lot::{Mutex, RwLock};

const PRIMARY: u16 = 1 << 0;
const SECONDARY: u16 = 1 << 1;
const AUXILIARY: u16 = 1 << 2;
const MOD_SHIFT: u16 = 1 << 0;
const MOD_CONTROL: u16 = 1 << 1;
const MOD_ALT: u16 = 1 << 2;
const MOD_META: u16 = 1 << 3;
const TABLET_POINT_SUBTYPE: i64 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

#[derive(Debug, Clone, Copy)]
pub struct PointerInjectorOptions {
    pub pen_enabled: bool,
    pub touch_enabled: bool,
    pub mouse_enabled: bool,
    pub keyboard_enabled: bool,
    pub gestures_enabled: bool,
    pub strict_palm_rejection: bool,
}

impl Default for PointerInjectorOptions {
    fn default() -> Self {
        Self {
            pen_enabled: true,
            touch_enabled: false,
            mouse_enabled: true,
            keyboard_enabled: true,
            gestures_enabled: true,
            strict_palm_rejection: true,
        }
    }
}

#[derive(Default)]
struct InjectorState {
    pen_down: bool,
    pen_position: Option<CGPoint>,
    primary_touch: Option<u32>,
    mouse_buttons: u16,
    pressed_keys: BTreeMap<String, u16>,
}

pub struct PointerInjector {
    state: Mutex<InjectorState>,
    target: RwLock<TargetGeometry>,
    options: RwLock<PointerInjectorOptions>,
}

impl PointerInjector {
    pub fn new(target: TargetGeometry, options: PointerInjectorOptions) -> Result<Self, InputError> {
        if !has_post_event_access() {
            let granted = unsafe { CGRequestPostEventAccess() };
            if !granted {
                tracing::warn!(
                    "macOS Accessibility permission is not active; input will become available after NFiDB is enabled in Privacy & Security > Accessibility"
                );
            }
        }
        Ok(Self {
            state: Mutex::new(InjectorState::default()),
            target: RwLock::new(target),
            options: RwLock::new(options),
        })
    }

    pub fn set_target(&self, target: TargetGeometry) {
        *self.target.write() = target;
    }

    pub fn set_options(&self, options: PointerInjectorOptions) {
        *self.options.write() = options;
    }

    /// Kept for API parity with the Windows test sink. Quartz always targets
    /// the application under the global pointer.
    pub fn set_target_window(&self, _window: usize) {}

    fn point(&self, sample: PointerSample) -> CGPoint {
        let mapped = self.target.read().map(NormalizedPoint {
            u: sample.x_norm,
            v: sample.y_norm,
        });
        CGPoint::new(f64::from(mapped.x), f64::from(mapped.y))
    }

    fn inject_pen(state: &mut InjectorState, sample: PointerSample, point: CGPoint) -> Result<(), InputError> {
        let event_type = match sample.action {
            Action::Down => CGEventType::LeftMouseDown,
            Action::Move if state.pen_down => CGEventType::LeftMouseDragged,
            Action::Move | Action::Hover => CGEventType::MouseMoved,
            Action::Up | Action::Cancel => CGEventType::LeftMouseUp,
        };
        let event = mouse_event(event_type, point, CGMouseButton::Left)?;
        event.set_integer_value_field(EventField::MOUSE_EVENT_SUB_TYPE, TABLET_POINT_SUBTYPE);
        event.set_integer_value_field(EventField::TABLET_EVENT_DEVICE_ID, i64::from(sample.pointer_id.max(1)));
        event.set_integer_value_field(EventField::TABLET_EVENT_POINT_BUTTONS, i64::from(sample.flags));
        event.set_double_value_field(EventField::MOUSE_EVENT_PRESSURE, f64::from(sample.pressure.clamp(0.0, 1.0)));
        event.set_double_value_field(
            EventField::TABLET_EVENT_POINT_PRESSURE,
            f64::from(sample.pressure.clamp(0.0, 1.0)),
        );
        event.set_double_value_field(
            EventField::TABLET_EVENT_TILT_X,
            f64::from(sample.tilt_x_deg.clamp(-90.0, 90.0) / 90.0),
        );
        event.set_double_value_field(
            EventField::TABLET_EVENT_TILT_Y,
            f64::from(sample.tilt_y_deg.clamp(-90.0, 90.0) / 90.0),
        );
        event.set_double_value_field(EventField::TABLET_EVENT_ROTATION, f64::from(sample.twist_deg));
        event.post(CGEventTapLocation::HID);
        state.pen_down = matches!(sample.action, Action::Down | Action::Move) && sample.action != Action::Hover;
        state.pen_position = (!sample.action.is_terminal()).then_some(point);
        Ok(())
    }

    fn inject_pointer_mouse(
        state: &mut InjectorState,
        sample: PointerSample,
        point: CGPoint,
    ) -> Result<(), InputError> {
        let previous = state.mouse_buttons;
        let current = if sample.action.is_terminal() { 0 } else { sample.flags };
        post_mouse_move(point, current)?;
        post_button_changes(point, previous, current)?;
        state.mouse_buttons = current;
        Ok(())
    }

    fn inject_touch_as_pointer(
        state: &mut InjectorState,
        sample: PointerSample,
        point: CGPoint,
    ) -> Result<(), InputError> {
        if sample.action == Action::Down && state.primary_touch.is_none() {
            state.primary_touch = Some(sample.pointer_id);
        }
        if state.primary_touch != Some(sample.pointer_id) {
            return Ok(());
        }
        let mut mouse_sample = sample;
        mouse_sample.flags = if sample.action.is_terminal() { 0 } else { PRIMARY };
        Self::inject_pointer_mouse(state, mouse_sample, point)?;
        if sample.action.is_terminal() {
            state.primary_touch = None;
        }
        Ok(())
    }
}

impl InputSink for PointerInjector {
    fn inject_batch(&self, batch: &PointerBatch) -> Result<(), InputError> {
        ensure_post_event_access()?;
        let options = *self.options.read();
        let mut state = self.state.lock();
        for &sample in &batch.samples {
            let point = self.point(sample);
            match sample.device_type {
                DeviceType::Pen if options.pen_enabled => Self::inject_pen(&mut state, sample, point)?,
                DeviceType::Touch
                    if options.touch_enabled && !(options.strict_palm_rejection && state.pen_down) =>
                {
                    Self::inject_touch_as_pointer(&mut state, sample, point)?;
                }
                DeviceType::Mouse if options.mouse_enabled => {
                    Self::inject_pointer_mouse(&mut state, sample, point)?;
                }
                DeviceType::Pen | DeviceType::Touch | DeviceType::Mouse => {}
            }
        }
        Ok(())
    }

    fn inject_wheel(&self, input: &WheelInput) -> Result<(), InputError> {
        if !self.options.read().mouse_enabled {
            return Ok(());
        }
        ensure_post_event_access()?;
        let source = event_source()?;
        let vertical = (-input.delta_y).round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
        let horizontal = input.delta_x.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
        let event = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 2, vertical, horizontal, 0)
            .map_err(|()| InputError("CGEventCreateScrollWheelEvent2 failed".to_owned()))?;
        event.set_flags(flags_from_modifiers(input.modifiers));
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn inject_keyboard(&self, input: &KeyboardInput) -> Result<(), InputError> {
        if !self.options.read().keyboard_enabled {
            return Ok(());
        }
        ensure_post_event_access()?;
        let keycode = keycode_for_dom_code(&input.code)
            .ok_or_else(|| InputError(format!("unsupported DOM keyboard code {}", input.code)))?;
        let down = input.action == KeyAction::Down;
        post_key(keycode, down, input.modifiers)?;
        let mut state = self.state.lock();
        if down {
            state.pressed_keys.insert(input.code.clone(), keycode);
        } else {
            state.pressed_keys.remove(&input.code);
        }
        Ok(())
    }

    fn inject_text(&self, input: &TextInput) -> Result<(), InputError> {
        if !self.options.read().keyboard_enabled {
            return Ok(());
        }
        ensure_post_event_access()?;
        for part in input.text.split_inclusive(['\r', '\n']) {
            let text = part.trim_end_matches(['\r', '\n']);
            if !text.is_empty() {
                let event = CGEvent::new_keyboard_event(event_source()?, 0, true)
                    .map_err(|()| InputError("CGEventCreateKeyboardEvent(text) failed".to_owned()))?;
                event.set_string(text);
                event.post(CGEventTapLocation::HID);
            }
            if part.ends_with('\r') || part.ends_with('\n') {
                post_key(0x24, true, 0)?;
                post_key(0x24, false, 0)?;
            }
        }
        Ok(())
    }

    fn inject_command(&self, input: &CommandInput) -> Result<(), InputError> {
        if input.command != RemoteCommand::ResetInput && !self.options.read().gestures_enabled {
            return Ok(());
        }
        match input.command {
            RemoteCommand::AppNext => post_chord(&[(0x37, MOD_META), (0x30, 0)]),
            RemoteCommand::AppPrevious => post_chord(&[(0x37, MOD_META), (0x38, MOD_SHIFT), (0x30, 0)]),
            RemoteCommand::MinimizeForeground => post_chord(&[(0x37, MOD_META), (0x2E, 0)]),
            RemoteCommand::TaskView => post_chord(&[(0x3B, MOD_CONTROL), (0x7E, 0)]),
            RemoteCommand::ResetInput => self.reset_all(),
        }
    }

    fn reset_all(&self) -> Result<(), InputError> {
        let mut state = self.state.lock();
        if state.pen_down {
            let position = state.pen_position.unwrap_or_else(|| CGPoint::new(0.0, 0.0));
            mouse_event(CGEventType::LeftMouseUp, position, CGMouseButton::Left)?.post(CGEventTapLocation::HID);
        }
        for keycode in state.pressed_keys.values().copied().collect::<Vec<_>>() {
            post_key(keycode, false, 0)?;
        }
        state.pen_down = false;
        state.pen_position = None;
        state.primary_touch = None;
        state.mouse_buttons = 0;
        state.pressed_keys.clear();
        Ok(())
    }

    fn set_remote_input_options(&self, touch_enabled: bool, gestures_enabled: bool) -> Result<(), InputError> {
        self.reset_all()?;
        let mut options = self.options.write();
        options.touch_enabled = touch_enabled;
        options.gestures_enabled = gestures_enabled;
        Ok(())
    }
}

fn has_post_event_access() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

fn ensure_post_event_access() -> Result<(), InputError> {
    has_post_event_access().then_some(()).ok_or_else(|| {
        InputError(
            "macOS blocked remote input. Enable NFiDB in System Settings > Privacy & Security > Accessibility."
                .to_owned(),
        )
    })
}

fn event_source() -> Result<CGEventSource, InputError> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| InputError("CGEventSourceCreate failed".to_owned()))
}

fn mouse_event(kind: CGEventType, point: CGPoint, button: CGMouseButton) -> Result<CGEvent, InputError> {
    CGEvent::new_mouse_event(event_source()?, kind, point, button)
        .map_err(|()| InputError("CGEventCreateMouseEvent failed".to_owned()))
}

fn post_mouse_move(point: CGPoint, buttons: u16) -> Result<(), InputError> {
    let (kind, button) = if buttons & PRIMARY != 0 {
        (CGEventType::LeftMouseDragged, CGMouseButton::Left)
    } else if buttons & SECONDARY != 0 {
        (CGEventType::RightMouseDragged, CGMouseButton::Right)
    } else if buttons & AUXILIARY != 0 {
        (CGEventType::OtherMouseDragged, CGMouseButton::Center)
    } else {
        (CGEventType::MouseMoved, CGMouseButton::Left)
    };
    mouse_event(kind, point, button)?.post(CGEventTapLocation::HID);
    Ok(())
}

fn post_button_changes(point: CGPoint, previous: u16, current: u16) -> Result<(), InputError> {
    for (mask, button, down, up) in [
        (PRIMARY, CGMouseButton::Left, CGEventType::LeftMouseDown, CGEventType::LeftMouseUp),
        (SECONDARY, CGMouseButton::Right, CGEventType::RightMouseDown, CGEventType::RightMouseUp),
        (AUXILIARY, CGMouseButton::Center, CGEventType::OtherMouseDown, CGEventType::OtherMouseUp),
    ] {
        let kind = match (previous & mask != 0, current & mask != 0) {
            (false, true) => Some(down),
            (true, false) => Some(up),
            _ => None,
        };
        if let Some(kind) = kind {
            mouse_event(kind, point, button)?.post(CGEventTapLocation::HID);
        }
    }
    Ok(())
}

fn flags_from_modifiers(modifiers: u16) -> CGEventFlags {
    let mut flags = CGEventFlags::CGEventFlagNull;
    if modifiers & MOD_SHIFT != 0 {
        flags |= CGEventFlags::CGEventFlagShift;
    }
    if modifiers & MOD_CONTROL != 0 {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if modifiers & MOD_ALT != 0 {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if modifiers & MOD_META != 0 {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    flags
}

fn post_key(keycode: u16, down: bool, modifiers: u16) -> Result<(), InputError> {
    let event = CGEvent::new_keyboard_event(event_source()?, keycode, down)
        .map_err(|()| InputError("CGEventCreateKeyboardEvent failed".to_owned()))?;
    event.set_flags(flags_from_modifiers(modifiers));
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn post_chord(keys: &[(u16, u16)]) -> Result<(), InputError> {
    let mut modifiers = 0;
    for &(key, modifier) in keys {
        modifiers |= modifier;
        post_key(key, true, modifiers)?;
    }
    for &(key, modifier) in keys.iter().rev() {
        post_key(key, false, modifiers)?;
        modifiers &= !modifier;
    }
    Ok(())
}

fn keycode_for_dom_code(code: &str) -> Option<u16> {
    let key = match code {
        "KeyA" => 0x00, "KeyS" => 0x01, "KeyD" => 0x02, "KeyF" => 0x03,
        "KeyH" => 0x04, "KeyG" => 0x05, "KeyZ" => 0x06, "KeyX" => 0x07,
        "KeyC" => 0x08, "KeyV" => 0x09, "KeyB" => 0x0B, "KeyQ" => 0x0C,
        "KeyW" => 0x0D, "KeyE" => 0x0E, "KeyR" => 0x0F, "KeyY" => 0x10,
        "KeyT" => 0x11, "Digit1" => 0x12, "Digit2" => 0x13, "Digit3" => 0x14,
        "Digit4" => 0x15, "Digit6" => 0x16, "Digit5" => 0x17, "Equal" => 0x18,
        "Digit9" => 0x19, "Digit7" => 0x1A, "Minus" => 0x1B, "Digit8" => 0x1C,
        "Digit0" => 0x1D, "BracketRight" => 0x1E, "KeyO" => 0x1F,
        "KeyU" => 0x20, "BracketLeft" => 0x21, "KeyI" => 0x22, "KeyP" => 0x23,
        "Enter" | "NumpadEnter" => 0x24, "KeyL" => 0x25, "KeyJ" => 0x26,
        "Quote" => 0x27, "KeyK" => 0x28, "Semicolon" => 0x29,
        "Backslash" | "IntlBackslash" => 0x2A, "Comma" => 0x2B, "Slash" => 0x2C,
        "KeyN" => 0x2D, "KeyM" => 0x2E, "Period" => 0x2F, "Tab" => 0x30,
        "Space" => 0x31, "Backquote" => 0x32, "Backspace" => 0x33,
        "Escape" => 0x35, "MetaLeft" | "OSLeft" => 0x37, "ShiftLeft" => 0x38,
        "CapsLock" => 0x39, "AltLeft" => 0x3A, "ControlLeft" => 0x3B,
        "ShiftRight" => 0x3C, "AltRight" => 0x3D, "ControlRight" => 0x3E,
        "F17" => 0x40, "NumpadDecimal" => 0x41, "NumpadMultiply" => 0x43,
        "NumpadAdd" => 0x45, "NumLock" => 0x47, "NumpadDivide" => 0x4B,
        "NumpadSubtract" => 0x4E, "F18" => 0x4F, "F19" => 0x50,
        "NumpadEqual" => 0x51, "Numpad0" => 0x52, "Numpad1" => 0x53,
        "Numpad2" => 0x54, "Numpad3" => 0x55, "Numpad4" => 0x56,
        "Numpad5" => 0x57, "Numpad6" => 0x58, "Numpad7" => 0x59,
        "F20" => 0x5A, "Numpad8" => 0x5B, "Numpad9" => 0x5C,
        "F5" => 0x60, "F6" => 0x61, "F7" => 0x62, "F3" => 0x63,
        "F8" => 0x64, "F9" => 0x65, "F11" => 0x67, "F13" => 0x69,
        "F16" => 0x6A, "F14" => 0x6B, "F10" => 0x6D, "F12" => 0x6F,
        "F15" => 0x71, "Help" | "Insert" => 0x72, "Home" => 0x73,
        "PageUp" => 0x74, "Delete" => 0x75, "F4" => 0x76, "End" => 0x77,
        "F2" => 0x78, "PageDown" => 0x79, "F1" => 0x7A, "ArrowLeft" => 0x7B,
        "ArrowRight" => 0x7C, "ArrowDown" => 0x7D, "ArrowUp" => 0x7E,
        _ => return None,
    };
    Some(key)
}

pub fn set_per_monitor_dpi_awareness() -> Result<(), InputError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dom_codes_used_by_the_ipad_keyboard() {
        for code in ["KeyA", "Digit6", "Tab", "Enter", "Backspace", "Delete", "AltLeft", "ArrowDown"] {
            assert!(keycode_for_dom_code(code).is_some(), "{code}");
        }
    }

    #[test]
    fn modifier_bits_map_to_quartz_flags() {
        let flags = flags_from_modifiers(MOD_SHIFT | MOD_CONTROL | MOD_ALT | MOD_META);
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(flags.contains(CGEventFlags::CGEventFlagControl));
        assert!(flags.contains(CGEventFlags::CGEventFlagAlternate));
        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
    }
}
