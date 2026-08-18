use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    #[default]
    PenDisplay,
    InputOnly,
    DisplayOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoProfile {
    Fast,
    #[default]
    Balanced,
    Sharp,
}

impl VideoProfile {
    #[must_use]
    pub const fn max_width(self) -> u32 {
        match self {
            Self::Fast => 1280,
            Self::Balanced => 1920,
            Self::Sharp => 2560,
        }
    }

    #[must_use]
    pub const fn bitrate_bps(self) -> u32 {
        match self {
            Self::Fast => 5_000_000,
            Self::Balanced => 10_000_000,
            Self::Sharp => 18_000_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub port: u16,
    pub mdns: bool,
    pub require_pin: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: 47_831,
            mdns: true,
            require_pin: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    pub profile: VideoProfile,
    pub max_fps: u32,
    pub cursor: bool,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            profile: VideoProfile::Balanced,
            max_fps: 60,
            cursor: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    pub pen: bool,
    pub touch: bool,
    pub mouse: bool,
    pub keyboard: bool,
    pub gestures: bool,
    pub strict_palm_rejection: bool,
    pub focus_target_on_pen_down: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            pen: true,
            touch: false,
            mouse: true,
            keyboard: true,
            gestures: true,
            strict_palm_rejection: true,
            focus_target_on_pen_down: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiConfig {
    pub show_advanced_stats: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub mode: CaptureMode,
    pub monitor_index: usize,
    pub network: NetworkConfig,
    pub video: VideoConfig,
    pub input: InputConfig,
    pub ui: UiConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: CaptureMode::PenDisplay,
            monitor_index: 1,
            network: NetworkConfig::default(),
            video: VideoConfig::default(),
            input: InputConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> io::Result<Self> {
        let path = Self::path()?;
        match fs::read_to_string(path) {
            Ok(contents) => {
                toml::from_str(&contents).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, contents)
    }

    pub fn path() -> io::Result<PathBuf> {
        let base = dirs::config_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "config directory unavailable"))?;
        Ok(base.join("NFiDB").join("config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips() {
        let config = AppConfig::default();
        let text = toml::to_string(&config).expect("serialize config");
        let decoded: AppConfig = toml::from_str(&text).expect("deserialize config");
        assert_eq!(decoded.network.port, 47_831);
        assert_eq!(decoded.video.profile, VideoProfile::Balanced);
        assert!(!decoded.input.touch);
    }

    #[test]
    fn older_input_config_gets_safe_remote_input_defaults() {
        let decoded: AppConfig = toml::from_str(
            r#"
                [input]
                pen = true
                touch = false
            "#,
        )
        .expect("deserialize prior config");
        assert!(decoded.input.mouse);
        assert!(decoded.input.keyboard);
        assert!(decoded.input.gestures);
    }
}
