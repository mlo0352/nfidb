use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nfidb_core::{InputError, InputSink};
use nfidb_protocol::{
    Action, CommandInput, DeviceType, KeyAction, KeyboardInput, NormalizedPoint, PointerBatch, PointerSample,
    RemoteCommand, TargetGeometry, TextInput, WheelInput,
};
use parking_lot::{Mutex, RwLock};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE, POINTER_FEEDBACK_NONE,
    POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};
use windows_sys::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_CHANGE_FIRSTBUTTON_DOWN, POINTER_CHANGE_FIRSTBUTTON_UP, POINTER_CHANGE_NONE,
    POINTER_FLAG_CANCELED, POINTER_FLAG_CONFIDENCE, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_NEW, POINTER_FLAG_PRIMARY, POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_PEN_INFO,
    POINTER_TOUCH_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, PEN_FLAG_BARREL, PEN_FLAG_NONE, PEN_MASK_PRESSURE, PEN_MASK_ROTATION,
    PEN_MASK_TILT_X, PEN_MASK_TILT_Y, PT_PEN, PT_TOUCH, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SW_MINIMIZE, ShowWindow, TOUCH_FLAG_NONE, TOUCH_MASK_CONTACTAREA, TOUCH_MASK_ORIENTATION,
    TOUCH_MASK_PRESSURE,
};

const MAX_TOUCH_CONTACTS: u32 = 10;
const ERROR_NOT_READY: i32 = 21;
const INJECTION_RETRY_LIMIT: Duration = Duration::from_millis(50);
const INJECTION_RETRY_BACKOFF: Duration = Duration::from_micros(100);
const BROWSER_BUTTON_SECONDARY: u16 = 1 << 1;
const BROWSER_BUTTON_PRIMARY: u16 = 1 << 0;
const BROWSER_BUTTON_AUXILIARY: u16 = 1 << 2;
const BROWSER_BUTTON_BACK: u16 = 1 << 3;
const BROWSER_BUTTON_FORWARD: u16 = 1 << 4;
const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;
const WHEEL_SCALE: f32 = 1.2;

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

#[derive(Clone, Copy)]
struct ActivePointer {
    pointer_id: u32,
    point: POINT,
    pressure: u32,
    tilt_x: i32,
    tilt_y: i32,
    down: bool,
}

struct InjectorState {
    pen_device: HSYNTHETICPOINTERDEVICE,
    touch_device: HSYNTHETICPOINTERDEVICE,
    pen: Option<ActivePointer>,
    touches: BTreeMap<u32, ActivePointer>,
    mouse_buttons: u16,
    pressed_keys: BTreeMap<String, u16>,
    wheel_remainder_x: f32,
    wheel_remainder_y: f32,
}

// Synthetic device handles are process-owned User32 handles. Calls are serialized by the outer Mutex.
unsafe impl Send for InjectorState {}

impl Drop for InjectorState {
    fn drop(&mut self) {
        unsafe {
            if !self.pen_device.is_null() {
                DestroySyntheticPointerDevice(self.pen_device);
            }
            if !self.touch_device.is_null() {
                DestroySyntheticPointerDevice(self.touch_device);
            }
        }
    }
}

pub struct PointerInjector {
    state: Mutex<InjectorState>,
    target: RwLock<TargetGeometry>,
    options: RwLock<PointerInjectorOptions>,
    target_window: AtomicUsize,
}

