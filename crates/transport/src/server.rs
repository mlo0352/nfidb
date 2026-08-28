use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use nfidb_core::{
    AppConfig, BrowserVideoCapabilities, EncodedVideoFrame, InputSink, KeyframeRequest, Metrics, SessionManager,
    SetVideoSettingsRequest, VideoConfig, VideoSettingsRuntime, VideoSettingsSnapshot, compatibility_matrix,
};
use parking_lot::Mutex;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use crate::diagnostics::{
    ClientDiagnosticSample, DiagnosticRecorder, DiagnosticReport, DiagnosticSummary, RecordedDiagnosticSample,
};
use crate::file_transfer::{
    FileTransferError, FileTransferErrorKind, FileTransferManager, FileTransferOptions, UPLOAD_CHUNK_SIZE,
    content_disposition,
};
use crate::process_input_packet;
use crate::webrtc_session::{ActivePeer, WebRtcOffer, accept_offer};

#[derive(RustEmbed)]
#[folder = "../../apps/ipad-web/dist/"]
struct WebAssets;

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub preferred_port: u16,
    pub host_name: String,
    pub host_platform: String,
    pub mode: String,
    pub mdns: bool,
    pub touch_default: bool,
    pub mouse_enabled: bool,
    pub keyboard_enabled: bool,
    pub gestures_default: bool,
    pub file_transfer: FileTransferOptions,
    pub video: VideoConfig,
}

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub port: u16,
    pub host_name: String,
    pub local_ip: Ipv4Addr,
    pub friendly_url: String,
    pub fallback_url: String,
    pub qr_url: String,
    pub pin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInputSettings {
    pub touch_enabled: bool,
    pub gestures_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct InputControlState {
    pub revision: u64,
    pub settings: RemoteInputSettings,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct SetInputSettingsRequest {
    base_revision: u64,
    settings: RemoteInputSettings,
}

pub struct ServerHandle {
    pub info: ServerInfo,
    state: Arc<AppState>,
    command_tx: mpsc::UnboundedSender<ServerCommand>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ServerHandle {
    pub fn spawn(
        options: ServerOptions,
        session: Arc<SessionManager>,
        metrics: Arc<Metrics>,
        input: Arc<dyn InputSink>,
        video_tx: broadcast::Sender<EncodedVideoFrame>,
        keyframe_request: KeyframeRequest,
        video_runtime: Arc<dyn VideoSettingsRuntime>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let file_transfers = FileTransferManager::new(
            options.file_transfer.clone(),
            Arc::clone(&session),
            Arc::clone(&metrics),
        )?;
        let initial_video = options.video.clone();
        let initial_input = RemoteInputSettings {
            touch_enabled: options.touch_default,
            gestures_enabled: options.gestures_default,
        };
        let state = Arc::new(AppState {
            options,
            session,
            metrics,
            input,
            video_tx,
            keyframe_request,
            peer: ActivePeer::default(),
            diagnostics: DiagnosticRecorder::default(),
            file_transfers,
            video_settings: Mutex::new(VideoSettingsSnapshot {
                revision: 1,
                settings: initial_video,
            }),
            input_settings: Mutex::new(InputControlState {
                revision: 1,
                settings: initial_input,
            }),
            video_update: Mutex::new(()),
            browser_video: Mutex::new(BrowserVideoCapabilities::default()),
            video_runtime,
        });
        let server_state = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("nfidb-network".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name("nfidb-io")
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let result = run_server(server_state, command_rx, shutdown_rx, ready_tx).await;
                    if let Err(error) = result {
                        tracing::error!(%error, "LAN server stopped with an error");
                    }
                });
            })
            .map_err(|error| error.to_string())?;
        let info = ready_rx
            .recv_timeout(Duration::from_secs(15))
            .map_err(|error| format!("LAN server did not start: {error}"))??;
        Ok(Self {
            info,
            state,
            command_tx,
            shutdown: Mutex::new(Some(shutdown_tx)),
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn stop(&self) {
        if let Some(shutdown) = self.shutdown.lock().take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
    }

    #[must_use]
    pub fn pairing_pin(&self) -> String {
        self.state.session.pin()
    }

    #[must_use]
    pub fn pairing_qr_url(&self) -> String {
        format!("{}?qr={}", self.info.fallback_url, self.state.session.qr_secret())
    }

    #[must_use]
    pub fn pairing_expires_in_seconds(&self) -> u64 {
        self.state.session.expires_in_seconds()
    }

    #[must_use]
    pub fn pairing_is_active(&self) -> bool {
        self.state.session.is_paired()
    }

    pub fn rotate_pairing(&self) {
        let session_id = self.state.session.session_id();
        self.state.file_transfers.cancel_session_uploads(&session_id);
        self.state.session.rotate();
        self.close_active_peer();
    }

    pub fn rotate_pairing_if_expired(&self) -> bool {
        let session_id = self.state.session.session_id();
        if self.state.session.rotate_if_expired() {
            self.state.file_transfers.cancel_session_uploads(&session_id);
            self.close_active_peer();
            true
        } else {
            false
        }
    }

    fn close_active_peer(&self) {
        self.state.metrics.set_connected(false);
        self.state.metrics.reset_input_continuity();
        let _ = self.state.input.reset_all();
        let _ = self.command_tx.send(ServerCommand::ClosePeer);
    }

    #[must_use]
    pub fn latest_diagnostic(&self) -> Option<RecordedDiagnosticSample> {
        self.state.diagnostics.latest()
    }

    #[must_use]
    pub fn diagnostic_summary(&self) -> DiagnosticSummary {
        self.state.diagnostics.summary()
    }

    #[must_use]
    pub fn diagnostic_report(&self) -> DiagnosticReport {
        self.state.diagnostics.report()
    }

    pub fn clear_diagnostics(&self) {
        self.state.diagnostics.clear();
    }

    #[must_use]
    pub fn file_transfer_snapshot(&self) -> crate::FileTransferSnapshot {
        self.state.file_transfers.snapshot()
    }

    pub fn queue_outgoing_file(&self, path: std::path::PathBuf) -> Result<crate::OutgoingFile, String> {
        self.state.file_transfers.queue_outgoing(path)
    }

    pub fn remove_outgoing_file(&self, id: uuid::Uuid) -> bool {
        self.state.file_transfers.remove_outgoing(id)
    }

    pub fn clear_outgoing_files(&self) {
        self.state.file_transfers.clear_outgoing();
    }

    pub fn configure_file_transfers(&self, enabled: bool, max_size_bytes: u64, rate_mbps: u32, pause: bool) {
        self.state
            .file_transfers
            .configure(enabled, max_size_bytes, rate_mbps, pause);
    }

    #[must_use]
    pub fn file_inbox_directory(&self) -> std::path::PathBuf {
        self.state.file_transfers.inbox_directory()
    }

    #[must_use]
    pub fn video_control_state(&self) -> VideoControlState {
        video_response(&self.state)
    }

    #[must_use]
    pub fn input_control_state(&self) -> InputControlState {
        *self.state.input_settings.lock()
    }

    /// Applies native-UI changes through the same authority used by the paired
    /// receiver, keeping both controls and the injector on one shared state.
    pub fn apply_input_settings_from_host(&self, settings: RemoteInputSettings) -> Result<InputControlState, String> {
        apply_input_settings_state(&self.state, settings, None)
    }

    /// Applies a settings edit made by the native Windows UI through the same
    /// validated, revisioned authority used by the browser control endpoint.
    pub fn apply_video_settings_from_host(&self, settings: VideoConfig) -> Result<VideoControlState, String> {
        let previous_codec = self.state.video_runtime.video_runtime_status().codec;
        let runtime = apply_video_settings_state(&self.state, settings, None)?;
        if runtime.codec != previous_codec {
            self.close_active_peer();
        }
        Ok(video_response(&self.state))
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

struct AppState {
    options: ServerOptions,
    session: Arc<SessionManager>,
    metrics: Arc<Metrics>,
    input: Arc<dyn InputSink>,
    video_tx: broadcast::Sender<EncodedVideoFrame>,
    keyframe_request: KeyframeRequest,
    peer: ActivePeer,
    diagnostics: DiagnosticRecorder,
    file_transfers: FileTransferManager,
    video_settings: Mutex<VideoSettingsSnapshot>,
    input_settings: Mutex<InputControlState>,
    video_update: Mutex<()>,
    browser_video: Mutex<BrowserVideoCapabilities>,
    video_runtime: Arc<dyn VideoSettingsRuntime>,
}

enum ServerCommand {
    ClosePeer,
}

async fn run_server(
    state: Arc<AppState>,
    mut commands: mpsc::UnboundedReceiver<ServerCommand>,
    shutdown: oneshot::Receiver<()>,
    ready: std::sync::mpsc::SyncSender<Result<ServerInfo, String>>,
) -> Result<(), String> {
    let (listener, port) = bind_available(state.options.preferred_port).await?;
    let local_ip = best_local_ipv4();
    let friendly_url = format!("http://{}.local:{port}/", state.options.host_name);
    let fallback_url = format!("http://{local_ip}:{port}/");
    let qr_url = format!("{fallback_url}?qr={}", state.session.qr_secret());
    let info = ServerInfo {
        port,
        host_name: state.options.host_name.clone(),
        local_ip,
        friendly_url,
        fallback_url,
        qr_url,
        pin: state.session.pin(),
    };

    let mdns = if state.options.mdns {
        match advertise_mdns(&state.options.host_name, local_ip, port) {
            Ok(service) => Some(service),
            Err(error) => {
                tracing::warn!(%error, "mDNS advertisement unavailable; numeric IP remains usable");
                None
            }
        }
    } else {
        None
    };

    let command_state = Arc::clone(&state);
    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/metrics", get(metrics_handler))
        .route("/api/diagnostics", get(diagnostics_handler))
        .route("/api/input", get(input_handler).put(set_input_settings))
        .route("/api/video", get(video_handler).put(set_video_settings))
        .route("/api/video/capabilities", post(set_browser_video_capabilities))
        .route("/api/video/presented", post(video_presented))
        .route("/api/video/benchmark-result", post(record_video_benchmark))
        .route("/api/video/benchmark-results", delete(clear_video_benchmarks))
        .route("/api/files", get(files_handler))
        .route("/api/files/uploads", post(create_upload))
        .route(
            "/api/files/uploads/{id}",
            get(upload_status).put(write_upload_chunk).delete(cancel_upload),
        )
        .route("/api/files/uploads/{id}/complete", post(complete_upload))
        .route("/api/files/outbox/{id}", delete(remove_outbox_file))
        .route("/api/files/outbox/{id}/download", get(download_outbox_file))
        .route("/api/pair", post(pair))
        .route("/api/disconnect", post(disconnect))
        .route("/api/webrtc/offer", post(webrtc_offer))
        .route("/api/ws", get(websocket))
        .fallback(static_asset)
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tokio::spawn(async move {
        while let Some(command) = commands.recv().await {
            match command {
                ServerCommand::ClosePeer => command_state.peer.close().await,
            }
        }
    });

    ready.send(Ok(info)).map_err(|error| error.to_string())?;
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
        })
        .await
        .map_err(|error| error.to_string());
    if let Some((daemon, fullname)) = mdns {
        let _ = daemon.unregister(&fullname);
        let _ = daemon.shutdown();
    }
    result
}

async fn bind_available(preferred: u16) -> Result<(tokio::net::TcpListener, u16), String> {
    for offset in 0..100_u16 {
        let Some(port) = preferred.checked_add(offset) else {
            break;
        };
        match tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(format!("failed to bind LAN server on port {port}: {error}")),
        }
    }
    Err(format!(
        "ports {preferred} through {} are already in use",
        preferred.saturating_add(99)
    ))
}

fn advertise_mdns(host_name: &str, local_ip: Ipv4Addr, port: u16) -> Result<(ServiceDaemon, String), String> {
    let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
    let instance = format!("NFiDB on {host_name}");
    let properties = [("path", "/"), ("product", "NFiDB")];
    let service = ServiceInfo::new(
        "_nfidb._tcp.local.",
        &instance,
        &format!("{host_name}.local."),
        IpAddr::V4(local_ip),
        port,
        &properties[..],
    )
    .map_err(|error| error.to_string())?
    .enable_addr_auto();
    let fullname = service.get_fullname().to_owned();
    daemon.register(service).map_err(|error| error.to_string())?;
    Ok((daemon, fullname))
}

fn best_local_ipv4() -> Ipv4Addr {
    let candidates = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(ip) if !ip.is_loopback() && (ip.is_private() || ip.is_link_local()) => Some((name, ip)),
            _ => None,
        })
        .collect();
    select_best_local_ipv4(candidates).unwrap_or(Ipv4Addr::LOCALHOST)
}

