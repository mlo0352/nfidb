use nfidb_protocol::TargetGeometry;
use windows_capture::monitor::Monitor;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};

#[derive(Debug, Clone)]
pub struct MonitorDescriptor {
    pub index: usize,
    pub name: String,
    pub device_name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub geometry: TargetGeometry,
    pub primary: bool,
}

pub fn enumerate_monitors() -> Result<Vec<MonitorDescriptor>, String> {
    let primary = Monitor::primary().map_err(|error| error.to_string())?;
    Monitor::enumerate()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|monitor| {
            let index = monitor.index().map_err(|error| error.to_string())?;
            let geometry = monitor_geometry(monitor)?;
            Ok(MonitorDescriptor {
                index,
                name: monitor.name().unwrap_or_else(|_| format!("Display {index}")),
                device_name: monitor.device_name().unwrap_or_default(),
                width: monitor.width().map_err(|error| error.to_string())?,
                height: monitor.height().map_err(|error| error.to_string())?,
                refresh_rate: monitor.refresh_rate().unwrap_or(60),
                geometry,
                primary: monitor == primary,
            })
        })
        .collect()
}

fn monitor_geometry(monitor: Monitor) -> Result<TargetGeometry, String> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    let ok = unsafe { GetMonitorInfoW(monitor.as_raw_hmonitor(), &mut info) };
    if ok == 0 {
        return Err(format!("GetMonitorInfoW failed: {}", std::io::Error::last_os_error()));
    }
    Ok(TargetGeometry {
        left: info.rcMonitor.left,
        top: info.rcMonitor.top,
        width: info.rcMonitor.right.saturating_sub(info.rcMonitor.left) as u32,
        height: info.rcMonitor.bottom.saturating_sub(info.rcMonitor.top) as u32,
    })
}