impl PointerInjector {
    pub fn new(target: TargetGeometry, options: PointerInjectorOptions) -> Result<Self, InputError> {
        let pen_device = unsafe { CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_NONE) };
        if pen_device.is_null() {
            return Err(last_error("CreateSyntheticPointerDevice(PT_PEN)"));
        }
        let touch_device = unsafe { CreateSyntheticPointerDevice(PT_TOUCH, MAX_TOUCH_CONTACTS, POINTER_FEEDBACK_NONE) };
        if touch_device.is_null() {
            unsafe { DestroySyntheticPointerDevice(pen_device) };
            return Err(last_error("CreateSyntheticPointerDevice(PT_TOUCH)"));
        }
        Ok(Self {
            state: Mutex::new(InjectorState {
                pen_device,
                touch_device,
                pen: None,
                touches: BTreeMap::new(),
                mouse_buttons: 0,
                pressed_keys: BTreeMap::new(),
                wheel_remainder_x: 0.0,
                wheel_remainder_y: 0.0,
            }),
            target: RwLock::new(target),
            options: RwLock::new(options),
            target_window: AtomicUsize::new(0),
        })
    }

    pub fn set_target(&self, target: TargetGeometry) {
        *self.target.write() = target;
    }

    pub fn set_options(&self, options: PointerInjectorOptions) {
        *self.options.write() = options;
    }

    /// Directs injected messages to a specific HWND. Production leaves this unset so User32
    /// performs normal screen-coordinate hit testing; the native sink uses it for deterministic
    /// automation when the test runner's own topmost window obscures the desktop.
    pub fn set_target_window(&self, hwnd: usize) {
        self.target_window.store(hwnd, Ordering::Relaxed);
    }

    fn inject_pen(
        state: &mut InjectorState,
        sample: PointerSample,
        point: POINT,
        target_window: usize,
    ) -> Result<(), InputError> {
        if state.pen.is_some_and(|active| active.pointer_id != sample.pointer_id) {
            release_pen(state)?;
        }

        let pressure = sample.pressure_u32();
        let (tilt_x, tilt_y) = sample.tilt_i32();
        let was_down = state.pen.is_some_and(|active| active.down);
        let pointer_flags = match sample.action {
            Action::Down => {
                POINTER_FLAG_NEW
                    | POINTER_FLAG_INRANGE
                    | POINTER_FLAG_INCONTACT
                    | POINTER_FLAG_DOWN
                    | POINTER_FLAG_PRIMARY
                    | POINTER_FLAG_CONFIDENCE
            }
            Action::Move if was_down => {
                POINTER_FLAG_INRANGE
                    | POINTER_FLAG_INCONTACT
                    | POINTER_FLAG_UPDATE
                    | POINTER_FLAG_PRIMARY
                    | POINTER_FLAG_CONFIDENCE
            }
            Action::Move | Action::Hover => POINTER_FLAG_INRANGE | POINTER_FLAG_UPDATE | POINTER_FLAG_PRIMARY,
            Action::Up => POINTER_FLAG_INRANGE | POINTER_FLAG_UP | POINTER_FLAG_PRIMARY,
            Action::Cancel => POINTER_FLAG_CANCELED | POINTER_FLAG_UP | POINTER_FLAG_PRIMARY,
        };
        let button_change = match sample.action {
            Action::Down => POINTER_CHANGE_FIRSTBUTTON_DOWN,
            Action::Up | Action::Cancel => POINTER_CHANGE_FIRSTBUTTON_UP,
            Action::Move | Action::Hover => POINTER_CHANGE_NONE,
        };
        let pen_flags = pen_flags_from_browser_buttons(sample.flags);
        let pen_info = POINTER_PEN_INFO {
            pointerInfo: POINTER_INFO {
                pointerType: PT_PEN,
                pointerId: sample.pointer_id.max(1),
                frameId: 0,
                pointerFlags: pointer_flags,
                sourceDevice: std::ptr::null_mut(),
                hwndTarget: target_window as _,
                ptPixelLocation: point,
                ptHimetricLocation: POINT::default(),
                ptPixelLocationRaw: point,
                ptHimetricLocationRaw: POINT::default(),
                dwTime: 0,
                historyCount: 1,
                InputData: 0,
                dwKeyStates: 0,
                PerformanceCount: 0,
                ButtonChangeType: button_change,
            },
            penFlags: pen_flags,
            penMask: PEN_MASK_PRESSURE | PEN_MASK_ROTATION | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
            pressure,
            rotation: sample.twist_deg.round() as u32,
            tiltX: tilt_x,
            tiltY: tilt_y,
        };
        let info = POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 { penInfo: pen_info },
        };
        inject_with_retry(state.pen_device, &info, 1, "InjectSyntheticPointerInput(PT_PEN)")?;

        if sample.action.is_terminal() {
            state.pen = None;
        } else {
            state.pen = Some(ActivePointer {
                pointer_id: sample.pointer_id,
                point,
                pressure,
                tilt_x,
                tilt_y,
                down: matches!(sample.action, Action::Down | Action::Move)
                    && (sample.action == Action::Down || was_down),
            });
        }
        Ok(())
    }

    fn inject_touch(
        state: &mut InjectorState,
        sample: PointerSample,
        point: POINT,
        target_window: usize,
    ) -> Result<(), InputError> {
        let pointer_id = sample.pointer_id.max(1);
        match sample.action {
            Action::Down | Action::Move => {
                state.touches.insert(
                    pointer_id,
                    ActivePointer {
                        pointer_id,
                        point,
                        pressure: sample.pressure_u32().max(1),
                        tilt_x: 0,
                        tilt_y: 0,
                        down: true,
                    },
                );
            }
            Action::Up | Action::Cancel | Action::Hover => {}
        }

        let mut contacts = Vec::with_capacity(state.touches.len().max(1));
        for (&id, active) in &state.touches {
            let is_changed = id == pointer_id;
            let flags = if is_changed {
                match sample.action {
                    Action::Down => {
                        POINTER_FLAG_NEW | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_DOWN
                    }
                    Action::Move => POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_UPDATE,
                    Action::Up => POINTER_FLAG_UP,
                    Action::Cancel => POINTER_FLAG_CANCELED | POINTER_FLAG_UP,
                    Action::Hover => continue,
                }
            } else {
                POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_UPDATE
            } | POINTER_FLAG_CONFIDENCE;
            contacts.push(touch_info(*active, flags, target_window));
        }

        if sample.action.is_terminal() && !state.touches.contains_key(&pointer_id) {
            let terminal = ActivePointer {
                pointer_id,
                point,
                pressure: 1,
                tilt_x: 0,
                tilt_y: 0,
                down: false,
            };
            contacts.push(touch_info(
                terminal,
                POINTER_FLAG_UP
                    | if sample.action == Action::Cancel {
                        POINTER_FLAG_CANCELED
                    } else {
                        0
                    },
                target_window,
            ));
        }

        if !contacts.is_empty() {
            inject_with_retry(
                state.touch_device,
                contacts.as_ptr(),
                contacts.len() as u32,
                "InjectSyntheticPointerInput(PT_TOUCH)",
            )?;
        }
        if sample.action.is_terminal() {
            state.touches.remove(&pointer_id);
        }
        Ok(())
    }

    fn inject_mouse(state: &mut InjectorState, sample: PointerSample, point: POINT) -> Result<(), InputError> {
        let mut inputs = Vec::new();
        if sample.action != Action::Cancel {
            inputs.push(mouse_move_input(point));
        }
        append_mouse_button_changes(&mut inputs, state.mouse_buttons, sample.flags);
        send_native_inputs(&inputs, "SendInput(mouse)")?;
        state.mouse_buttons = if sample.action.is_terminal() { 0 } else { sample.flags };
        Ok(())
    }

    fn inject_wheel_native(state: &mut InjectorState, input: &WheelInput, point: POINT) -> Result<(), InputError> {
        // DOM horizontal deltas and WM_MOUSEHWHEEL both use positive=right.
        // DOM vertical positive=down, while WM_MOUSEWHEEL positive=up.
        state.wheel_remainder_x += scaled_wheel_delta(input.delta_x, true);
        state.wheel_remainder_y += scaled_wheel_delta(input.delta_y, false);
        let horizontal = take_integral_delta(&mut state.wheel_remainder_x);
        let vertical = take_integral_delta(&mut state.wheel_remainder_y);
        let mut inputs = vec![mouse_move_input(point)];
        if vertical != 0 {
            inputs.push(mouse_input(0, 0, vertical as u32, MOUSEEVENTF_WHEEL));
        }
        if horizontal != 0 {
            inputs.push(mouse_input(0, 0, horizontal as u32, MOUSEEVENTF_HWHEEL));
        }
        send_native_inputs(&inputs, "SendInput(wheel)")
    }

    fn inject_keyboard_native(state: &mut InjectorState, input: &KeyboardInput) -> Result<(), InputError> {
        let (virtual_key, extended) = virtual_key_for_code(&input.code)
            .ok_or_else(|| InputError(format!("unsupported DOM keyboard code {}", input.code)))?;
        let key = input.code.clone();
        match input.action {
            KeyAction::Down => {
                send_native_inputs(&[keyboard_input(virtual_key, false, extended)], "SendInput(key down)")?;
                state.pressed_keys.insert(key, virtual_key);
            }
            KeyAction::Up => {
                let virtual_key = state.pressed_keys.remove(&key).unwrap_or(virtual_key);
                send_native_inputs(&[keyboard_input(virtual_key, true, extended)], "SendInput(key up)")?;
            }
        }
        Ok(())
    }

    fn inject_text_native(input: &TextInput) -> Result<(), InputError> {
        let mut inputs = Vec::with_capacity(input.text.encode_utf16().count() * 2);
        let mut previous_was_carriage_return = false;
        for character in input.text.chars() {
            if character == '\r' || character == '\n' {
                if character == '\n' && previous_was_carriage_return {
                    previous_was_carriage_return = false;
                    continue;
                }
                inputs.push(keyboard_input(0x0D, false, false));
                inputs.push(keyboard_input(0x0D, true, false));
                previous_was_carriage_return = character == '\r';
                continue;
            }
            previous_was_carriage_return = false;
            let mut units = [0_u16; 2];
            for unit in character.encode_utf16(&mut units) {
                inputs.push(unicode_input(*unit, false));
                inputs.push(unicode_input(*unit, true));
            }
        }
        send_native_inputs(&inputs, "SendInput(Unicode text)")
    }

    fn inject_command_native(&self, command: RemoteCommand) -> Result<(), InputError> {
        match command {
            RemoteCommand::AppNext => send_chord(&[(0x12, false), (0x09, false)]),
            RemoteCommand::AppPrevious => send_chord(&[(0x12, false), (0x10, false), (0x09, false)]),
            RemoteCommand::TaskView => send_chord(&[(0x5B, true), (0x09, false)]),
            RemoteCommand::MinimizeForeground => {
                let foreground = unsafe { GetForegroundWindow() };
                if foreground.is_null() {
                    return Err(InputError("Windows has no foreground window to minimize".to_owned()));
                }
                unsafe {
                    ShowWindow(foreground, SW_MINIMIZE);
                }
                Ok(())
            }
            RemoteCommand::ResetInput => self.reset_all(),
        }
    }
}