fn select_best_local_ipv4(mut candidates: Vec<(String, Ipv4Addr)>) -> Option<Ipv4Addr> {
    candidates.sort_by_key(|(name, ip)| {
        (
            ip.is_link_local(),
            !is_physical_lan_interface(name),
            is_overlay_or_virtual_interface(name),
        )
    });
    candidates.first().map(|(_, ip)| *ip)
}

fn is_physical_lan_interface(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if is_overlay_or_virtual_interface(&name) {
        return false;
    }
    let compact: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    let macos_en = compact
        .strip_prefix("en")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit()));
    macos_en
        || name.contains("wi-fi")
        || name.contains("wifi")
        || name.contains("wireless")
        || name.contains("ethernet")
        || ["eth", "eno", "enp", "ens", "wlan", "wlp", "wlx"]
            .iter()
            .any(|prefix| compact.starts_with(prefix))
}

fn is_overlay_or_virtual_interface(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "utun",
        "tailscale",
        "zerotier",
        "wireguard",
        "hamachi",
        "vethernet",
        "hyper-v",
        "vmnet",
        "vbox",
        "feth",
        "docker",
        "bridge",
        "loopback",
        "vpn",
        "tunnel",
    ]
    .iter()
    .any(|marker| name.contains(marker))
        || name.starts_with("tun")
        || name.starts_with("tap")
        || name.starts_with("wg")
}

