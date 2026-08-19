use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

static CONFIG_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Balanced => "Balanced",
            Self::Sharp => "Sharp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Av1 => "AV1",
        }
    }

    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::H264 => "video/H264",
            Self::Hevc => "video/H265",
            Self::Av1 => "video/AV1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EncoderMode {
    #[default]
    Auto,
    H264Hardware,
    HevcHardware,
    Av1Hardware,
    H264Software,
}

impl EncoderMode {
    #[must_use]
    pub const fn codec(self) -> Option<VideoCodec> {
        match self {
            Self::Auto => None,
            Self::H264Hardware | Self::H264Software => Some(VideoCodec::H264),
            Self::HevcHardware => Some(VideoCodec::Hevc),
            Self::Av1Hardware => Some(VideoCodec::Av1),
        }
    }

    #[must_use]
    pub const fn requires_hardware(self) -> bool {
        matches!(self, Self::H264Hardware | Self::HevcHardware | Self::Av1Hardware)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto — Recommended",
            Self::H264Hardware => "H.264 Hardware",
            Self::HevcHardware => "HEVC Hardware",
            Self::Av1Hardware => "AV1 Hardware",
            Self::H264Software => "H.264 Software",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodecBitrates {
    pub h264_mbps: f32,
    /// `None` means use the measured Auto recommendation, or the H.264 target
    /// until this machine has a trustworthy measurement.
    pub hevc_mbps: Option<f32>,
    /// `None` means use the measured Auto recommendation, or the H.264 target
    /// until this machine has a trustworthy measurement.
    pub av1_mbps: Option<f32>,
}

impl CodecBitrates {
    #[must_use]
    pub fn for_codec(&self, codec: VideoCodec) -> f32 {
        match codec {
            VideoCodec::H264 => self.h264_mbps,
            VideoCodec::Hevc => self.hevc_mbps.unwrap_or(self.h264_mbps),
            VideoCodec::Av1 => self.av1_mbps.unwrap_or(self.h264_mbps),
        }
    }

    pub fn set_for_codec(&mut self, codec: VideoCodec, mbps: f32) {
        match codec {
            VideoCodec::H264 => self.h264_mbps = mbps,
            VideoCodec::Hevc => self.hevc_mbps = Some(mbps),
            VideoCodec::Av1 => self.av1_mbps = Some(mbps),
        }
    }
}

impl Default for CodecBitrates {
    fn default() -> Self {
        Self {
            h264_mbps: 10.0,
            hevc_mbps: None,
            av1_mbps: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoPreset {
    pub max_width: u32,
    pub max_fps: u32,
    pub bitrates: CodecBitrates,
}

impl VideoPreset {
    #[must_use]
    pub fn bitrate_bps(&self, codec: VideoCodec) -> u32 {
        (self.bitrates.for_codec(codec) * 1_000_000.0).round() as u32
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(320..=7680).contains(&self.max_width) {
            return Err("maximum width must be between 320 and 7680 pixels".to_owned());
        }
        if !(1..=120).contains(&self.max_fps) {
            return Err("frame rate must be between 1 and 120 fps".to_owned());
        }
        for value in [
            Some(self.bitrates.h264_mbps),
            self.bitrates.hevc_mbps,
            self.bitrates.av1_mbps,
        ]
        .into_iter()
        .flatten()
        {
            if !value.is_finite() || !(0.5..=200.0).contains(&value) {
                return Err("bitrate must be between 0.5 and 200 Mbps".to_owned());
            }
        }
        Ok(())
    }
}

impl Default for VideoPreset {
    fn default() -> Self {
        Self {
            max_width: 1920,
            max_fps: 60,
            bitrates: CodecBitrates::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoPresets {
    pub fast: VideoPreset,
    pub balanced: VideoPreset,
    pub sharp: VideoPreset,
}

impl VideoPresets {
    #[must_use]
    pub const fn get(&self, profile: VideoProfile) -> &VideoPreset {
        match profile {
            VideoProfile::Fast => &self.fast,
            VideoProfile::Balanced => &self.balanced,
            VideoProfile::Sharp => &self.sharp,
        }
    }

    #[must_use]
    pub const fn get_mut(&mut self, profile: VideoProfile) -> &mut VideoPreset {
        match profile {
            VideoProfile::Fast => &mut self.fast,
            VideoProfile::Balanced => &mut self.balanced,
            VideoProfile::Sharp => &mut self.sharp,
        }
    }
}

impl Default for VideoPresets {
    fn default() -> Self {
        Self {
            fast: VideoPreset {
                max_width: 1280,
                max_fps: 60,
                bitrates: CodecBitrates {
                    h264_mbps: 5.0,
                    ..CodecBitrates::default()
                },
            },
            balanced: VideoPreset::default(),
            sharp: VideoPreset {
                max_width: 2560,
                max_fps: 60,
                bitrates: CodecBitrates {
                    h264_mbps: 18.0,
                    ..CodecBitrates::default()
                },
            },
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    pub profile: VideoProfile,
    pub encoder: EncoderMode,
    pub cursor: bool,
    pub presets: VideoPresets,
    /// Compatibility input for configurations written before presets became
    /// editable. New configurations omit this field.
    #[serde(rename = "max_fps", skip_serializing_if = "Option::is_none")]
    pub legacy_max_fps: Option<u32>,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            profile: VideoProfile::Balanced,
            encoder: EncoderMode::Auto,
            cursor: true,
            presets: VideoPresets::default(),
            legacy_max_fps: None,
        }
    }
}

impl VideoConfig {
    #[must_use]
    pub fn active_preset(&self) -> VideoPreset {
        let mut preset = self.presets.get(self.profile).clone();
        if let Some(legacy_max_fps) = self.legacy_max_fps {
            preset.max_fps = legacy_max_fps.clamp(1, 120);
        }
        preset
    }

    pub fn migrate_legacy(&mut self) {
        if let Some(max_fps) = self.legacy_max_fps.take() {
            self.presets.get_mut(self.profile).max_fps = max_fps.clamp(1, 120);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.presets.fast.validate()?;
        self.presets.balanced.validate()?;
        self.presets.sharp.validate()
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
pub struct FileTransferConfig {
    pub enabled: bool,
    pub max_file_size_mib: u64,
    pub rate_limit_mbps: u32,
    pub pause_while_drawing: bool,
    pub inbox_directory: Option<PathBuf>,
}

impl Default for FileTransferConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_size_mib: 10 * 1024,
            rate_limit_mbps: 32,
            pause_while_drawing: true,
            inbox_directory: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub mode: CaptureMode,
    pub monitor_index: usize,
    pub network: NetworkConfig,
    pub video: VideoConfig,
    pub input: InputConfig,
    pub file_transfer: FileTransferConfig,
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
            file_transfer: FileTransferConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> io::Result<Self> {
        let path = Self::path()?;
        match fs::read_to_string(path) {
            Ok(contents) => {
                let mut config: Self =
                    toml::from_str(&contents).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                config.video.migrate_legacy();
                config
                    .video
                    .validate()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                Ok(config)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let _guard = CONFIG_WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.save_unlocked()
    }

    fn save_unlocked(&self) -> io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, contents)
    }

    pub fn save_video_settings(video: &VideoConfig) -> io::Result<()> {
        video.validate().map_err(io::Error::other)?;
        let _guard = CONFIG_WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut config = Self::load()?;
        config.video = video.clone();
        config.save_unlocked()
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
        assert_eq!(decoded.video.encoder, EncoderMode::Auto);
        assert_eq!(decoded.video.active_preset().max_width, 1920);
        assert!(!decoded.input.touch);
        assert!(decoded.file_transfer.enabled);
        assert_eq!(decoded.file_transfer.max_file_size_mib, 10 * 1024);
    }

    #[test]
    fn old_video_config_migrates_without_deleting_user_configuration() {
        let mut decoded: AppConfig = toml::from_str(
            r#"
                [video]
                profile = "fast"
                max_fps = 45
                cursor = false
            "#,
        )
        .expect("deserialize prior video config");
        decoded.video.migrate_legacy();
        assert_eq!(decoded.video.encoder, EncoderMode::Auto);
        assert_eq!(decoded.video.active_preset().max_width, 1280);
        assert_eq!(decoded.video.active_preset().max_fps, 45);
        assert!(!decoded.video.cursor);
        assert!(decoded.video.legacy_max_fps.is_none());
    }

    #[test]
    fn invalid_remote_scale_values_are_rejected() {
        let mut video = VideoConfig::default();
        video.presets.balanced.max_fps = 1_000_000;
        assert!(video.validate().is_err());
        video.presets.balanced.max_fps = 60;
        video.presets.balanced.bitrates.h264_mbps = f32::INFINITY;
        assert!(video.validate().is_err());
    }

    #[test]
    fn codec_specific_bitrate_round_trips() {
        let mut config = AppConfig::default();
        config.video.presets.fast.bitrates.set_for_codec(VideoCodec::Hevc, 3.75);
        let text = toml::to_string(&config).expect("serialize config");
        let decoded: AppConfig = toml::from_str(&text).expect("deserialize config");
        assert_eq!(decoded.video.presets.fast.bitrates.for_codec(VideoCodec::Hevc), 3.75);
        assert_eq!(decoded.video.presets.fast.bitrates.for_codec(VideoCodec::Av1), 5.0);
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
        assert!(decoded.file_transfer.enabled);
    }
}