impl InputSink for PointerInjector {
    fn inject_batch(&self, batch: &PointerBatch) -> Result<(), InputError> {
        let options = *self.options.read();
        let target = *self.target.read();
        let target_window = self.target_window.load(Ordering::Relaxed);
        let mut state = self.state.lock();
        for sample in &batch.samples {
            let point = target.map(NormalizedPoint {
                u: sample.x_norm,
                v: sample.y_norm,
            });
            let point = POINT { x: point.x, y: point.y };
            match sample.device_type {
                DeviceType::Pen if options.pen_enabled => Self::inject_pen(&mut state, *sample, point, target_window)?,
                DeviceType::Touch
                    if options.touch_enabled
                        && !(options.strict_palm_rejection && state.pen.is_some_and(|pen| pen.down)) =>
                {
                    Self::inject_touch(&mut state, *sample, point, target_window)?;
                }
                DeviceType::Mouse if options.mouse_enabled => Self::inject_mouse(&mut state, *sample, point)?,
                DeviceType::Pen | DeviceType::Touch | DeviceType::Mouse => {}
            }
        }
        Ok(())
    }

    fn inject_wheel(&self, input: &WheelInput) -> Result<(), InputError> {
        if !self.options.read().mouse_enabled {
            return Ok(());
        }
        let target = *self.target.read();
        let point = target.map(NormalizedPoint {
            u: input.x_norm,
            v: input.y_norm,
        });
        Self::inject_wheel_native(&mut self.state.lock(), input, POINT { x: point.x, y: point.y })
    }