#[derive(Serialize)]
struct StatusResponse {
    #[serde(flatten)]
    session: nfidb_core::PublicSession,
    protocol_version: u8,
    webrtc: bool,
    host_platform: String,
    touch_default: bool,
    mouse_enabled: bool,
    keyboard_enabled: bool,
    gestures_default: bool,
    file_transfer_enabled: bool,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let input = state.input_settings.lock().settings;
    Json(StatusResponse {
        session: state
            .session
            .public(state.options.host_name.clone(), state.options.mode.clone()),
        protocol_version: nfidb_protocol::PROTOCOL_VERSION,
        webrtc: true,
        host_platform: state.options.host_platform.clone(),
        touch_default: input.touch_enabled,
        mouse_enabled: state.options.mouse_enabled,
        keyboard_enabled: state.options.keyboard_enabled,
        gestures_default: input.gestures_enabled,
        file_transfer_enabled: state.file_transfers.snapshot().enabled,
    })
}

async fn input_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized_cookie(&headers, &state.session) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    Json(*state.input_settings.lock()).into_response()
}

async fn set_input_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SetInputSettingsRequest>,
) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    let current = state.input_settings.lock().revision;
    if !input_revision_matches(request.base_revision, current) {
        let mut response = Json(serde_json::json!({
            "error": "input settings changed on another device",
            "current": *state.input_settings.lock(),
        }))
        .into_response();
        *response.status_mut() = StatusCode::CONFLICT;
        return response;
    }
    match apply_input_settings_state(&state, request.settings, Some(request.base_revision)) {
        Ok(control) => Json(control).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error),
    }
}

