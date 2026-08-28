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
    let content = SCShareableContent::get().map_err(|error| {
        format!(
            "ScreenCaptureKit could not enumerate displays: {error}. Allow NFiDB in System Settings > Privacy & Security > Screen & System Audio Recording."
        )
    })?;
    let mut monitors = Vec::new();
    for (index, display) in content.displays().into_iter().enumerate() {
        let id = display.display_id();
        let cg_display = CGDisplay::new(id);
        let bounds = cg_display.bounds();
        let refresh_rate = cg_display
            .display_mode()
            .map_or(60, |mode| mode.refresh_rate().round().clamp(1.0, 240.0) as u32);
        monitors.push(MonitorDescriptor {
            index,
            name: if id == primary {
                "Main display".to_owned()
            } else {
                format!("Display {}", index + 1)
            },
            device_name: format!("CGDisplay {id}"),
            width: display.width(),
            height: display.height(),
            refresh_rate,
            geometry: TargetGeometry {
                left: bounds.origin.x.round() as i32,
                top: bounds.origin.y.round() as i32,
                width: bounds.size.width.round().max(1.0) as u32,
                height: bounds.size.height.round().max(1.0) as u32,
            },
            primary: id == primary,
        });
    }
    Ok(monitors)
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
