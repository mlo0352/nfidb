use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nfidb_core::Metrics;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

pub struct ProcessResourceMonitor {
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ProcessResourceMonitor {
    #[must_use]
    pub fn start(metrics: Arc<Metrics>) -> Self {
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let thread = thread::Builder::new()
            .name("nfidb-resource-monitor".to_owned())
            .spawn(move || monitor_loop(metrics, worker_stopped))
            .ok();
        Self { stopped, thread }
    }
}

impl Drop for ProcessResourceMonitor {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn monitor_loop(metrics: Arc<Metrics>, stopped: Arc<AtomicBool>) {
    let logical_processors = std::thread::available_parallelism().map_or(1, std::num::NonZero::get) as f64;
    let mut previous = process_sample();
    let mut previous_at = Instant::now();
    while !stopped.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(500));
        let now = Instant::now();
        if let (Some(before), Some(current)) = (previous, process_sample()) {
            let elapsed = now.duration_since(previous_at).as_secs_f64().max(0.001);
            let process_seconds = current.process_100ns.saturating_sub(before.process_100ns) as f64 / 10_000_000.0;
            let cpu_percent = process_seconds / elapsed / logical_processors * 100.0;
            metrics.process_resources(cpu_percent, current.working_set_bytes, current.peak_working_set_bytes);
            previous = Some(current);
        } else {
            previous = process_sample();
        }
        previous_at = now;
    }
}

#[derive(Clone, Copy)]
struct ProcessSample {
    process_100ns: u64,
    working_set_bytes: u64,
    peak_working_set_bytes: u64,
}

fn process_sample() -> Option<ProcessSample> {
    unsafe {
        let process = GetCurrentProcess();
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user).ok()?;
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        GetProcessMemoryInfo(process, &mut memory, memory.cb).ok()?;
        Some(ProcessSample {
            process_100ns: filetime_u64(kernel).saturating_add(filetime_u64(user)),
            working_set_bytes: memory.WorkingSetSize as u64,
            peak_working_set_bytes: memory.PeakWorkingSetSize as u64,
        })
    }
}

const fn filetime_u64(value: FILETIME) -> u64 {
    (value.dwHighDateTime as u64) << 32 | value.dwLowDateTime as u64
}
