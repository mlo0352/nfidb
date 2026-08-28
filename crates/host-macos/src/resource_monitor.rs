use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nfidb_core::Metrics;

const MACH_TASK_BASIC_INFO: i32 = 20;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeValue {
    seconds: i32,
    microseconds: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MachTaskBasicInfo {
    virtual_size: u64,
    resident_size: u64,
    resident_size_max: u64,
    user_time: TimeValue,
    system_time: TimeValue,
    policy: i32,
    suspend_count: i32,
}

unsafe extern "C" {
    fn mach_task_self() -> u32;
    fn task_info(target_task: u32, flavor: i32, task_info_out: *mut i32, task_info_count: *mut u32) -> i32;
}

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
            let cpu_percent =
                (current.cpu_seconds - before.cpu_seconds).max(0.0) / elapsed / logical_processors * 100.0;
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
    cpu_seconds: f64,
    working_set_bytes: u64,
    peak_working_set_bytes: u64,
}

fn process_sample() -> Option<ProcessSample> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let mut basic = MachTaskBasicInfo::default();
    let mut count = (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<i32>()) as u32;
    let status = unsafe {
        task_info(
            mach_task_self(),
            MACH_TASK_BASIC_INFO,
            std::ptr::from_mut(&mut basic).cast::<i32>(),
            &mut count,
        )
    };
    let working_set_bytes = if status == 0 { basic.resident_size } else { 0 };
    let peak_working_set_bytes = if status == 0 {
        basic.resident_size_max
    } else {
        usage.ru_maxrss.max(0) as u64
    };
    let cpu_seconds = usage.ru_utime.tv_sec as f64
        + usage.ru_utime.tv_usec as f64 / 1_000_000.0
        + usage.ru_stime.tv_sec as f64
        + usage.ru_stime.tv_usec as f64 / 1_000_000.0;
    Some(ProcessSample {
        cpu_seconds,
        working_set_bytes,
        peak_working_set_bytes,
    })
}
