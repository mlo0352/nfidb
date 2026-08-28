use std::process::Command;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionStatus {
    pub screen_recording: bool,
    pub accessibility: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyPane {
    ScreenRecording,
    Accessibility,
}

#[must_use]
pub fn permission_status() -> PermissionStatus {
    PermissionStatus {
        screen_recording: unsafe { CGPreflightScreenCaptureAccess() },
        accessibility: unsafe { CGPreflightPostEventAccess() },
    }
}

#[must_use]
pub fn request_screen_recording_access() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

#[must_use]
pub fn request_accessibility_access() -> bool {
    unsafe { CGRequestPostEventAccess() }
}

pub fn open_privacy_pane(pane: PrivacyPane) -> Result<(), String> {
    let suffix = match pane {
        PrivacyPane::ScreenRecording => "Privacy_ScreenCapture",
        PrivacyPane::Accessibility => "Privacy_Accessibility",
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{suffix}");
    let status = Command::new("open")
        .arg(url)
        .status()
        .map_err(|error| format!("could not open System Settings: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("System Settings exited with status {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_probe_is_safe_without_grants() {
        let _ = permission_status();
    }
}
