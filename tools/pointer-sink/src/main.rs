use std::f32::consts::TAU;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use nfidb_core::InputSink;
use nfidb_host_windows::{PointerInjector, PointerInjectorOptions, set_per_monitor_dpi_awareness};
use nfidb_protocol::{Action, DeviceType, PointerBatch, PointerSample, TargetGeometry};
use serde_json::Value;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::AttachThreadInput;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Input::Pointer::{
    GetPointerPenInfo, GetPointerPenInfoHistory, POINTER_FLAG_DOWN, POINTER_FLAG_UP, POINTER_PEN_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GA_ROOT, GWLP_USERDATA, GetAncestor, GetClientRect, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, HWND_TOPMOST, IDC_ARROW, LoadCursorW, MSG, PEN_FLAG_BARREL, PostMessageW,
    PostQuitMessage, RegisterClassW, SW_SHOW, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, TranslateMessage, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WM_POINTERDOWN, WM_POINTERUP,
    WM_POINTERUPDATE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WindowFromPoint,
};

#[derive(Debug, Parser)]
#[command(
    name = "pointer-sink",
    about = "NFiDB native Windows pen diagnostic and stress receiver"
)]
struct Cli {
    #[arg(
        long,
        conflicts_with = "stress_test",
        help = "Run the fast deterministic four-sample check"
    )]
    self_test: bool,
    #[arg(
        long,
        conflicts_with = "self_test",
        help = "Run a sustained pressure/tilt injection benchmark"
    )]
    stress_test: bool,
    #[arg(long, default_value_t = 7200, requires = "stress_test")]
    samples: u32,
    #[arg(long, default_value_t = 240, requires = "stress_test")]
    rate: u32,
    #[arg(long, default_value_t = 4, requires = "stress_test")]
    batch_size: usize,
    #[arg(long, requires = "stress_test")]
    json_output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
struct TestOptions {
    expected_samples: u32,
    rate: u32,
    batch_size: usize,
    quick: bool,
}

impl TestOptions {
    fn from_cli(cli: &Cli) -> Option<Self> {
        if cli.self_test {
            Some(Self {
                expected_samples: 4,
                rate: 25,
                batch_size: 1,
                quick: true,
            })
        } else if cli.stress_test {
            Some(Self {
                expected_samples: cli.samples.clamp(3, 1_000_000),
                rate: cli.rate.clamp(1, 10_000),
                batch_size: cli.batch_size.clamp(1, 512),
                quick: false,
            })
        } else {
            None
        }
    }
}