fn apply_input_settings_state(
    state: &AppState,
    settings: RemoteInputSettings,
    expected_revision: Option<u64>,
) -> Result<InputControlState, String> {
    let mut current = state.input_settings.lock();
    if expected_revision.is_some_and(|revision| revision != current.revision) {
        return Err("input settings changed on another device".to_owned());
    }
    if current.settings == settings {
        return Ok(*current);
    }
    state
        .input
        .set_remote_input_options(settings.touch_enabled, settings.gestures_enabled)
        .map_err(|error| error.to_string())?;
    current.revision = current.revision.saturating_add(1);
    current.settings = settings;
    state.metrics.reset_input_continuity();
    Ok(*current)
}

#[derive(Deserialize)]
struct PairRequest {
    pin: Option<String>,
    qr_secret: Option<String>,
}

async fn pair(State(state): State<Arc<AppState>>, Json(request): Json<PairRequest>) -> Response {
    let result = match (request.pin, request.qr_secret) {
        (Some(pin), _) => state.session.pair_with_pin(&pin),
        (_, Some(secret)) => state.session.pair_with_qr_secret(&secret),
        _ => return api_error(StatusCode::BAD_REQUEST, "PIN or QR secret required"),
    };
    match result {
        Ok(result) => {
            state.metrics.reset_input_continuity();
            let cookie = format!(
                "nfidb_token={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=28800",
                result.access_token
            );
            let mut response = Json(result).into_response();
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
            response
        }
        Err(error) => api_error(StatusCode::UNAUTHORIZED, &error.to_string()),
    }
}

async fn metrics_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized_cookie(&headers, &state.session) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    Json(state.metrics.snapshot()).into_response()
}

async fn diagnostics_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized_cookie(&headers, &state.session) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    Json(state.diagnostics.summary()).into_response()
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoControlState {
    pub settings: VideoSettingsSnapshot,
    pub host_capabilities: Vec<nfidb_core::EncoderCapability>,
    pub browser_capabilities: BrowserVideoCapabilities,
    pub compatibility: Vec<nfidb_core::CompatibilityEntry>,
    pub runtime: nfidb_core::VideoRuntimeStatus,
    pub learned_results: Vec<nfidb_core::AutoBenchmarkObservation>,
}

fn video_response(state: &AppState) -> VideoControlState {
    let settings = state.video_settings.lock().clone();
    let browser_capabilities = state.browser_video.lock().clone();
    let host_capabilities = state.video_runtime.encoder_capabilities();
    let compatibility = compatibility_matrix(&host_capabilities, &browser_capabilities);
    VideoControlState {
        settings,
        host_capabilities,
        browser_capabilities,
        compatibility,
        runtime: state.video_runtime.video_runtime_status(),
        learned_results: state.video_runtime.auto_benchmark_results(),
    }
}

