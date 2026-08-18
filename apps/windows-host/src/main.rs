#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::Duration;
use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use eframe::egui;
use nfidb_core::{AppConfig, CaptureMode, InputSink, LoggingInputSink, Metrics, SessionManager, VideoProfile};
use nfidb_host_windows::{
    CaptureManager, MonitorDescriptor, PointerInjector, PointerInjectorOptions, enumerate_monitors,
    set_per_monitor_dpi_awareness,
};
use nfidb_transport::{ServerHandle, ServerOptions};
use qrcode::QrCode;
use qrcode::types::Color;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "NFiDB", version, about = "No Frills iPad Drawing Bridge")]
struct Cli {
    #[arg(long, conflicts_with = "display_only")]
    input_only: bool,
    #[arg(long, conflicts_with = "input_only")]
    display_only: bool,
    #[arg(long, value_enum, default_value_t = CaptureChoice::Monitor)]
    capture: CaptureChoice,
    #[arg(long, value_enum, default_value_t = InputSinkChoice::Inject)]
    input_sink: InputSinkChoice,
    #[arg(long, default_value = "info")]
    log_level: String,
    #[arg(long)]
    diagnostics: bool,
    #[arg(long, help = "Run without the desktop shell (diagnostic/automation mode)")]
    headless: bool,
    #[arg(long, requires = "headless", help = "Exit headless mode after this many seconds")]
    run_seconds: Option<u64>,
    #[arg(long, value_enum, help = "Override the saved video quality profile")]
    video_profile: Option<VideoProfileChoice>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=120), help = "Override the capture frame-rate limit")]
    max_fps: Option<u32>,
    #[arg(long, default_value_t = 1280, help = "Generated test-pattern source width")]
    test_width: u32,
    #[arg(long, default_value_t = 720, help = "Generated test-pattern source height")]
    test_height: u32,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1024..=65500), help = "Override the saved HTTP port")]
    port: Option<u16>,
    #[arg(long, help = "Disable local mDNS advertisement for this run")]
    no_mdns: bool,
    #[arg(long, requires = "headless", help = "Write startup URL/PIN JSON for automation")]
    session_info: Option<PathBuf>,
    #[arg(long, requires_all = ["headless", "run_seconds"], help = "Write final host metrics JSON for benchmarks")]
    metrics_output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum CaptureChoice {
    Monitor,
    TestPattern,
    None,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum InputSinkChoice {
    Inject,
    Log,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum VideoProfileChoice {
    Fast,
    Balanced,
    Sharp,
}

impl From<VideoProfileChoice> for VideoProfile {
    fn from(value: VideoProfileChoice) -> Self {
        match value {
            VideoProfileChoice::Fast => Self::Fast,
            VideoProfileChoice::Balanced => Self::Balanced,
            VideoProfileChoice::Sharp => Self::Sharp,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let filter = EnvFilter::try_new(&cli.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
    set_per_monitor_dpi_awareness().context("failed to configure Windows DPI awareness")?;

    let mut config = AppConfig::load().unwrap_or_else(|error| {
        tracing::warn!(%error, "using default settings because config could not be loaded");
        AppConfig::default()
    });
    if cli.input_only {
        config.mode = CaptureMode::InputOnly;
    } else if cli.display_only {
        config.mode = CaptureMode::DisplayOnly;
    }
    if let Some(profile) = cli.video_profile {
        config.video.profile = profile.into();
    }
    if let Some(max_fps) = cli.max_fps {
        config.video.max_fps = max_fps;
    }
    if let Some(port) = cli.port {
        config.network.port = port;
    }
    if cli.no_mdns {
        config.network.mdns = false;
    }
    let monitors = enumerate_monitors()
        .map_err(anyhow::Error::msg)
        .context("Windows did not report any captureable monitors")?;
    let selected = monitors
        .iter()
        .find(|monitor| monitor.index == config.monitor_index)
        .or_else(|| monitors.first())
        .context("Windows did not report any captureable monitors")?
        .clone();
    config.monitor_index = selected.index;

    let metrics = Arc::new(Metrics::default());
    let session = Arc::new(SessionManager::new());
    let native_injector = if cli.input_sink == InputSinkChoice::Inject && config.mode != CaptureMode::DisplayOnly {
        Some(Arc::new(PointerInjector::new(
            selected.geometry,
            PointerInjectorOptions {
                pen_enabled: config.input.pen,
                touch_enabled: config.input.touch,
                strict_palm_rejection: config.input.strict_palm_rejection,
            },
        )?))
    } else {
        None
    };
    let input: Arc<dyn InputSink> = native_injector.as_ref().map_or_else(
        || Arc::new(LoggingInputSink) as Arc<dyn InputSink>,
        |injector| Arc::clone(injector) as Arc<dyn InputSink>,
    );

    let (video_tx, _) = broadcast::channel(3);
    let capture = Arc::new(CaptureManager::new(
        video_tx.clone(),
        Arc::clone(&metrics),
        config.video.profile,
        config.video.max_fps,
        config.video.cursor,
    ));
    if config.mode != CaptureMode::InputOnly {
        let capture_result = match cli.capture {
            CaptureChoice::Monitor => capture.start_monitor(selected.index),
            CaptureChoice::TestPattern => capture.start_test_pattern_size(cli.test_width, cli.test_height),
            CaptureChoice::None => Ok(()),
        };
        if let Err(error) = capture_result {
            tracing::error!(%error, "capture did not start; input transport remains available");
        }
    }

    let host_name = sanitized_host_name();
    let server = Arc::new(
        ServerHandle::spawn(
            ServerOptions {
                preferred_port: config.network.port,
                host_name,
                mode: mode_name(config.mode).to_owned(),
                mdns: config.network.mdns,
            },
            Arc::clone(&session),
            Arc::clone(&metrics),
            input,
            video_tx,
        )
        .map_err(anyhow::Error::msg)?,
    );

    if cli.headless {
        if let Some(path) = &cli.session_info {
            write_json(
                path,
                &serde_json::json!({
                    "product": "NFiDB",
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                    "url": server.info.fallback_url,
                    "friendly_url": server.info.friendly_url,
                    "pin": server.info.pin,
                    "port": server.info.port,
                    "capture": capture.status().source,
                    "profile": format!("{:?}", config.video.profile).to_ascii_lowercase(),
                    "max_fps": config.video.max_fps,
                }),
            )?;
        }
        tracing::info!(url = %server.info.fallback_url, "NFiDB headless host ready");
        if let Some(seconds) = cli.run_seconds {
            std::thread::sleep(Duration::from_secs(seconds));
        } else {
            loop {
                std::thread::park();
            }
        }
        capture.stop();
        if let Some(path) = &cli.metrics_output {
            write_json(path, &metrics.snapshot())?;
        }
        server.stop();
        return Ok(());
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NFiDB — No Frills iPad Drawing Bridge")
            .with_inner_size([980.0, 690.0])
            .with_min_inner_size([760.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "NFiDB",
        native_options,
        Box::new(move |context| {
            configure_visuals(&context.egui_ctx);
            Ok(Box::new(HostApp {
                config,
                monitors,
                selected_index: selected.index,
                metrics,
                server,
                capture,
                injector: native_injector,
                show_diagnostics: cli.diagnostics,
                last_message: None,
            }))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

fn write_json(path: &PathBuf, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

struct HostApp {
    config: AppConfig,
    monitors: Vec<MonitorDescriptor>,
    selected_index: usize,
    metrics: Arc<Metrics>,
    server: Arc<ServerHandle>,
    capture: Arc<CaptureManager>,
    injector: Option<Arc<PointerInjector>>,
    show_diagnostics: bool,
    last_message: Option<String>,
}

impl eframe::App for HostApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        context.request_repaint_after(Duration::from_millis(500));
        egui::Panel::top("top_bar")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(9, 12, 13))
                    .inner_margin(egui::Margin::symmetric(26, 18)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("NFi").size(23.0).strong().color(accent()));
                    ui.add_space(-7.0);
                    ui.label(egui::RichText::new("DB").size(23.0).strong());
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("NO FRILLS IPAD DRAWING BRIDGE")
                            .size(10.0)
                            .color(muted()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("LOCAL NETWORK ONLY")
                                .size(10.0)
                                .strong()
                                .color(accent()),
                        );
                    });
                });
            });

        egui::Panel::left("navigation")
            .exact_size(210.0)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(12, 16, 17))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ui, |ui| {
                ui.add_space(10.0);
                nav_label(ui, "SESSION", true);
                nav_label(ui, "SOURCE", false);
                nav_label(ui, "INPUT", false);
                nav_label(ui, "APP SETUP", false);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        egui::RichText::new("MIT OR APACHE-2.0\nNo account · No telemetry")
                            .size(10.0)
                            .color(muted()),
                    );
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(16, 20, 21))
                    .inner_margin(egui::Margin::same(28)),
            )
            .show(ui, |ui| {
                ui.heading(egui::RichText::new("Ready for iPad").size(27.0).strong());
                ui.label(
                    egui::RichText::new("Open the local address in Safari, then enter the PIN.")
                        .size(13.0)
                        .color(muted()),
                );
                ui.add_space(18.0);
                self.session_card(ui);
                ui.add_space(16.0);
                self.status_strip(ui);
                ui.add_space(16.0);
                self.settings_card(ui);
                if self.show_diagnostics {
                    ui.add_space(16.0);
                    self.diagnostics_card(ui);
                }
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(injector) = &self.injector {
            let _ = injector.reset_all();
        }
        self.capture.stop();
        self.server.stop();
        let _ = self.config.save();
    }
}

impl HostApp {
    fn session_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(11, 15, 16))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 54, 55)))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("SAFARI ADDRESS").size(9.0).strong().color(muted()));
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&self.server.info.fallback_url)
                                    .size(18.0)
                                    .monospace()
                                    .color(accent()),
                            );
                            if ui.small_button("COPY").clicked() {
                                ui.ctx().copy_text(self.server.info.fallback_url.clone());
                                self.last_message = Some("Address copied".to_owned());
                            }
                        });
                        ui.label(
                            egui::RichText::new(&self.server.info.friendly_url)
                                .size(11.0)
                                .monospace()
                                .color(muted()),
                        );
                        ui.add_space(20.0);
                        ui.label(egui::RichText::new("PAIRING PIN").size(9.0).strong().color(muted()));
                        ui.label(
                            egui::RichText::new(format_pin(&self.server.info.pin))
                                .size(31.0)
                                .monospace()
                                .strong(),
                        );
                        ui.add_space(8.0);
                        if ui.button("Open pointer diagnostic").clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                                "{}diagnostics/pointer",
                                self.server.info.fallback_url
                            )));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        draw_qr(ui, &self.server.info.qr_url, 154.0);
                    });
                });
                if let Some(message) = &self.last_message {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(message).size(10.0).color(accent()));
                }
            });
    }

    fn status_strip(&mut self, ui: &mut egui::Ui) {
        let metrics = self.metrics.snapshot();
        let capture = self.capture.status();
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(13, 18, 19))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(37, 47, 48)))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    status_metric(
                        ui,
                        if metrics.connected { "● IPAD" } else { "○ WAITING" },
                        if metrics.connected { "Connected" } else { "Ready" },
                    );
                    ui.separator();
                    status_metric(
                        ui,
                        "VIDEO",
                        &format!("{:.0}/{:.0} fps", metrics.capture_fps, metrics.encoded_fps),
                    );
                    ui.separator();
                    status_metric(ui, "PENCIL", &format!("{:.0} samples/s", metrics.input_samples_per_sec));
                    ui.separator();
                    status_metric(ui, "PRESSURE", &format!("{:.2}", metrics.pressure));
                    ui.separator();
                    status_metric(ui, "ENCODER", if capture.running { "Active" } else { "Stopped" });
                });
            });
    }

    fn settings_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(19, 24, 25))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 54, 55)))
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("CAPTURE SOURCE").size(9.0).strong().color(muted()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let before = self.selected_index;
                        egui::ComboBox::from_id_salt("monitor")
                            .selected_text(self.monitor_name(self.selected_index))
                            .show_ui(ui, |ui| {
                                for monitor in &self.monitors {
                                    ui.selectable_value(
                                        &mut self.selected_index,
                                        monitor.index,
                                        format!(
                                            "{} · {}×{} @ {} Hz",
                                            monitor.name, monitor.width, monitor.height, monitor.refresh_rate
                                        ),
                                    );
                                }
                            });
                        if before != self.selected_index {
                            self.change_monitor();
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("INPUT").size(9.0).strong().color(muted()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut changed = false;
                        changed |= ui
                            .checkbox(&mut self.config.input.strict_palm_rejection, "Strict palm mode")
                            .changed();
                        changed |= ui.checkbox(&mut self.config.input.touch, "Forward touch").changed();
                        changed |= ui.checkbox(&mut self.config.input.pen, "Forward Pencil").changed();
                        if changed {
                            self.apply_input_options();
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("QUALITY").size(9.0).strong().color(muted()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (profile, name) in [
                            (VideoProfile::Sharp, "Sharp"),
                            (VideoProfile::Balanced, "Balanced"),
                            (VideoProfile::Fast, "Fast"),
                        ] {
                            ui.selectable_value(&mut self.config.video.profile, profile, name);
                        }
                    });
                });
            });
    }

    fn diagnostics_card(&self, ui: &mut egui::Ui) {
        let metrics = self.metrics.snapshot();
        let capture = self.capture.status();
        egui::CollapsingHeader::new("Advanced diagnostics")
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new("diagnostics_grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        diagnostic_row(ui, "Capture source", &capture.source);
                        diagnostic_row(ui, "Encoder", &capture.encoder);
                        diagnostic_row(
                            ui,
                            "Frame size",
                            &format!("{}×{}", metrics.source_width, metrics.source_height),
                        );
                        diagnostic_row(ui, "Encode time", &format!("{:.2} ms", metrics.encode_ms));
                        diagnostic_row(ui, "Dropped before encode", &metrics.dropped_frames.to_string());
                        diagnostic_row(
                            ui,
                            "Last tilt",
                            &format!("{:.0}° / {:.0}°", metrics.tilt_x, metrics.tilt_y),
                        );
                    });
                if let Some(error) = capture.error {
                    ui.colored_label(egui::Color32::from_rgb(255, 137, 125), error);
                }
                if ui.button("Copy sanitized diagnostics").clicked() {
                    let report = serde_json::json!({
                        "product": concat!("NFiDB ", env!("CARGO_PKG_VERSION")),
                        "host": "redacted",
                        "capture": capture.source,
                        "encoder": capture.encoder,
                        "metrics": metrics,
                    });
                    ui.ctx().copy_text(
                        serde_json::to_string_pretty(&report)
                            .unwrap_or_else(|_| "NFiDB diagnostics unavailable".to_owned()),
                    );
                }
            });
    }

    fn change_monitor(&mut self) {
        let Some(monitor) = self
            .monitors
            .iter()
            .find(|monitor| monitor.index == self.selected_index)
        else {
            return;
        };
        self.config.monitor_index = monitor.index;
        if let Some(injector) = &self.injector {
            let _ = injector.reset_all();
            injector.set_target(monitor.geometry);
        }
        if self.config.mode != CaptureMode::InputOnly {
            self.last_message = match self.capture.start_monitor(monitor.index) {
                Ok(()) => Some(format!("Now capturing {}", monitor.name)),
                Err(error) => Some(format!("Capture failed: {error}")),
            };
        }
        let _ = self.config.save();
    }

    fn apply_input_options(&mut self) {
        if let Some(injector) = &self.injector {
            let _ = injector.reset_all();
            injector.set_options(PointerInjectorOptions {
                pen_enabled: self.config.input.pen,
                touch_enabled: self.config.input.touch,
                strict_palm_rejection: self.config.input.strict_palm_rejection,
            });
        }
        let _ = self.config.save();
    }

    fn monitor_name(&self, index: usize) -> String {
        self.monitors
            .iter()
            .find(|monitor| monitor.index == index)
            .map_or_else(|| format!("Display {index}"), |monitor| monitor.name.clone())
    }
}