#[derive(Default)]
struct SinkState {
    messages: u32,
    received: Vec<POINTER_PEN_INFO>,
    down_samples: u32,
    up_samples: u32,
    barrel_samples: u32,
    options: Option<TestOptions>,
    complete: Arc<AtomicBool>,
    first_received: Option<Instant>,
    last_received: Option<Instant>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = set_per_monitor_dpi_awareness() {
        eprintln!("DPI awareness warning: {error}");
    }
    match run(TestOptions::from_cli(&cli)) {
        Ok(report) => {
            if let Some(report) = report {
                let json = serde_json::to_string_pretty(&report).unwrap_or_else(|error| error.to_string());
                println!("{json}");
                if let Some(path) = cli.json_output
                    && let Err(error) = std::fs::write(&path, json)
                {
                    eprintln!("failed to write {}: {error}", path.display());
                    std::process::exit(1);
                }
                if report.get("pass").and_then(Value::as_bool) != Some(true) {
                    std::process::exit(2);
                }
            }
        }
        Err(error) => {
            eprintln!("pointer-sink failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run(options: Option<TestOptions>) -> Result<Option<Value>, String> {
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
        options,
        ..Default::default()
    });
    let title = wide("NFiDB Pointer Sink — draw here with a pen");
    let automated = options.is_some();
    let hwnd = unsafe {
        CreateWindowExW(
            if automated { 0x0000_0008 } else { 0 },
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            if automated { 100 } else { CW_USEDEFAULT },
            if automated { 100 } else { CW_USEDEFAULT },
            900,
            620,
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
        SetWindowPos(hwnd, HWND_TOPMOST, 100, 100, 900, 620, SWP_SHOWWINDOW);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
    if let Some(options) = options {
        spawn_test(hwnd, Arc::clone(&state.complete), options);
    }
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(options.map(|options| build_report(&state, options)))
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
        let mut current = POINTER_PEN_INFO::default();
        if unsafe { GetPointerPenInfo(pointer_id, &mut current) } != 0 {
            let state = unsafe { &mut *state_ptr };
            let mut history = if message == WM_POINTERUPDATE && current.pointerInfo.historyCount > 1 {
                let mut count = current.pointerInfo.historyCount;
                let mut entries = vec![POINTER_PEN_INFO::default(); count as usize];
                if unsafe { GetPointerPenInfoHistory(pointer_id, &mut count, entries.as_mut_ptr()) } != 0 {
                    entries.truncate(count.min(entries.len() as u32) as usize);
                    entries.reverse();
                    entries
                } else {
                    vec![current]
                }
            } else {
                vec![current]
            };
            state.messages = state.messages.saturating_add(1);
            state.first_received.get_or_insert_with(Instant::now);
            state.last_received = Some(Instant::now());
            for info in &history {
                if info.pointerInfo.pointerFlags & POINTER_FLAG_DOWN != 0 {
                    state.down_samples = state.down_samples.saturating_add(1);
                }
                if info.pointerInfo.pointerFlags & POINTER_FLAG_UP != 0 {
                    state.up_samples = state.up_samples.saturating_add(1);
                }
                if info.penFlags & PEN_FLAG_BARREL != 0 {
                    state.barrel_samples = state.barrel_samples.saturating_add(1);
                }
            }
            state.received.append(&mut history);
            let title = wide(&format!(
                "NFiDB Pointer Sink · ID {pointer_id} · pressure {} / 1024 · tilt {}° / {}° · {} samples / {} messages",
                current.pressure,
                current.tiltX,
                current.tiltY,
                state.received.len(),
                state.messages
            ));
            unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
            if message == WM_POINTERUP && state.options.is_some() {
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

fn spawn_test(hwnd: HWND, complete: Arc<AtomicBool>, options: TestOptions) {
    let hwnd_value = hwnd as usize;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(350));
        let hwnd = hwnd_value as HWND;
        force_foreground(hwnd);
        let mut client = RECT::default();
        let mut origin = POINT::default();
        if unsafe { GetClientRect(hwnd, &mut client) } == 0 || unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
            eprintln!(
                "failed to resolve pointer-sink client rectangle: {}",
                std::io::Error::last_os_error()
            );
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            return;
        }
        eprintln!(
            "pointer-sink hwnd={:?} foreground={:?} hit={:?} root_hit={:?} client_origin={},{} client_size={}x{}",
            hwnd,
            unsafe { GetForegroundWindow() },
            unsafe {
                WindowFromPoint(POINT {
                    x: origin.x + (client.right - client.left) / 2,
                    y: origin.y + (client.bottom - client.top) / 2,
                })
            },
            unsafe {
                GetAncestor(
                    WindowFromPoint(POINT {
                        x: origin.x + (client.right - client.left) / 2,
                        y: origin.y + (client.bottom - client.top) / 2,
                    }),
                    GA_ROOT,
                )
            },
            origin.x,
            origin.y,
            client.right - client.left,
            client.bottom - client.top
        );
        let visible_point = (8..(client.bottom - client.top - 8)).step_by(12).find_map(|y| {
            (8..(client.right - client.left - 8)).step_by(12).find_map(|x| {
                let point = POINT {
                    x: origin.x + x,
                    y: origin.y + y,
                };
                let hit = unsafe { WindowFromPoint(point) };
                (unsafe { GetAncestor(hit, GA_ROOT) } == hwnd).then_some(point)
            })
        });
        let target_point = visible_point.unwrap_or(POINT {
            x: origin.x + (client.right - client.left) / 2,
            y: origin.y + (client.bottom - client.top) / 2,
        });
        let injector = match PointerInjector::new(
            TargetGeometry {
                left: target_point.x,
                top: target_point.y,
                width: 1,
                height: 1,
            },
            PointerInjectorOptions::default(),
        ) {
            Ok(injector) => injector,
            Err(error) => {
                eprintln!("injector initialization failed: {error}");
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                return;
            }
        };
        injector.set_target_window(hwnd as usize);
        let started = Instant::now();
        let mut next_sequence = 1_u32;
        let mut batch_sequence = 0_u32;
        while next_sequence <= options.expected_samples {
            let remaining = (options.expected_samples - next_sequence + 1) as usize;
            let count = remaining.min(options.batch_size);
            let samples = (0..count)
                .map(|offset| make_sample(next_sequence + offset as u32, options))
                .collect();
            let batch = PointerBatch {
                batch_sequence,
                client_send_time_ms: started.elapsed().as_secs_f64() * 1000.0,
                samples,
            };
            if let Err(error) = injector.inject_batch(&batch) {
                eprintln!("automated injection failed at sequence {next_sequence}: {error}");
                break;
            }
            next_sequence = next_sequence.saturating_add(count as u32);
            batch_sequence = batch_sequence.wrapping_add(1);
            let expected_elapsed =
                Duration::from_secs_f64(f64::from(next_sequence.saturating_sub(1)) / f64::from(options.rate));
            if let Some(delay) = expected_elapsed.checked_sub(started.elapsed()) {
                std::thread::sleep(delay);
            }
        }
        let _ = injector.reset_all();
        let timeout = Duration::from_secs_f64(f64::from(options.expected_samples) / f64::from(options.rate) + 8.0);
        while !complete.load(Ordering::Acquire) && started.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !complete.load(Ordering::Acquire) {
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        }
    });
}

fn force_foreground(hwnd: HWND) {
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_thread = if foreground.is_null() {
        0
    } else {
        unsafe { GetWindowThreadProcessId(foreground, null_mut()) }
    };
    let window_thread = unsafe { GetWindowThreadProcessId(hwnd, null_mut()) };
    let attached = foreground_thread != 0
        && foreground_thread != window_thread
        && unsafe { AttachThreadInput(window_thread, foreground_thread, 1) } != 0;
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, 100, 100, 900, 620, SWP_SHOWWINDOW);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }
    if attached {
        unsafe { AttachThreadInput(window_thread, foreground_thread, 0) };
    }
}

