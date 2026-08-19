use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use nfidb_core::{EncodedVideoFrame, InputSink, KeyframeRequest, Metrics, SessionManager};
use parking_lot::Mutex;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use crate::diagnostics::{
    ClientDiagnosticSample, DiagnosticRecorder, DiagnosticReport, DiagnosticSummary, RecordedDiagnosticSample,
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
    pub mode: String,
    pub mdns: bool,
    pub touch_default: bool,
    pub mouse_enabled: bool,
    pub keyboard_enabled: bool,
    pub gestures_default: bool,
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
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let state = Arc::new(AppState {
            options,
            session,
            metrics,
            input,
            video_tx,
            keyframe_request,
            peer: ActivePeer::default(),
            diagnostics: DiagnosticRecorder::default(),
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
        self.state.session.rotate();
        self.close_active_peer();
    }

    pub fn rotate_pairing_if_expired(&self) -> bool {
        if self.state.session.rotate_if_expired() {
            self.close_active_peer();
            true
        } else {
            false
        }
    }

    fn close_active_peer(&self) {
        self.state.metrics.set_connected(false);
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
    let mut candidates: Vec<_> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(ip) if !ip.is_loopback() && (ip.is_private() || ip.is_link_local()) => Some((name, ip)),
            _ => None,
        })
        .collect();
    candidates.sort_by_key(|(name, ip)| {
        let adapter_name = name.to_ascii_lowercase();
        let physical_lan = adapter_name.contains("wi-fi")
            || (adapter_name.contains("ethernet") && !adapter_name.contains("vethernet"));
        (ip.is_link_local(), !physical_lan)
    });
    candidates.first().map(|(_, ip)| *ip).unwrap_or(Ipv4Addr::LOCALHOST)
}

#[derive(Serialize)]
struct StatusResponse {
    #[serde(flatten)]
    session: nfidb_core::PublicSession,
    protocol_version: u8,
    webrtc: bool,
    touch_default: bool,
    mouse_enabled: bool,
    keyboard_enabled: bool,
    gestures_default: bool,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    Json(StatusResponse {
        session: state
            .session
            .public(state.options.host_name.clone(), state.options.mode.clone()),
        protocol_version: nfidb_protocol::PROTOCOL_VERSION,
        webrtc: true,
        touch_default: state.options.touch_default,
        mouse_enabled: state.options.mouse_enabled,
        keyboard_enabled: state.options.keyboard_enabled,
        gestures_default: state.options.gestures_default,
    })
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

#[derive(Deserialize)]
struct TokenRequest {
    token: String,
}

async fn disconnect(State(state): State<Arc<AppState>>, Json(request): Json<TokenRequest>) -> Response {
    if !state.session.authorize(&request.token) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid session token");
    }
    state.peer.close().await;
    let _ = state.input.reset_all();
    state.metrics.set_connected(false);
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
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| cookie_value(cookies, "nfidb_token"))
        .is_some_and(|token| session.authorize(token))
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

#[cfg(test)]
mod tests {
    use super::cookie_value;

    #[test]
    fn extracts_exact_cookie_without_prefix_confusion() {
        let cookies = "theme=dark; nfidb_token=correct-token; nfidb_token_old=wrong";
        assert_eq!(cookie_value(cookies, "nfidb_token"), Some("correct-token"));
        assert_eq!(cookie_value(cookies, "missing"), None);
    }
}