async fn video_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized_cookie(&headers, &state.session) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    Json(video_response(&state)).into_response()
}

async fn set_browser_video_capabilities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(capabilities): Json<BrowserVideoCapabilities>,
) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    if capabilities.user_agent.len() > 1024
        || capabilities
            .h264
            .mime_types
            .iter()
            .chain(capabilities.hevc.mime_types.iter())
            .chain(capabilities.av1.mime_types.iter())
            .any(|value| value.len() > 256)
    {
        return api_error(StatusCode::BAD_REQUEST, "browser capability report is too large");
    }
    let previous_codec = state.video_runtime.video_runtime_status().codec;
    let settings = state.video_settings.lock().settings.clone();
    match state.video_runtime.apply_video_settings(&settings, &capabilities) {
        Ok(runtime) => {
            *state.browser_video.lock() = capabilities;
            if runtime.codec != previous_codec {
                state.peer.close().await;
                let _ = state.input.reset_all();
                state.metrics.reset_input_continuity();
            }
            Json(video_response(&state)).into_response()
        }
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error),
    }
}

async fn set_video_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SetVideoSettingsRequest>,
) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    let current_revision = state.video_settings.lock().revision;
    if !video_revision_matches(request.base_revision, current_revision) {
        let mut response = Json(serde_json::json!({
            "error": "video settings changed on another device",
            "current": video_response(&state),
        }))
        .into_response();
        *response.status_mut() = StatusCode::CONFLICT;
        return response;
    }
    let previous_codec = state.video_runtime.video_runtime_status().codec;
    let runtime = match apply_video_settings_state(&state, request.settings, Some(request.base_revision)) {
        Ok(runtime) => runtime,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    if runtime.codec != previous_codec {
        state.peer.close().await;
        let _ = state.input.reset_all();
        state.metrics.reset_input_continuity();
    }
    Json(video_response(&state)).into_response()
}

fn apply_video_settings_state(
    state: &AppState,
    settings: VideoConfig,
    expected_revision: Option<u64>,
) -> Result<nfidb_core::VideoRuntimeStatus, String> {
    settings.validate()?;
    let _update = state.video_update.lock();
    let previous = state.video_settings.lock().clone();
    if expected_revision.is_some_and(|revision| !video_revision_matches(revision, previous.revision)) {
        return Err("video settings changed on another device; refresh and try again".to_owned());
    }
    let browser = state.browser_video.lock().clone();
    let runtime = state.video_runtime.apply_video_settings(&settings, &browser)?;
    if let Err(error) = AppConfig::save_video_settings(&settings) {
        // Do not leave a live-but-unpersisted setting behind. The rollback is
        // best-effort; its failure is included because it changes recovery.
        let rollback = state
            .video_runtime
            .apply_video_settings(&previous.settings, &browser)
            .err()
            .map(|value| format!("; rollback also failed: {value}"))
            .unwrap_or_default();
        return Err(format!("failed to save video settings: {error}{rollback}"));
    }
    {
        let mut snapshot = state.video_settings.lock();
        snapshot.revision = previous.revision.saturating_add(1);
        snapshot.settings = settings;
    }
    Ok(runtime)
}

const fn video_revision_matches(base_revision: u64, current_revision: u64) -> bool {
    base_revision == current_revision
}

const fn input_revision_matches(base_revision: u64, current_revision: u64) -> bool {
    base_revision == current_revision
}

#[derive(Deserialize)]
struct VideoPresentedRequest {
    codec: nfidb_core::VideoCodec,
    #[serde(default)]
    first_keyframe_received: bool,
    #[serde(default)]
    presented: bool,
    #[serde(default)]
    failure_reason: Option<String>,
}

async fn video_presented(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(report): Json<VideoPresentedRequest>,
) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    if report.failure_reason.as_ref().is_some_and(|value| value.len() > 512) {
        return api_error(StatusCode::BAD_REQUEST, "failure reason is too large");
    }
    {
        let mut browser = state.browser_video.lock();
        let codec = browser.get_mut(report.codec);
        codec.first_keyframe_received |= report.first_keyframe_received;
        codec.presented |= report.presented;
        codec.negotiated = true;
        codec.failure_reason = report.failure_reason;
    }
    let browser = state.browser_video.lock().clone();
    let settings = state.video_settings.lock().settings.clone();
    if let Err(error) = state.video_runtime.apply_video_settings(&settings, &browser) {
        return api_error(StatusCode::BAD_REQUEST, &error);
    }
    Json(video_response(&state)).into_response()
}