    fn inject_keyboard(&self, input: &KeyboardInput) -> Result<(), InputError> {
        if !self.options.read().keyboard_enabled {
            return Ok(());
        }
        Self::inject_keyboard_native(&mut self.state.lock(), input)
    }

    fn inject_text(&self, input: &TextInput) -> Result<(), InputError> {
        if !self.options.read().keyboard_enabled {
            return Ok(());
        }
        Self::inject_text_native(input)
    }

    fn inject_command(&self, input: &CommandInput) -> Result<(), InputError> {
        if input.command != RemoteCommand::ResetInput && !self.options.read().gestures_enabled {
            return Ok(());
        }
        self.inject_command_native(input.command)
    }

    fn set_remote_input_options(&self, touch_enabled: bool, gestures_enabled: bool) -> Result<(), InputError> {
        self.reset_all()?;
        let mut options = self.options.write();
        options.touch_enabled = touch_enabled;
        options.gestures_enabled = gestures_enabled;
        Ok(())
    }

    fn reset_all(&self) -> Result<(), InputError> {
        let mut state = self.state.lock();
        release_pen(&mut state)?;
        if !state.touches.is_empty() {
            let contacts: Vec<_> = state
                .touches
                .values()
                .copied()
                .map(|active| {
                    touch_info(
                        active,
                        POINTER_FLAG_CANCELED | POINTER_FLAG_UP,
                        self.target_window.load(Ordering::Relaxed),
                    )
                })
                .collect();
            if unsafe { InjectSyntheticPointerInput(state.touch_device, contacts.as_ptr(), contacts.len() as u32) } == 0
            {
                return Err(last_error("reset PT_TOUCH"));
            }
            state.touches.clear();
        }
        release_remote_state(&mut state)?;
        Ok(())
    }
}