fn make_sample(sequence: u32, options: TestOptions) -> PointerSample {
    if options.quick {
        let values = [
            (Action::Down, 0.1, 0.0, 0.0, 0.20),
            (Action::Move, 0.5, 30.0, 0.0, 0.35),
            (Action::Move, 1.0, 0.0, -30.0, 0.50),
            (Action::Up, 0.0, 30.0, -30.0, 0.65),
        ];
        let (action, pressure, tilt_x, tilt_y, x) = values[(sequence - 1) as usize];
        return sample(sequence, action, pressure, tilt_x, tilt_y, x, 0.45);
    }
    let progress = (sequence - 1) as f32 / (options.expected_samples - 1) as f32;
    let phase = progress * TAU * 12.0;
    let action = if sequence == 1 {
        Action::Down
    } else if sequence == options.expected_samples {
        Action::Up
    } else {
        Action::Move
    };
    let pressure = if action == Action::Up {
        0.0
    } else {
        0.05 + 0.95 * (0.5 + 0.5 * phase.sin())
    };
    sample(
        sequence,
        action,
        pressure,
        60.0 * phase.sin(),
        60.0 * phase.cos(),
        0.05 + progress * 0.9,
        0.5 + 0.35 * (progress * TAU * 5.0).sin(),
    )
}

fn sample(sequence: u32, action: Action, pressure: f32, tilt_x: f32, tilt_y: f32, x: f32, y: f32) -> PointerSample {
    PointerSample {
        device_type: DeviceType::Pen,
        action,
        // Safari Pointer Events sets browser button bit 0 while the Pencil tip
        // is in contact. The native receiver must observe this as a normal tip,
        // never as PEN_FLAG_BARREL.
        flags: if matches!(action, Action::Down | Action::Move) {
            1
        } else {
            0
        },
        pointer_id: 7,
        sample_sequence: sequence,
        x_norm: x,
        y_norm: y,
        pressure,
        tilt_x_deg: tilt_x,
        tilt_y_deg: tilt_y,
        twist_deg: (sequence % 360) as f32,
        client_time_ms: 0.0,
    }
}

