use core_graphics::display::CGDisplay;
use nfidb_protocol::TargetGeometry;
use screencapturekit::shareable_content::SCShareableContent;

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
    let primary = CGDisplay::main().id;
    let content = SCShareableContent::get();
    let mut monitors = Vec::new();
    if let Ok(content) = content {
        for (index, display) in content.displays().into_iter().enumerate() {
            monitors.push(descriptor(
                index,
                display.display_id(),
                display.width(),
                display.height(),
                primary,
            ));
        }
    } else {
        // Screen Recording permission is intentionally not a prerequisite for
        // opening the host UI or running input-only/test-pattern diagnostics.
        // CoreGraphics still provides non-capture display geometry so the user
        // can grant permission from a functioning NFiDB window.
        let displays = CGDisplay::active_displays()
            .map_err(|error| format!("CoreGraphics display enumeration failed with code {error}"))?;
        for (index, id) in displays.into_iter().enumerate() {
            let display = CGDisplay::new(id);
            monitors.push(descriptor(
                index,
                id,
                display.pixels_wide().min(u64::from(u32::MAX)) as u32,
                display.pixels_high().min(u64::from(u32::MAX)) as u32,
                primary,
            ));
        }
    }
    (!monitors.is_empty())
        .then_some(monitors)
        .ok_or_else(|| "CoreGraphics did not report an active display".to_owned())
}

fn descriptor(index: usize, id: u32, width: u32, height: u32, primary: u32) -> MonitorDescriptor {
    let display = CGDisplay::new(id);
    let bounds = display.bounds();
    let refresh_rate = display
        .display_mode()
        .map_or(60, |mode| mode.refresh_rate().round().clamp(1.0, 240.0) as u32);
    MonitorDescriptor {
        index,
        name: if id == primary {
            "Main display".to_owned()
        } else {
            format!("Display {}", index + 1)
        },
        device_name: format!("CGDisplay {id}"),
        width,
        height,
        refresh_rate,
        geometry: TargetGeometry {
            left: bounds.origin.x.round() as i32,
            top: bounds.origin.y.round() as i32,
            width: bounds.size.width.round().max(1.0) as u32,
            height: bounds.size.height.round().max(1.0) as u32,
        },
        primary: id == primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_descriptors_have_valid_geometry_when_permission_is_available() {
        if let Ok(displays) = enumerate_monitors() {
            assert!(!displays.is_empty());
            assert!(displays.iter().all(|display| display.width > 0 && display.height > 0));
            assert!(displays.iter().any(|display| display.primary));
        }
    }
}