fn mouse_move_input(point: POINT) -> INPUT {
    let (x, y) = virtual_desktop_coordinates(point);
    mouse_input(
        x,
        y,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn keyboard_input(virtual_key: u16, key_up: bool, extended: bool) -> INPUT {
    let mut flags = 0;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn unicode_input(unit: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_native_inputs(inputs: &[INPUT], context: &str) -> Result<(), InputError> {
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(last_error(&format!("{context} sent {sent}/{} events", inputs.len())))
    }
}

fn send_chord(keys: &[(u16, bool)]) -> Result<(), InputError> {
    let mut inputs = Vec::with_capacity(keys.len() * 2);
    inputs.extend(keys.iter().map(|&(key, extended)| keyboard_input(key, false, extended)));
    inputs.extend(
        keys.iter()
            .rev()
            .map(|&(key, extended)| keyboard_input(key, true, extended)),
    );
    send_native_inputs(&inputs, "SendInput(command chord)")
}

fn virtual_desktop_coordinates(point: POINT) -> (i32, i32) {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    let x = i64::from(point.x - left).clamp(0, i64::from(width.saturating_sub(1)));
    let y = i64::from(point.y - top).clamp(0, i64::from(height.saturating_sub(1)));
    (
        (x * 65_535 / i64::from(width.saturating_sub(1).max(1))) as i32,
        (y * 65_535 / i64::from(height.saturating_sub(1).max(1))) as i32,
    )
}

fn append_mouse_button_changes(inputs: &mut Vec<INPUT>, previous: u16, current: u16) {
    append_button(
        inputs,
        previous,
        current,
        BROWSER_BUTTON_PRIMARY,
        MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP,
        0,
    );
    append_button(
        inputs,
        previous,
        current,
        BROWSER_BUTTON_SECONDARY,
        MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP,
        0,
    );
    append_button(
        inputs,
        previous,
        current,
        BROWSER_BUTTON_AUXILIARY,
        MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP,
        0,
    );
    append_button(
        inputs,
        previous,
        current,
        BROWSER_BUTTON_BACK,
        MOUSEEVENTF_XDOWN,
        MOUSEEVENTF_XUP,
        XBUTTON1,
    );
    append_button(
        inputs,
        previous,
        current,
        BROWSER_BUTTON_FORWARD,
        MOUSEEVENTF_XDOWN,
        MOUSEEVENTF_XUP,
        XBUTTON2,
    );
}

fn append_button(inputs: &mut Vec<INPUT>, previous: u16, current: u16, mask: u16, down: u32, up: u32, data: u32) {
    if previous & mask == 0 && current & mask != 0 {
        inputs.push(mouse_input(0, 0, data, down));
    } else if previous & mask != 0 && current & mask == 0 {
        inputs.push(mouse_input(0, 0, data, up));
    }
}

fn take_integral_delta(remainder: &mut f32) -> i32 {
    let whole = remainder.trunc().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    *remainder -= whole as f32;
    whole
}

fn scaled_wheel_delta(delta: f32, horizontal: bool) -> f32 {
    delta * WHEEL_SCALE * if horizontal { 1.0 } else { -1.0 }
}

fn release_remote_state(state: &mut InjectorState) -> Result<(), InputError> {
    let mut inputs = Vec::new();
    append_mouse_button_changes(&mut inputs, state.mouse_buttons, 0);
    for virtual_key in state.pressed_keys.values().rev() {
        inputs.push(keyboard_input(
            *virtual_key,
            true,
            is_extended_virtual_key(*virtual_key),
        ));
    }
    send_native_inputs(&inputs, "SendInput(reset remote input)")?;
    state.mouse_buttons = 0;
    state.pressed_keys.clear();
    state.wheel_remainder_x = 0.0;
    state.wheel_remainder_y = 0.0;
    Ok(())
}

fn virtual_key_for_code(code: &str) -> Option<(u16, bool)> {
    let key = match code {
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Enter" | "NumpadEnter" => 0x0D,
        "ShiftLeft" => 0xA0,
        "ShiftRight" => 0xA1,
        "ControlLeft" => 0xA2,
        "ControlRight" => 0xA3,
        "AltLeft" => 0xA4,
        "AltRight" => 0xA5,
        "Pause" => 0x13,
        "CapsLock" => 0x14,
        "KanaMode" => 0x15,
        "Escape" => 0x1B,
        "Convert" => 0x1C,
        "NonConvert" => 0x1D,
        "Space" => 0x20,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "End" => 0x23,
        "Home" => 0x24,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "PrintScreen" => 0x2C,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Help" => 0x2F,
        "MetaLeft" | "OSLeft" => 0x5B,
        "MetaRight" | "OSRight" => 0x5C,
        "ContextMenu" => 0x5D,
        "Semicolon" => 0xBA,
        "Equal" => 0xBB,
        "Comma" => 0xBC,
        "Minus" => 0xBD,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backquote" => 0xC0,
        "BracketLeft" => 0xDB,
        "Backslash" => 0xDC,
        "BracketRight" => 0xDD,
        "Quote" => 0xDE,
        "IntlBackslash" => 0xE2,
        "NumpadMultiply" => 0x6A,
        "NumpadAdd" => 0x6B,
        "NumpadSubtract" => 0x6D,
        "NumpadDecimal" => 0x6E,
        "NumpadDivide" => 0x6F,
        "NumLock" => 0x90,
        "ScrollLock" => 0x91,
        "BrowserBack" => 0xA6,
        "BrowserForward" => 0xA7,
        "BrowserRefresh" => 0xA8,
        "BrowserStop" => 0xA9,
        "BrowserSearch" => 0xAA,
        "BrowserFavorites" => 0xAB,
        "BrowserHome" => 0xAC,
        "AudioVolumeMute" => 0xAD,
        "AudioVolumeDown" => 0xAE,
        "AudioVolumeUp" => 0xAF,
        "MediaTrackNext" => 0xB0,
        "MediaTrackPrevious" => 0xB1,
        "MediaStop" => 0xB2,
        "MediaPlayPause" => 0xB3,
        "LaunchMail" => 0xB4,
        "MediaSelect" => 0xB5,
        "LaunchApp1" => 0xB6,
        "LaunchApp2" => 0xB7,
        _ if code.len() == 4 && code.starts_with("Key") => u16::from(code.as_bytes()[3].to_ascii_uppercase()),
        _ if code.len() == 6 && code.starts_with("Digit") => u16::from(code.as_bytes()[5]),
        _ if let Some(number) = code.strip_prefix('F').and_then(|value| value.parse::<u16>().ok())
            && (1..=24).contains(&number) =>
        {
            0x6F + number
        }
        _ if let Some(number) = code.strip_prefix("Numpad").and_then(|value| value.parse::<u16>().ok())
            && number <= 9 =>
        {
            0x60 + number
        }
        _ => return None,
    };
    Some((key, is_extended_virtual_key(key)))
}

fn is_extended_virtual_key(key: u16) -> bool {
    matches!(
        key,
        0x21..=0x28 | 0x2D | 0x2E | 0x5B..=0x5D | 0x6F | 0xA3 | 0xA5
    )
}

fn inject_with_retry(
    device: HSYNTHETICPOINTERDEVICE,
    info: *const POINTER_TYPE_INFO,
    count: u32,
    context: &str,
) -> Result<(), InputError> {
    let started = Instant::now();
    loop {
        if unsafe { InjectSyntheticPointerInput(device, info, count) } != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NOT_READY) || started.elapsed() >= INJECTION_RETRY_LIMIT {
            return Err(InputError(format!("{context} failed: {error}")));
        }
        std::thread::sleep(INJECTION_RETRY_BACKOFF);
    }
}

fn pen_flags_from_browser_buttons(buttons: u16) -> u32 {
    // Pointer Events uses bit 0 for the primary pen tip and bit 1 for the
    // secondary/barrel button. Treating the tip bit as PEN_FLAG_BARREL turns
    // every ordinary stroke into a Windows right-click gesture.
    if buttons & BROWSER_BUTTON_SECONDARY != 0 {
        PEN_FLAG_BARREL
    } else {
        PEN_FLAG_NONE
    }
}

fn release_pen(state: &mut InjectorState) -> Result<(), InputError> {
    let Some(active) = state.pen.take() else {
        return Ok(());
    };
    if !active.down {
        return Ok(());
    }
    let pen_info = POINTER_PEN_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_PEN,
            pointerId: active.pointer_id.max(1),
            pointerFlags: POINTER_FLAG_INRANGE | POINTER_FLAG_UP | POINTER_FLAG_PRIMARY,
            ptPixelLocation: active.point,
            ptPixelLocationRaw: active.point,
            historyCount: 1,
            ButtonChangeType: POINTER_CHANGE_FIRSTBUTTON_UP,
            ..Default::default()
        },
        penMask: PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
        pressure: active.pressure,
        tiltX: active.tilt_x,
        tiltY: active.tilt_y,
        ..Default::default()
    };
    let info = POINTER_TYPE_INFO {
        r#type: PT_PEN,
        Anonymous: POINTER_TYPE_INFO_0 { penInfo: pen_info },
    };
    if unsafe { InjectSyntheticPointerInput(state.pen_device, &info, 1) } == 0 {
        return Err(last_error("reset PT_PEN"));
    }
    Ok(())
}