async fn record_video_benchmark(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(observation): Json<nfidb_core::AutoBenchmarkObservation>,
) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    if let Err(error) = state.video_runtime.record_auto_benchmark(observation) {
        return api_error(StatusCode::BAD_REQUEST, &error);
    }
    let settings = state.video_settings.lock().settings.clone();
    if settings.encoder == nfidb_core::EncoderMode::Auto {
        let previous_codec = state.video_runtime.video_runtime_status().codec;
        let browser = state.browser_video.lock().clone();
        match state.video_runtime.apply_video_settings(&settings, &browser) {
            Ok(runtime) if runtime.codec != previous_codec => {
                state.peer.close().await;
                let _ = state.input.reset_all();
                state.metrics.reset_input_continuity();
            }
            Ok(_) => {}
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
        }
    }
    Json(video_response(&state)).into_response()
}

async fn clear_video_benchmarks(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    match state.video_runtime.clear_auto_benchmarks() {
        Ok(()) => Json(video_response(&state)).into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn files_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(session_id) = authorized_session(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    };
    Json(state.file_transfers.browser_listing(&session_id)).into_response()
}

#[derive(Deserialize)]
struct CreateUploadRequest {
    #[serde(default)]
    upload_id: Option<uuid::Uuid>,
    name: String,
    #[serde(default)]
    mime: String,
    size: u64,
}

async fn create_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateUploadRequest>,
) -> Response {
    let Some(session_id) = authorize_mutation(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    };
    match state
        .file_transfers
        .create_upload(
            session_id,
            request.upload_id,
            &request.name,
            &request.mime,
            request.size,
        )
        .await
    {
        Ok(ticket) => (StatusCode::CREATED, Json(ticket)).into_response(),
        Err(error) => file_error_response(error),
    }
}

async fn upload_status(State(state): State<Arc<AppState>>, Path(id): Path<uuid::Uuid>, headers: HeaderMap) -> Response {
    let Some(session_id) = authorized_session(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    };
    match state.file_transfers.upload_progress(id, &session_id) {
        Ok(progress) => Json(progress).into_response(),
        Err(error) => file_error_response(error),
    }
}

async fn write_upload_chunk(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(session_id) = authorize_mutation(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    };
    let offset = match header_u64(&headers, "x-nfidb-offset") {
        Ok(value) => value,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, message),
    };
    let Some(checksum) = headers
        .get("x-nfidb-chunk-sha256")
        .and_then(|value| value.to_str().ok())
    else {
        return api_error(StatusCode::BAD_REQUEST, "x-nfidb-chunk-sha256 is required");
    };
    let bytes = match to_bytes(body, UPLOAD_CHUNK_SIZE as usize + 1).await {
        Ok(bytes) => bytes,
        Err(_) => return api_error(StatusCode::PAYLOAD_TOO_LARGE, "upload chunk exceeds the 1 MiB limit"),
    };
    match state
        .file_transfers
        .write_upload_chunk(id, &session_id, offset, bytes, checksum)
        .await
    {
        Ok(progress) => Json(progress).into_response(),
        Err(error) => file_error_response(error),
    }
}

async fn complete_upload(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
) -> Response {
    let Some(session_id) = authorize_mutation(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    };
    match state.file_transfers.complete_upload(id, &session_id).await {
        Ok(complete) => Json(complete).into_response(),
        Err(error) => file_error_response(error),
    }
}

async fn cancel_upload(State(state): State<Arc<AppState>>, Path(id): Path<uuid::Uuid>, headers: HeaderMap) -> Response {
    let Some(session_id) = authorize_mutation(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    };
    match state.file_transfers.cancel_upload(id, &session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => file_error_response(error),
    }
}

async fn remove_outbox_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
) -> Response {
    if authorize_mutation(&headers, &state.session).is_none() {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token or request origin");
    }
    if state.file_transfers.remove_outgoing(id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        api_error(StatusCode::NOT_FOUND, "file is no longer queued")
    }
}

async fn download_outbox_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
    Query(query): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(session_id) = authorized_session(&headers, &state.session) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    };
    let range_header = headers.get(header::RANGE).and_then(|value| value.to_str().ok());
    match state
        .file_transfers
        .open_download(id, session_id, range_header, query.remove == Some(1))
        .await
    {
        Ok(download) => {
            let size = if download.file.size == 0 {
                0
            } else if download.partial {
                download.range.len()
            } else {
                download.file.size
            };
            let mut response = Response::new(download.body);
            *response.status_mut() = if download.partial {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            };
            let headers = response.headers_mut();
            insert_header(headers, header::CONTENT_TYPE, &download.file.mime);
            insert_header(
                headers,
                header::CONTENT_DISPOSITION,
                &content_disposition(&download.file.name),
            );
            insert_header(headers, header::CONTENT_LENGTH, &size.to_string());
            headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
            if download.partial {
                insert_header(
                    headers,
                    header::CONTENT_RANGE,
                    &format!(
                        "bytes {}-{}/{}",
                        download.range.start, download.range.end, download.file.size
                    ),
                );
            }
            if let Some(checksum) = download.file.sha256 {
                insert_header(headers, "x-nfidb-sha256", &checksum);
            }
            response
        }
        Err(error) => file_error_response(error),
    }
}