fn build_report(state: &SinkState, options: TestOptions) -> Value {
    let mut pressure_error = 0_u64;
    let mut tilt_error = 0_u64;
    let mut twist_error = 0_u64;
    let mut value_mismatches = Vec::new();
    let mut pressure_min = u32::MAX;
    let mut pressure_max = 0_u32;
    let mut tilt_x_min = i32::MAX;
    let mut tilt_x_max = i32::MIN;
    let mut tilt_y_min = i32::MAX;
    let mut tilt_y_max = i32::MIN;
    for (index, info) in state.received.iter().enumerate() {
        let sequence = index as u32 + 1;
        if sequence <= options.expected_samples {
            let expected = make_sample(sequence, options);
            let sample_pressure_error = info.pressure.abs_diff(expected.pressure_u32());
            let (expected_x, expected_y) = expected.tilt_i32();
            let sample_tilt_error = info.tiltX.abs_diff(expected_x) + info.tiltY.abs_diff(expected_y);
            let sample_twist_error = info.rotation.abs_diff(expected.twist_deg.round() as u32);
            pressure_error += u64::from(sample_pressure_error);
            tilt_error += u64::from(sample_tilt_error);
            twist_error += u64::from(sample_twist_error);
            if sample_pressure_error != 0 || sample_tilt_error != 0 || sample_twist_error != 0 {
                value_mismatches.push(sequence);
            }
        }
        pressure_min = pressure_min.min(info.pressure);
        pressure_max = pressure_max.max(info.pressure);
        tilt_x_min = tilt_x_min.min(info.tiltX);
        tilt_x_max = tilt_x_max.max(info.tiltX);
        tilt_y_min = tilt_y_min.min(info.tiltY);
        tilt_y_max = tilt_y_max.max(info.tiltY);
    }
    let elapsed = state
        .first_received
        .zip(state.last_received)
        .map_or(0.0, |(first, last)| last.duration_since(first).as_secs_f64());
    let received_count = state.received.len() as u32;
    let missing_count = options.expected_samples.saturating_sub(received_count);
    let excess_samples = received_count.saturating_sub(options.expected_samples);
    let exact_values = pressure_error == 0 && tilt_error == 0 && twist_error == 0;
    let ranges_ok = if options.quick {
        pressure_min <= 150 && pressure_max >= 950 && tilt_x_max >= 30 && tilt_y_min <= -30
    } else {
        pressure_min <= 55
            && pressure_max >= 1020
            && tilt_x_min <= -59
            && tilt_x_max >= 59
            && tilt_y_min <= -59
            && tilt_y_max >= 59
    };
    let pass = received_count == options.expected_samples
        && exact_values
        && ranges_ok
        && state.down_samples == 1
        && state.up_samples == 1
        && state.barrel_samples == 0
        && state.complete.load(Ordering::Acquire);
    serde_json::json!({
        "pass": pass,
        "mode": if options.quick { "self-test" } else { "stress-test" },
        "expected_samples": options.expected_samples,
        "received_samples": received_count,
        "windows_messages": state.messages,
        "coalesced_samples_recovered": received_count.saturating_sub(state.messages),
        "missing_count": missing_count,
        "excess_samples": excess_samples,
        "ordered_values": exact_values,
        "value_mismatch_count": value_mismatches.len(),
        "value_mismatch_preview": value_mismatches.into_iter().take(20).collect::<Vec<_>>(),
        "down_samples": state.down_samples,
        "up_samples": state.up_samples,
        "barrel_samples": state.barrel_samples,
        "pressure_range": [if pressure_min == u32::MAX { 0 } else { pressure_min }, pressure_max],
        "tilt_x_range": [if tilt_x_min == i32::MAX { 0 } else { tilt_x_min }, if tilt_x_max == i32::MIN { 0 } else { tilt_x_max }],
        "tilt_y_range": [if tilt_y_min == i32::MAX { 0 } else { tilt_y_min }, if tilt_y_max == i32::MIN { 0 } else { tilt_y_max }],
        "pressure_absolute_error": pressure_error,
        "tilt_absolute_error": tilt_error,
        "twist_absolute_error": twist_error,
        "elapsed_seconds": elapsed,
        "received_samples_per_second": if elapsed > 0.0 { f64::from(received_count.saturating_sub(1)) / elapsed } else { 0.0 },
        "target_samples_per_second": options.rate,
        "batch_size": options.batch_size,
        "pen_released": state.complete.load(Ordering::Acquire),
    })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