fn touch_info(active: ActivePointer, flags: u32, target_window: usize) -> POINTER_TYPE_INFO {
    let radius = 4;
    let touch_info = POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            pointerId: active.pointer_id,
            pointerFlags: flags,
            hwndTarget: target_window as _,
            ptPixelLocation: active.point,
            ptPixelLocationRaw: active.point,
            historyCount: 1,
            ..Default::default()
        },
        touchFlags: TOUCH_FLAG_NONE,
        touchMask: TOUCH_MASK_CONTACTAREA | TOUCH_MASK_ORIENTATION | TOUCH_MASK_PRESSURE,
        rcContact: RECT {
            left: active.point.x - radius,
            top: active.point.y - radius,
            right: active.point.x + radius,
            bottom: active.point.y + radius,
        },
        rcContactRaw: RECT::default(),
        orientation: 90,
        pressure: active.pressure.max(1),
    };
    POINTER_TYPE_INFO {
        r#type: PT_TOUCH,
        Anonymous: POINTER_TYPE_INFO_0 { touchInfo: touch_info },
    }
}

pub fn set_per_monitor_dpi_awareness() -> Result<(), InputError> {
    if unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } == 0 {
        let error = std::io::Error::last_os_error();
        // ERROR_ACCESS_DENIED means awareness was already fixed by the application manifest or runtime.
        if error.raw_os_error() != Some(5) {
            return Err(InputError(format!("SetProcessDpiAwarenessContext failed: {error}")));
        }
    }
    Ok(())
}

