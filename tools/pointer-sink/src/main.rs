use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nfidb_core::InputSink;
use nfidb_host_windows::{PointerInjector, PointerInjectorOptions, set_per_monitor_dpi_awareness};
use nfidb_protocol::{Action, DeviceType, PointerBatch, PointerSample, TargetGeometry};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::Pointer::{GetPointerPenInfo, POINTER_PEN_INFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GWLP_USERDATA, GetMessageW, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SW_SHOW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, WM_DESTROY, WM_NCCREATE,
    WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

#[derive(Default)]
struct SinkState {
    events: u32,
    pressures: Vec<u32>,
    tilt_x: Vec<i32>,
    tilt_y: Vec<i32>,
    self_test: bool,
    complete: Arc<AtomicBool>,
}

fn main() {
    if let Err(error) = set_per_monitor_dpi_awareness() {
        eprintln!("DPI awareness warning: {error}");
    }
    let self_test = std::env::args().any(|argument| argument == "--self-test");
    match run(self_test) {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(2),
        Err(error) => {
            eprintln!("pointer-sink failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run(self_test: bool) -> Result<bool, String> {
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let class_name = wide("NFiDBPointerSink");
    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(format!("RegisterClassW failed: {}", std::io::Error::last_os_error()));
    }
    let mut state = Box::new(SinkState {
        self_test,
        ..Default::default()
    });
    let title = wide("NFiDB Pointer Sink — draw here with a pen");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            if self_test { 100 } else { CW_USEDEFAULT },
            if self_test { 100 } else { CW_USEDEFAULT },
            720,
            520,
            null_mut(),
            null_mut(),
            instance,
            (&mut *state as *mut SinkState).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(format!("CreateWindowExW failed: {}", std::io::Error::last_os_error()));
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
    if self_test {
        spawn_self_test(hwnd, Arc::clone(&state.complete));
    }
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    if !self_test {
        return Ok(true);
    }
    let pressure_ok = state.pressures.iter().any(|pressure| *pressure <= 150)
        && state.pressures.iter().any(|pressure| (450..=600).contains(pressure))
        && state.pressures.iter().any(|pressure| *pressure >= 950);
    let tilt_ok = state.tilt_x.contains(&30) && state.tilt_y.contains(&-30);
    let result = serde_json::json!({
        "events": state.events,
        "pressures": state.pressures,
        "tilt_x": state.tilt_x,
        "tilt_y": state.tilt_y,
        "pressure_ok": pressure_ok,
        "tilt_ok": tilt_ok,
        "pen_released": state.complete.load(Ordering::Acquire),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
    );
    Ok(pressure_ok && tilt_ok && state.complete.load(Ordering::Acquire))
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        if !create.is_null() {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize) };
        }
    }
    let state_ptr = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
        as *mut SinkState;
    if matches!(message, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) && !state_ptr.is_null() {
        let pointer_id = (wparam as u32) & 0xffff;
        let mut info = POINTER_PEN_INFO::default();
        if unsafe { GetPointerPenInfo(pointer_id, &mut info) } != 0 {
            let state = unsafe { &mut *state_ptr };
            state.events = state.events.saturating_add(1);
            state.pressures.push(info.pressure);
            state.tilt_x.push(info.tiltX);
            state.tilt_y.push(info.tiltY);
            let title = wide(&format!(
                "NFiDB Pointer Sink · ID {pointer_id} · pressure {} / 1024 · tilt {}° / {}° · {} events",
                info.pressure, info.tiltX, info.tiltY, state.events
            ));
            unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
            if message == WM_POINTERUP && state.self_test && state.events >= 4 {
                state.complete.store(true, Ordering::Release);
                unsafe { PostQuitMessage(0) };
            }
            return 0;
        }
    }
    if message == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn spawn_self_test(hwnd: HWND, complete: Arc<AtomicBool>) {
    let hwnd_value = hwnd as usize;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(350));
        let hwnd = hwnd_value as HWND;
        unsafe { SetForegroundWindow(hwnd) };
        let injector = match PointerInjector::new(
            TargetGeometry {
                left: 120,
                top: 145,
                width: 640,
                height: 410,
            },
            PointerInjectorOptions::default(),
        ) {
            Ok(injector) => injector,
            Err(error) => {
                eprintln!("injector initialization failed: {error}");
                unsafe { PostQuitMessage(1) };
                return;
            }
        };
        let samples = [
            make_sample(Action::Down, 1, 0.1, 0.0, 0.0, 0.20),
            make_sample(Action::Move, 2, 0.5, 30.0, 0.0, 0.35),
            make_sample(Action::Move, 3, 1.0, 0.0, -30.0, 0.50),
            make_sample(Action::Up, 4, 0.0, 30.0, -30.0, 0.65),
        ];
        for (index, sample) in samples.into_iter().enumerate() {
            let batch = PointerBatch {
                batch_sequence: index as u32,
                client_send_time_ms: 0.0,
                samples: vec![sample],
            };
            if let Err(error) = injector.inject_batch(&batch) {
                eprintln!("self-test injection failed: {error}");
                break;
            }
            std::thread::sleep(Duration::from_millis(45));
        }
        let _ = injector.reset_all();
        std::thread::sleep(Duration::from_secs(3));
        if !complete.load(Ordering::Acquire) {
            unsafe { PostQuitMessage(2) };
        }
    });
}

fn make_sample(action: Action, sequence: u32, pressure: f32, tilt_x: f32, tilt_y: f32, x: f32) -> PointerSample {
    PointerSample {
        device_type: DeviceType::Pen,
        action,
        flags: 0,
        pointer_id: 7,
        sample_sequence: sequence,
        x_norm: x,
        y_norm: 0.45,
        pressure,
        tilt_x_deg: tilt_x,
        tilt_y_deg: tilt_y,
        twist_deg: 0.0,
        client_time_ms: 0.0,
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
