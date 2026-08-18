#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use eframe::egui;
use nfidb_core::{AppConfig, CaptureMode, InputSink, LoggingInputSink, Metrics, SessionManager, VideoProfile};
use nfidb_host_windows::{
    CaptureManager, MonitorDescriptor, PointerInjector, PointerInjectorOptions, enumerate_monitors,
    set_per_monitor_dpi_awareness,
};
use nfidb_transport::{Distribution, ServerHandle, ServerOptions};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPage {
    Session,
    Source,
    Input,
    Diagnostics,
    AppSetup,
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
            capture.keyframe_request(),
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
                active_page: if cli.diagnostics {
                    HostPage::Diagnostics
                } else {
                    HostPage::Session
                },
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
    active_page: HostPage,
    last_message: Option<String>,
}

impl eframe::App for HostApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        context.request_repaint_after(Duration::from_millis(500));
        if context.input(|input| input.focused) && self.server.rotate_pairing_if_expired() {
            self.last_message = Some("Expired PIN and QR code rotated automatically".to_owned());
        }
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
                for (page, label) in [
                    (HostPage::Session, "SESSION"),
                    (HostPage::Source, "SOURCE"),
                    (HostPage::Input, "INPUT"),
                    (HostPage::Diagnostics, "DIAGNOSTICS"),
                    (HostPage::AppSetup, "APP SETUP"),
                ] {
                    if nav_button(ui, label, self.active_page == page).clicked() {
                        self.active_page = page;
                    }
                }
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
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.active_page {
                        HostPage::Session => self.session_page(ui),
                        HostPage::Source => self.source_page(ui),
                        HostPage::Input => self.input_page(ui),
                        HostPage::Diagnostics => self.diagnostics_page(ui),
                        HostPage::AppSetup => self.app_setup_page(ui),
                    });
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
    fn session_page(&mut self, ui: &mut egui::Ui) {
        page_heading(
            ui,
            "Ready for iPad",
            "Open the local address in Safari, then enter the current PIN or scan the QR code.",
        );
        ui.add_space(18.0);
        self.session_card(ui);
        ui.add_space(16.0);
        self.status_strip(ui);
    }

    fn source_page(&mut self, ui: &mut egui::Ui) {
        page_heading(
            ui,
            "Source and video",
            "Choose the mirrored monitor and inspect the capture/encoder path.",
        );
        ui.add_space(18.0);
        self.status_strip(ui);
        ui.add_space(16.0);
        self.source_settings_card(ui);
    }

    fn input_page(&mut self, ui: &mut egui::Ui) {
        page_heading(
            ui,
            "Pencil and touch input",
            "Control forwarding and verify pressure, tilt, continuity, and injection timing.",
        );
        ui.add_space(18.0);
        self.status_strip(ui);
        ui.add_space(16.0);
        self.input_settings_card(ui);
        ui.add_space(16.0);
        self.input_diagnostics_card(ui);
    }

    fn diagnostics_page(&mut self, ui: &mut egui::Ui) {
        page_heading(
            ui,
            "Live diagnostic recorder",
            "The iPad sends one detailed WebRTC/browser sample per second; NFiDB retains and processes up to six hours locally.",
        );
        ui.add_space(18.0);
        self.diagnostic_actions(ui);
        ui.add_space(16.0);
        self.diagnostics_card(ui);
        ui.add_space(16.0);
        self.client_diagnostics_card(ui);
        ui.add_space(16.0);
        self.processed_diagnostics_card(ui);
    }

    fn app_setup_page(&mut self, ui: &mut egui::Ui) {
        page_heading(
            ui,
            "Application setup",
            "Validate Safari signals first, then Windows Ink, then the drawing application.",
        );
        ui.add_space(18.0);
        card(ui, |ui| {
            ui.heading(egui::RichText::new("Recommended validation order").size(17.0));
            ui.label(
                "1. Open the pointer diagnostic on the iPad and test pressure, tilt, coalescing, and long strokes.",
            );
            ui.label("2. Run pointer-sink.exe --self-test to validate native Windows pen injection.");
            ui.label("3. Enable Windows Ink/Pointer input in Krita, Rebelle, Photoshop, or the target drawing app.");
            ui.label("4. Return to Diagnostics, reset the recording, perform the test matrix, then export JSON.");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Open pointer diagnostic").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                        "{}diagnostics/pointer",
                        self.server.info.fallback_url
                    )));
                }
                if ui.button("Copy pointer-sink command").clicked() {
                    ui.ctx().copy_text("pointer-sink.exe --self-test".to_owned());
                    self.last_message = Some("pointer-sink command copied".to_owned());
                }
            });
        });
        if let Some(message) = &self.last_message {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(message).color(accent()));
        }
    }

    fn session_card(&mut self, ui: &mut egui::Ui) {
        let pairing_pin = self.server.pairing_pin();
        let pairing_qr_url = self.server.pairing_qr_url();
        let expires = self.server.pairing_expires_in_seconds();
        let pairing_active = self.server.pairing_is_active();
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
                            egui::RichText::new(format_pin(&pairing_pin))
                                .size(31.0)
                                .monospace()
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(if pairing_active {
                                "CONNECTED · reset to pair a different device".to_owned()
                            } else {
                                format!(
                                    "ROTATES IN {:02}:{:02} WHILE THIS WINDOW IS ACTIVE",
                                    expires / 60,
                                    expires % 60
                                )
                            })
                            .size(9.0)
                            .color(muted()),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Reset PIN + QR").clicked() {
                                self.server.rotate_pairing();
                                self.last_message =
                                    Some("PIN and QR rotated; any connected iPad was disconnected".to_owned());
                            }
                            if ui.button("Open pointer diagnostic").clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                                    "{}diagnostics/pointer",
                                    self.server.info.fallback_url
                                )));
                            }
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        draw_qr(ui, &pairing_qr_url, 154.0);
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

    fn source_settings_card(&mut self, ui: &mut egui::Ui) {
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
                    ui.label(egui::RichText::new("QUALITY").size(9.0).strong().color(muted()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (profile, name) in [
                            (VideoProfile::Sharp, "Sharp"),
                            (VideoProfile::Balanced, "Balanced"),
                            (VideoProfile::Fast, "Fast"),
                        ] {
                            if ui
                                .selectable_value(&mut self.config.video.profile, profile, name)
                                .changed()
                            {
                                let _ = self.config.save();
                                self.last_message =
                                    Some("Video profile saved; restart NFiDB to rebuild the encoder".to_owned());
                            }
                        }
                    });
                });
                if let Some(message) = &self.last_message {
                    ui.separator();
                    ui.label(egui::RichText::new(message).size(10.0).color(accent()));
                }
            });
    }

    fn input_settings_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(egui::RichText::new("FORWARDING").size(9.0).strong().color(muted()));
            ui.add_space(8.0);
            let mut changed = false;
            changed |= ui
                .checkbox(&mut self.config.input.pen, "Forward Apple Pencil as Windows pen")
                .changed();
            changed |= ui
                .checkbox(&mut self.config.input.touch, "Forward touch contacts")
                .changed();
            changed |= ui
                .checkbox(&mut self.config.input.strict_palm_rejection, "Strict palm rejection")
                .changed();
            if changed {
                self.apply_input_options();
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Keep touch disabled for the first Pencil pass. Enable it only when the drawing app needs canvas gestures.")
                    .size(10.0)
                    .color(muted()),
            );
        });
    }

    fn input_diagnostics_card(&self, ui: &mut egui::Ui) {
        let metrics = self.metrics.snapshot();
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("LIVE INPUT EVIDENCE")
                    .size(9.0)
                    .strong()
                    .color(muted()),
            );
            ui.add_space(8.0);
            egui::Grid::new("input_diagnostics_grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    diagnostic_row(
                        ui,
                        "Samples",
                        &format!(
                            "{} total · {:.1}/s",
                            metrics.input_samples, metrics.input_samples_per_sec
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Estimated arrival",
                        &format!(
                            "{:.2} ms now · {:.2} avg · {:.2} max",
                            metrics.input_arrival_ms, metrics.average_input_arrival_ms, metrics.max_input_arrival_ms
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Native injection",
                        &format!(
                            "{:.3} ms now · {:.3} avg · {:.3} max",
                            metrics.input_inject_ms, metrics.average_input_inject_ms, metrics.max_input_inject_ms
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Continuity",
                        &format!(
                            "{} sample gaps · {} out of order · {} lifecycle errors",
                            metrics.sample_sequence_gaps, metrics.out_of_order_samples, metrics.lifecycle_errors
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Pressure range",
                        &format!("{:.3}–{:.3}", metrics.pressure_min, metrics.pressure_max),
                    );
                    diagnostic_row(
                        ui,
                        "Tilt X range",
                        &format!("{:.1}°–{:.1}°", metrics.tilt_x_min, metrics.tilt_x_max),
                    );
                    diagnostic_row(
                        ui,
                        "Tilt Y range",
                        &format!("{:.1}°–{:.1}°", metrics.tilt_y_min, metrics.tilt_y_max),
                    );
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
                            &format!(
                                "{}×{} source → {}×{} encoded",
                                metrics.source_width,
                                metrics.source_height,
                                metrics.output_width,
                                metrics.output_height
                            ),
                        );
                        diagnostic_row(
                            ui,
                            "Capture / encode rate",
                            &format!("{:.1} / {:.1} fps", metrics.capture_fps, metrics.encoded_fps),
                        );
                        diagnostic_row(
                            ui,
                            "Preprocess",
                            &format!(
                                "{:.2} ms now · {:.2} avg · {:.2} max",
                                metrics.preprocess_ms, metrics.average_preprocess_ms, metrics.max_preprocess_ms
                            ),
                        );
                        diagnostic_row(
                            ui,
                            "Encode",
                            &format!(
                                "{:.2} ms now · {:.2} avg · {:.2} max",
                                metrics.encode_ms, metrics.average_encode_ms, metrics.max_encode_ms
                            ),
                        );
                        diagnostic_row(ui, "Encoded data", &format_bytes(metrics.encoded_bytes));
                        diagnostic_row(
                            ui,
                            "Dropped before encode",
                            &format!("{} newest-frame replacements", metrics.dropped_frames),
                        );
                        diagnostic_row(ui, "WebRTC transport skips", &metrics.video_transport_drops.to_string());
                        diagnostic_row(ui, "Encoded keyframes", &metrics.encoded_keyframes.to_string());
                        diagnostic_row(
                            ui,
                            "Video startup",
                            &format!(
                                "{:.0} ms · {} pre-IDR frames skipped",
                                metrics.video_startup_wait_ms, metrics.video_startup_delta_frames
                            ),
                        );
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

    fn diagnostic_actions(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Reset recording").clicked() {
                    self.server.clear_diagnostics();
                    self.last_message =
                        Some("Diagnostic history reset; the next iPad sample starts a fresh run".to_owned());
                }
                if ui.button("Export detailed JSON").clicked() {
                    self.last_message = Some(match self.export_diagnostics() {
                        Ok(path) => format!("Diagnostic report exported to {}", path.display()),
                        Err(error) => format!("Diagnostic export failed: {error}"),
                    });
                }
                if ui.button("Copy processed summary").clicked() {
                    let summary = self.server.diagnostic_summary();
                    ui.ctx().copy_text(
                        serde_json::to_string_pretty(&summary)
                            .unwrap_or_else(|_| "NFiDB diagnostic summary unavailable".to_owned()),
                    );
                    self.last_message = Some("Processed diagnostic summary copied".to_owned());
                }
            });
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Recording is local, starts automatically when Safari connects, and includes raw RTC counters plus synchronized host metrics. Exact glass-to-glass Pencil latency still requires a high-speed camera.",
                )
                .size(10.0)
                .color(muted()),
            );
            if let Some(message) = &self.last_message {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(message).size(10.0).color(accent()));
            }
        });
    }

    fn client_diagnostics_card(&self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("LATEST IPAD / SAFARI SAMPLE")
                    .size(9.0)
                    .strong()
                    .color(muted()),
            );
            ui.add_space(8.0);
            let Some(latest) = self.server.latest_diagnostic() else {
                ui.label("Waiting for the connected iPad to send its first one-second diagnostic sample…");
                return;
            };
            let client = latest.client;
            egui::Grid::new("client_diagnostics_grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    diagnostic_row(
                        ui,
                        "Sample",
                        &format!("#{} · {:.0} ms interval", client.sequence, client.sample_interval_ms),
                    );
                    diagnostic_row(
                        ui,
                        "Device",
                        &format!(
                            "client {} · {} · DPR {} · {}×{} viewport",
                            json_string(&client.device, "clientVersion"),
                            json_string(&client.device, "platform"),
                            json_number(&client.device, "devicePixelRatio"),
                            json_number(&client.device, "viewportWidth"),
                            json_number(&client.device, "viewportHeight")
                        ),
                    );
                    diagnostic_row(ui, "Browser", &json_string(&client.device, "userAgent"));
                    if client
                        .device
                        .get("diagnosticFallback")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                    {
                        diagnostic_row(ui, "Recorder fallback", &json_string(&client.device, "diagnosticError"));
                    }
                    diagnostic_row(
                        ui,
                        "State",
                        &format!(
                            "{} · ICE {} · input {}",
                            json_string(&client.connection, "peerConnectionState"),
                            json_string(&client.connection, "iceConnectionState"),
                            json_string(&client.connection, "inputTransport")
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Video",
                        &format!(
                            "{}×{} · {:.1} decode / {:.1} present fps",
                            client.video.width, client.video.height, client.video.decode_fps, client.video.playback_fps
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "First frame",
                        &client
                            .video
                            .startup_ms
                            .map_or_else(|| "pending/unavailable".to_owned(), |value| format!("{value:.1} ms")),
                    );
                    diagnostic_row(
                        ui,
                        "Bandwidth",
                        &format!(
                            "{:.3} Mbps receive · {:.3} Mbps available",
                            client.network.receive_mbps, client.network.available_incoming_mbps
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "LAN timing",
                        &format!(
                            "{:.2} ms RTT · {:.2} ms jitter · {:.2} ms one-way estimate",
                            client.network.rtt_ms, client.network.jitter_ms, client.network.one_way_estimate_ms
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "RTP integrity",
                        &format!(
                            "{} packets · {} lost · {} lost this sample",
                            client.network.packets_received,
                            client.network.packets_lost,
                            client.network.packet_loss_delta
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Jitter buffer",
                        &format!("{:.3} ms/frame", client.video.jitter_buffer_ms_per_frame),
                    );
                    diagnostic_row(
                        ui,
                        "Decoder",
                        &format!(
                            "{:.3} ms/frame · {} decoder drops · {} freezes / {:.3} s",
                            client.video.decode_ms_per_frame,
                            client.video.decoder_dropped_frames,
                            client.video.freeze_count,
                            client.video.total_freeze_seconds
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Presentation",
                        &format!(
                            "{:.2}% browser drops · {:.1} ms p95 / {:.1} ms max frame gap",
                            client.video.presentation_drop_percent,
                            client.frame_timing.frame_gap_p95_ms,
                            client.frame_timing.frame_gap_max_ms
                        ),
                    );
                    diagnostic_row(
                        ui,
                        "Capture → present",
                        &client.frame_timing.capture_to_present_p95_ms.map_or_else(
                            || {
                                format!(
                                    "metadata unavailable · {:.1} ms component estimate",
                                    client.frame_timing.estimated_pipeline_ms
                                )
                            },
                            |value| format!("{value:.1} ms p95 from Safari frame metadata"),
                        ),
                    );
                });
        });
    }

    fn processed_diagnostics_card(&self, ui: &mut egui::Ui) {
        let summary = self.server.diagnostic_summary();
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("PROCESSED RUN SUMMARY")
                    .size(9.0)
                    .strong()
                    .color(muted()),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} retained samples over {:.1} seconds · {} discarded after the six-hour bound",
                    summary.sample_count, summary.retained_seconds, summary.discarded_samples
                ))
                .size(10.0)
                .color(muted()),
            );
            ui.add_space(8.0);
            egui::Grid::new("processed_diagnostics_grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    distribution_row(ui, "RTT", &summary.rtt_ms, "ms");
                    distribution_row(ui, "Receive bandwidth", &summary.receive_mbps, "Mbps");
                    distribution_row(ui, "Decode rate", &summary.decode_fps, "fps");
                    distribution_row(ui, "Presentation rate", &summary.playback_fps, "fps");
                    distribution_row(ui, "Jitter buffer", &summary.jitter_buffer_ms_per_frame, "ms/frame");
                    distribution_row(ui, "Decode time", &summary.decode_ms_per_frame, "ms/frame");
                    distribution_row(ui, "Frame-gap p95", &summary.frame_gap_p95_ms, "ms");
                    distribution_row(ui, "Capture → present p95", &summary.capture_to_present_p95_ms, "ms");
                    distribution_row(ui, "Pipeline estimate", &summary.estimated_pipeline_ms, "ms");
                    distribution_row(ui, "Host encode", &summary.host_encode_ms, "ms");
                    distribution_row(ui, "Input arrival estimate", &summary.input_arrival_ms, "ms");
                    distribution_row(ui, "Native input injection", &summary.input_inject_ms, "ms");
                    diagnostic_row(
                        ui,
                        "Integrity totals",
                        &format!(
                            "{} packet loss · {} input gaps · {} input errors · {} transport skips",
                            summary.packet_loss_total,
                            summary.latest_input_sample_gaps,
                            summary.latest_input_errors,
                            summary.latest_video_transport_drops
                        ),
                    );
                });
        });
    }

    fn export_diagnostics(&self) -> Result<PathBuf> {
        let config_path = AppConfig::path().context("diagnostic directory unavailable")?;
        let directory = config_path
            .parent()
            .context("diagnostic directory has no parent")?
            .join("diagnostics");
        fs::create_dir_all(&directory).with_context(|| format!("failed to create {}", directory.display()))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = directory.join(format!("nfidb-diagnostics-{stamp}.json"));
        let capture = self.capture.status();
        let report = serde_json::json!({
            "product": concat!("NFiDB ", env!("CARGO_PKG_VERSION")),
            "capture": capture.source,
            "encoder": capture.encoder,
            "configuration": &self.config,
            "current_host_metrics": self.metrics.snapshot(),
            "diagnostics": self.server.diagnostic_report(),
        });
        write_json(&path, &report)?;
        Ok(path)
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

fn nav_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let color = if active { accent() } else { muted() };
    ui.add_sized(
        [170.0, 38.0],
        egui::Button::new(egui::RichText::new(label).size(10.0).strong().color(color))
            .fill(if active {
                egui::Color32::from_rgb(20, 40, 36)
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::new(
                1.0,
                if active {
                    egui::Color32::from_rgb(48, 103, 90)
                } else {
                    egui::Color32::TRANSPARENT
                },
            )),
    )
}

fn page_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.heading(egui::RichText::new(title).size(27.0).strong());
    ui.label(egui::RichText::new(subtitle).size(13.0).color(muted()));
}

fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(egui::Color32::from_rgb(19, 24, 25))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 54, 55)))
        .inner_margin(egui::Margin::same(18))
        .show(ui, contents);
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

fn distribution_row(ui: &mut egui::Ui, label: &str, distribution: &Distribution, unit: &str) {
    diagnostic_row(
        ui,
        label,
        &format!(
            "n={} · mean {:.2} · p50 {:.2} · p95 {:.2} · p99 {:.2} · max {:.2} {unit}",
            distribution.count,
            distribution.mean,
            distribution.p50,
            distribution.p95,
            distribution.p99,
            distribution.max,
        ),
    );
}

fn json_string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn json_number(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .map_or_else(|| "?".to_owned(), |number| format!("{number:.0}"))
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
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
