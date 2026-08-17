use std::collections::BTreeMap;

use nfidb_core::{InputError, InputSink};
use nfidb_protocol::{Action, DeviceType, NormalizedPoint, PointerBatch, PointerSample, TargetGeometry};
use parking_lot::{Mutex, RwLock};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE, POINTER_FEEDBACK_NONE,
    POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_CHANGE_FIRSTBUTTON_DOWN, POINTER_CHANGE_FIRSTBUTTON_UP, POINTER_CHANGE_NONE,
    POINTER_FLAG_CANCELED, POINTER_FLAG_CONFIDENCE, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_NEW, POINTER_FLAG_PRIMARY, POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_PEN_INFO,
    POINTER_TOUCH_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_BARREL, PEN_FLAG_NONE, PEN_MASK_PRESSURE, PEN_MASK_ROTATION, PEN_MASK_TILT_X, PEN_MASK_TILT_Y, PT_PEN,
    PT_TOUCH, TOUCH_FLAG_NONE, TOUCH_MASK_CONTACTAREA, TOUCH_MASK_ORIENTATION, TOUCH_MASK_PRESSURE,
};

const MAX_TOUCH_CONTACTS: u32 = 10;

#[derive(Debug, Clone, Copy)]
pub struct PointerInjectorOptions {
    pub pen_enabled: bool,
    pub touch_enabled: bool,
    pub strict_palm_rejection: bool,
}

impl Default for PointerInjectorOptions {
    fn default() -> Self {
        Self {
            pen_enabled: true,
            touch_enabled: false,
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
            }),
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

    fn inject_pen(state: &mut InjectorState, sample: PointerSample, point: POINT) -> Result<(), InputError> {
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
        let pen_flags = if sample.flags & 1 != 0 {
            PEN_FLAG_BARREL
        } else {
            PEN_FLAG_NONE
        };
        let pen_info = POINTER_PEN_INFO {
            pointerInfo: POINTER_INFO {
                pointerType: PT_PEN,
                pointerId: sample.pointer_id.max(1),
                frameId: 0,
                pointerFlags: pointer_flags,
                sourceDevice: std::ptr::null_mut(),
                hwndTarget: std::ptr::null_mut(),
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
        if unsafe { InjectSyntheticPointerInput(state.pen_device, &info, 1) } == 0 {
            return Err(last_error("InjectSyntheticPointerInput(PT_PEN)"));
        }

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

    fn inject_touch(state: &mut InjectorState, sample: PointerSample, point: POINT) -> Result<(), InputError> {
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
            contacts.push(touch_info(*active, flags));
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
            ));
        }

        if !contacts.is_empty()
            && unsafe { InjectSyntheticPointerInput(state.touch_device, contacts.as_ptr(), contacts.len() as u32) } == 0
        {
            return Err(last_error("InjectSyntheticPointerInput(PT_TOUCH)"));
        }
        if sample.action.is_terminal() {
            state.touches.remove(&pointer_id);
        }
        Ok(())
    }
}

impl InputSink for PointerInjector {
    fn inject_batch(&self, batch: &PointerBatch) -> Result<(), InputError> {
        let options = *self.options.read();
        let target = *self.target.read();
        let mut state = self.state.lock();
        for sample in &batch.samples {
            let point = target.map(NormalizedPoint {
                u: sample.x_norm,
                v: sample.y_norm,
            });
            let point = POINT { x: point.x, y: point.y };
            match sample.device_type {
                DeviceType::Pen if options.pen_enabled => Self::inject_pen(&mut state, *sample, point)?,
                DeviceType::Touch
                    if options.touch_enabled
                        && !(options.strict_palm_rejection && state.pen.is_some_and(|pen| pen.down)) =>
                {
                    Self::inject_touch(&mut state, *sample, point)?;
                }
                DeviceType::Pen | DeviceType::Touch => {}
            }
        }
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
                .map(|active| touch_info(active, POINTER_FLAG_CANCELED | POINTER_FLAG_UP))
                .collect();
            if unsafe { InjectSyntheticPointerInput(state.touch_device, contacts.as_ptr(), contacts.len() as u32) } == 0
            {
                return Err(last_error("reset PT_TOUCH"));
            }
            state.touches.clear();
        }
        Ok(())
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

fn touch_info(active: ActivePointer, flags: u32) -> POINTER_TYPE_INFO {
    let radius = 4;
    let touch_info = POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            pointerId: active.pointer_id,
            pointerFlags: flags,
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
}