#[derive(Default, Deserialize)]
struct DownloadQuery {
    remove: Option<u8>,
}

#[derive(Deserialize)]
struct TokenRequest {
    token: String,
}

async fn disconnect(State(state): State<Arc<AppState>>, Json(request): Json<TokenRequest>) -> Response {
    if !state.session.authorize(&request.token) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    state.peer.close().await;
    let session_id = state.session.session_id();
    state.file_transfers.cancel_session_uploads(&session_id);
    let _ = state.input.reset_all();
    state.metrics.set_connected(false);
    state.metrics.reset_input_continuity();
    state.session.disconnect();
    StatusCode::NO_CONTENT.into_response()
}

async fn webrtc_offer(State(state): State<Arc<AppState>>, Json(offer): Json<WebRtcOffer>) -> Response {
    if !state.session.authorize(&offer.token) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    match accept_offer(
        offer,
        Arc::clone(&state.input),
        Arc::clone(&state.metrics),
        state.video_tx.subscribe(),
        state.keyframe_request.clone(),
        state.video_runtime.video_runtime_status().codec,
        &state.peer,
    )
    .await
    {
        Ok(answer) => Json(answer).into_response(),
        Err(error) => {
            tracing::error!(%error, "WebRTC negotiation failed");
            api_error(StatusCode::BAD_GATEWAY, &format!("WebRTC negotiation failed: {error}"))
        }
    }
}

async fn websocket(State(state): State<Arc<AppState>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    if !authorized_cookie(&headers, &state.session) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    upgrade
        .on_upgrade(move |socket| websocket_loop(socket, state))
        .into_response()
}

fn authorized_cookie(headers: &HeaderMap, session: &SessionManager) -> bool {
    authorized_session(headers, session).is_some()
}

fn authorized_session(headers: &HeaderMap, session: &SessionManager) -> Option<uuid::Uuid> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| cookie_value(cookies, "nfidb_token"))
        .filter(|token| session.authorize(token))
        .map(|_| session.session_id())
}