fn configure_visuals(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(16, 20, 21);
    visuals.window_fill = egui::Color32::from_rgb(17, 22, 23);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(24, 31, 32);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(49, 61, 62));
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(29, 45, 42);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent());
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(31, 72, 63);
    visuals.selection.bg_fill = egui::Color32::from_rgb(30, 95, 80);
    visuals.selection.stroke = egui::Stroke::new(1.0, accent());
    visuals.window_corner_radius = egui::CornerRadius::same(2);
    context.set_visuals(visuals);
}

fn draw_qr(ui: &mut egui::Ui, value: &str, size: f32) {
    let Ok(code) = QrCode::new(value.as_bytes()) else {
        ui.label("QR unavailable");
        return;
    };
    let modules = code.width();
    let quiet = 3_usize;
    let total = modules + quiet * 2;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
    let cell = size / total as f32;
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] == Color::Dark {
                let min = rect.min + egui::vec2((x + quiet) as f32 * cell, (y + quiet) as f32 * cell);
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(min, egui::vec2(cell + 0.25, cell + 0.25)),
                    0.0,
                    egui::Color32::BLACK,
                );
            }
        }
    }
}

fn nav_label(ui: &mut egui::Ui, label: &str, active: bool) {
    let color = if active { accent() } else { muted() };
    ui.add_sized(
        [170.0, 38.0],
        egui::Label::new(egui::RichText::new(label).size(10.0).strong().color(color)).sense(egui::Sense::hover()),
    );
}

fn status_metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).size(8.0).strong().color(accent()));
        ui.label(
            egui::RichText::new(value)
                .size(11.0)
                .color(egui::Color32::from_rgb(220, 227, 225)),
        );
    });
}

fn diagnostic_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).size(10.0).color(muted()));
    ui.label(egui::RichText::new(value).size(10.0).monospace());
    ui.end_row();
}

fn format_pin(pin: &str) -> String {
    if pin.len() == 6 {
        format!("{} {}", &pin[..3], &pin[3..])
    } else {
        pin.to_owned()
    }
}

fn sanitized_host_name() -> String {
    let raw = hostname::get().unwrap_or_default().to_string_lossy().into_owned();
    let name: String = raw
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(50)
        .collect();
    if name.is_empty() { "nfidb-pc".to_owned() } else { name }
}

const fn mode_name(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::PenDisplay => "pen-display",
        CaptureMode::InputOnly => "input-only",
        CaptureMode::DisplayOnly => "display-only",
    }
}

const fn accent() -> egui::Color32 {
    egui::Color32::from_rgb(91, 224, 194)
}

const fn muted() -> egui::Color32 {
    egui::Color32::from_rgb(133, 148, 146)
}