fn last_error(context: &str) -> InputError {
    InputError(format!("{context} failed: {}", std::io::Error::last_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_and_tilt_conversion_match_windows_ranges() {
        let sample = PointerSample {
            device_type: DeviceType::Pen,
            action: Action::Move,
            flags: 0,
            pointer_id: 1,
            sample_sequence: 1,
            x_norm: 0.5,
            y_norm: 0.5,
            pressure: 0.75,
            tilt_x_deg: -30.4,
            tilt_y_deg: 91.0,
            twist_deg: 0.0,
            client_time_ms: 0.0,
        };
        assert_eq!(sample.pressure_u32(), 768);
        assert_eq!(sample.tilt_i32(), (-30, 90));
    }

    #[test]
    fn primary_pen_contact_never_sets_the_barrel_flag() {
        assert_eq!(pen_flags_from_browser_buttons(0), PEN_FLAG_NONE);
        assert_eq!(pen_flags_from_browser_buttons(1), PEN_FLAG_NONE);
        assert_eq!(pen_flags_from_browser_buttons(2), PEN_FLAG_BARREL);
        assert_eq!(pen_flags_from_browser_buttons(3), PEN_FLAG_BARREL);
    }

    #[test]
    fn dom_keyboard_codes_cover_modifiers_navigation_and_full_key_rows() {
        assert_eq!(virtual_key_for_code("AltLeft"), Some((0xA4, false)));
        assert_eq!(virtual_key_for_code("ControlRight"), Some((0xA3, true)));
        assert_eq!(virtual_key_for_code("Delete"), Some((0x2E, true)));
        assert_eq!(virtual_key_for_code("KeyZ"), Some((0x5A, false)));
        assert_eq!(virtual_key_for_code("Digit7"), Some((0x37, false)));
        assert_eq!(virtual_key_for_code("F24"), Some((0x87, false)));
        assert_eq!(virtual_key_for_code("Numpad9"), Some((0x69, false)));
        assert_eq!(virtual_key_for_code("MediaPlayPause"), Some((0xB3, false)));
        assert_eq!(virtual_key_for_code("OSLeft"), Some((0x5B, true)));
        assert_eq!(virtual_key_for_code("Unidentified"), None);
    }

    #[test]
    fn browser_mouse_button_transitions_map_to_windows_flags() {
        let mut inputs = Vec::new();
        append_mouse_button_changes(&mut inputs, 0, BROWSER_BUTTON_PRIMARY | BROWSER_BUTTON_SECONDARY);
        assert_eq!(inputs.len(), 2);
        let flags: Vec<_> = inputs
            .iter()
            .map(|input| unsafe { input.Anonymous.mi.dwFlags })
            .collect();
        assert_eq!(flags, vec![MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_RIGHTDOWN]);

        inputs.clear();
        append_mouse_button_changes(&mut inputs, BROWSER_BUTTON_PRIMARY | BROWSER_BUTTON_SECONDARY, 0);
        let flags: Vec<_> = inputs
            .iter()
            .map(|input| unsafe { input.Anonymous.mi.dwFlags })
            .collect();
        assert_eq!(flags, vec![MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTUP]);
    }

    #[test]
    fn high_resolution_wheel_deltas_retain_fractional_remainder() {
        assert!((scaled_wheel_delta(100.0, true) - 120.0).abs() < 0.001);
        assert!((scaled_wheel_delta(100.0, false) + 120.0).abs() < 0.001);
        let mut remainder = 0.25;
        assert_eq!(take_integral_delta(&mut remainder), 0);
        remainder += 1.5;
        assert_eq!(take_integral_delta(&mut remainder), 1);
        assert!((remainder - 0.75).abs() < f32::EPSILON);
    }
}