fn authorize_mutation(headers: &HeaderMap, session: &SessionManager) -> Option<uuid::Uuid> {
    same_origin(headers)
        .then(|| authorized_session(headers, session))
        .flatten()
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get(header::HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    origin.eq_ignore_ascii_case(&format!("http://{host}")) || origin.eq_ignore_ascii_case(&format!("https://{host}"))
}

fn cookie_value<'a>(cookies: &'a str, name: &str) -> Option<&'a str> {
    cookies.split(';').find_map(|cookie| {
        let (key, value) = cookie.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

async fn websocket_loop(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let session_id = state.session.session_id();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        process_input_packet(state.input.as_ref(), state.metrics.as_ref(), &bytes, "websocket");
                    }
                    Some(Ok(Message::Text(text))) => {
                        if text.len() <= 64 * 1024
                            && let Ok(message) = serde_json::from_str::<ClientControlMessage>(&text)
                        {
                            match message.kind.as_str() {
                                "ping" => {
                                    if let Some(t0) = message.t0 {
                                        let t1 = epoch_ms();
                                        let pong = serde_json::json!({ "type": "pong", "t0": t0, "t1": t1, "t2": epoch_ms() });
                                        if sender.send(Message::Text(pong.to_string().into())).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                "client-diagnostics" => {
                                    if let Some(sample) = message.sample {
                                        state.metrics.set_rtt_ms(sample.network.rtt_ms);
                                        state.metrics.set_client_clock_offset_ms(sample.network.clock_offset_ms);
                                        state.diagnostics.record(sample, state.metrics.snapshot());
                                    }
                                }
                                "request-keyframe" => {
                                    state.metrics.video_recovery_requested();
                                    state.keyframe_request.request();
                                    tracing::info!("browser requested a video recovery keyframe");
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
            _ = interval.tick() => {
                if state.session.session_id() != session_id {
                    break;
                }
                let message = serde_json::json!({ "type": "stats", "stats": state.metrics.snapshot() }).to_string();
                if sender.send(Message::Text(message.into())).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = state.input.reset_all();
    state.metrics.reset_input_continuity();
}

#[derive(Deserialize)]
struct ClientControlMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    t0: Option<f64>,
    #[serde(default)]
    sample: Option<ClientDiagnosticSample>,
}

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

async fn static_asset(OriginalUri(uri): OriginalUri) -> Response {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() || !path.contains('.') {
        path = "index.html";
    }
    match WebAssets::get(path) {
        Some(asset) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let mut response = Response::new(Body::from(asset.data));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime.as_ref()).unwrap_or(HeaderValue::from_static("application/octet-stream")),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(if path == "index.html" {
                    "no-store"
                } else {
                    "public, max-age=31536000, immutable"
                }),
            );
            response
                .headers_mut()
                .insert("x-content-type-options", HeaderValue::from_static("nosniff"));
            response
                .headers_mut()
                .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
            response
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn file_error_response(error: FileTransferError) -> Response {
    let status = match error.kind {
        FileTransferErrorKind::Disabled => StatusCode::SERVICE_UNAVAILABLE,
        FileTransferErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        FileTransferErrorKind::NotFound => StatusCode::NOT_FOUND,
        FileTransferErrorKind::Invalid => StatusCode::BAD_REQUEST,
        FileTransferErrorKind::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        FileTransferErrorKind::Conflict => StatusCode::CONFLICT,
        FileTransferErrorKind::Range => StatusCode::RANGE_NOT_SATISFIABLE,
        FileTransferErrorKind::Io => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(serde_json::json!({
            "error": error.message,
            "expected_offset": error.expected_offset,
        })),
    )
        .into_response()
}

fn header_u64(headers: &HeaderMap, name: &'static str) -> Result<u64, &'static str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or("x-nfidb-offset must be an unsigned integer")
}

fn insert_header(headers: &mut HeaderMap, name: impl header::IntoHeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{cookie_value, input_revision_matches, same_origin, select_best_local_ipv4, video_revision_matches};

    #[test]
    fn extracts_exact_cookie_without_prefix_confusion() {
        let cookies = "theme=dark; nfidb_token=correct-token; nfidb_token_old=wrong";
        assert_eq!(cookie_value(cookies, "nfidb_token"), Some("correct-token"));
        assert_eq!(cookie_value(cookies, "missing"), None);
    }

    #[test]
    fn accepts_only_matching_mutation_origins_when_origin_is_present() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.10:47831"));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://192.168.1.10:47831"));
        assert!(same_origin(&headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://attacker.test"));
        assert!(!same_origin(&headers));
    }

    #[test]
    fn video_edits_reject_stale_revisions() {
        assert!(video_revision_matches(14, 14));
        assert!(!video_revision_matches(13, 14));
        assert!(!video_revision_matches(15, 14));
    }

    #[test]
    fn input_edits_reject_stale_revisions() {
        assert!(input_revision_matches(8, 8));
        assert!(!input_revision_matches(7, 8));
        assert!(!input_revision_matches(9, 8));
    }

    #[test]
    fn macos_physical_lan_beats_overlay_interfaces() {
        let selected = select_best_local_ipv4(vec![
            ("feth204".to_owned(), Ipv4Addr::new(10, 207, 7, 167)),
            ("utun4".to_owned(), Ipv4Addr::new(100, 119, 178, 41)),
            ("en0".to_owned(), Ipv4Addr::new(192, 168, 1, 209)),
        ]);
        assert_eq!(selected, Some(Ipv4Addr::new(192, 168, 1, 209)));
    }

    #[test]
    fn windows_and_linux_physical_lan_beats_known_virtual_interfaces() {
        for candidates in [
            vec![
                ("Tailscale".to_owned(), Ipv4Addr::new(100, 90, 80, 70)),
                ("Wi-Fi".to_owned(), Ipv4Addr::new(192, 168, 10, 12)),
            ],
            vec![
                ("docker0".to_owned(), Ipv4Addr::new(172, 17, 0, 1)),
                ("wlp2s0".to_owned(), Ipv4Addr::new(10, 0, 0, 14)),
            ],
        ] {
            let expected = candidates[1].1;
            assert_eq!(select_best_local_ipv4(candidates), Some(expected));
        }
    }

    #[test]
    fn private_address_beats_link_local_even_on_an_unknown_interface() {
        let selected = select_best_local_ipv4(vec![
            ("en0".to_owned(), Ipv4Addr::new(169, 254, 1, 2)),
            ("mystery0".to_owned(), Ipv4Addr::new(192, 168, 50, 5)),
        ]);
        assert_eq!(selected, Some(Ipv4Addr::new(192, 168, 50, 5)));
    }
}
