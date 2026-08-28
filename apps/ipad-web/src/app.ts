import {
  disconnect,
  getDiagnosticSummary,
  getInputControl,
  getMetrics,
  getStatus,
  pairWithPin,
  pairWithQr,
  recordAutoBenchmark,
  reportVideoPresented,
  sendBrowserVideoCapabilities,
  sendOffer,
  setInputSettings,
  setVideoSettings,
  type BrowserCodecCapability,
  type BrowserVideoCapabilities,
  type AutoBenchmarkObservation,
  type EncoderMode,
  type HostDiagnosticSummary,
  type HostMetrics,
  type HostStatus,
  type InputControl,
  type VideoCodec,
  type VideoConfig,
  type VideoControl,
} from "./api";
import {
  getFileListing,
  loadAutoClearDownloads,
  outgoingDownloadUrl,
  removeOutgoingFile,
  saveAutoClearDownloads,
  uploadFile,
  type FileListing,
} from "./file-transfer";
import type { FitMode } from "./geometry";
import { normalizePinEntry } from "./pin-entry";
import { PointerEngine } from "./pointer-engine";
import { RemoteInputEngine } from "./remote-input";
import { RemoteCommand, type RemoteCommandValue } from "./protocol";

declare const __NFIDB_CLIENT_VERSION__: string;

type ConnectionState = "pairing" | "connecting" | "connected" | "input-only" | "failed";
type InputTransport = "datachannel" | "websocket";

interface RtcDiagnosticStats {
  inboundVideo: Record<string, unknown> | null;
  candidatePair: Record<string, unknown> | null;
  localCandidate: Record<string, unknown> | null;
  remoteCandidate: Record<string, unknown> | null;
}

interface PreviousRtcCounters {
  atMs: number;
  bytesReceived: number;
  framesDecoded: number;
  packetsLost: number;
  jitterBufferDelay: number;
  jitterBufferEmittedCount: number;
  totalDecodeTime: number;
  totalVideoFrames: number;
  droppedVideoFrames: number;
}

interface LiveClientDiagnostic {
  video: {
    decodeFps: number;
    playbackFps: number;
    presentationDropPercent: number;
    decodeMsPerFrame: number;
    jitterBufferMsPerFrame: number;
  };
  network: {
    rttMs: number;
    receiveMbps: number;
    availableIncomingMbps: number;
    packetsLost: number;
    jitterMs: number;
  };
  frameTiming: {
    frameGapP95Ms: number;
    frameGapMaxMs: number;
    captureToPresentP95Ms: number | null;
    estimatedPipelineMs: number;
  };
}

interface QueuedUpload {
  localId: number;
  file: File;
  uploaded: number;
  state: "queued" | "uploading" | "completed" | "canceled" | "failed";
  message: string;
  retries: number;
}

export interface ClientDiagnosticSnapshot {
  connectionState: ConnectionState;
  peerConnectionState: RTCPeerConnectionState | "none";
  inputTransport: InputTransport | "pending";
  dataChannelBufferedBytes: number;
  webSocketBufferedBytes: number;
  video: {
    width: number;
    height: number;
    readyState: number;
    currentTime: number;
    totalFrames: number;
    droppedFrames: number;
    startupMs: number | null;
  };
  inboundVideo: Record<string, unknown> | null;
  candidatePair: Record<string, unknown> | null;
  host: HostMetrics;
  hostDiagnostics: HostDiagnosticSummary;
}

export class NfidbApp {
  private readonly root: HTMLElement;
  private status: HostStatus | null = null;
  private token = "";
  private state: ConnectionState = "pairing";
  private peer: RTCPeerConnection | null = null;
  private channel: RTCDataChannel | null = null;
  private socket: WebSocket | null = null;
  private pointerEngine: PointerEngine | null = null;
  private remoteInputEngine: RemoteInputEngine | null = null;
  private inputTransport: InputTransport | null = null;
  private readonly pendingInput: ArrayBuffer[] = [];
  private fitMode: FitMode = "fit";
  private touchEnabled = false;
  private gesturesEnabled = true;
  private metrics: HostMetrics | null = null;
  private hideToolbarTimer = 0;
  private videoTrackAtMs = 0;
  private firstVideoFrameAtMs = 0;
  private videoRecoveryTimer = 0;
  private videoRecoveryAttempts = 0;
  private diagnosticTimer = 0;
  private diagnosticSequence = 0;
  private diagnosticFailures = 0;
  private diagnosticCollectionActive = false;
  private previousRtcCounters: PreviousRtcCounters | null = null;
  private liveClientDiagnostic: LiveClientDiagnostic | null = null;
  private rttMs = 0;
  private clockOffsetMs = 0;
  private pairingRequestActive = false;
  private videoFrameRequest = 0;
  private frameCallbackCount = 0;
  private lastFrameCallbackAtMs = 0;
  private readonly frameGapsMs: number[] = [];
  private readonly captureToPresentMs: number[] = [];
  private readonly receiveToPresentMs: number[] = [];
  private readonly frameProcessingMs: number[] = [];
  private fileListing: FileListing | null = null;
  private fileRefreshTimer = 0;
  private fileRefreshActive = false;
  private knownOutgoingIds: Set<string> | null = null;
  private autoClearDownloads = loadAutoClearDownloads();
  private readonly uploadQueue: QueuedUpload[] = [];
  private uploadQueueActive = false;
  private activeUploadId = 0;
  private activeUploadAbort: AbortController | null = null;
  private nextUploadId = 1;
  private videoControl: VideoControl | null = null;
  private inputControl: InputControl | null = null;
  private inputUpdateActive = false;
  private browserVideoCapabilities: BrowserVideoCapabilities | null = null;
  private videoPresentationReported = false;
  private videoBenchmarkRunning = false;
  private videoBenchmarkStatus = "";

  constructor(root: HTMLElement) {
    this.root = root;
  }

  async start(): Promise<void> {
    this.renderLoading();
    try {
      this.status = await getStatus();
      this.touchEnabled = this.status.touch_default;
      this.gesturesEnabled = this.status.gestures_default;
      const qrSecret = new URLSearchParams(location.search).get("qr");
      if (qrSecret) {
        history.replaceState(null, "", location.pathname);
        const paired = await pairWithQr(qrSecret);
        this.token = paired.access_token;
        await this.connect();
      } else {
        this.renderPairing();
      }
    } catch (error) {
      this.renderFatal(error instanceof Error ? error.message : String(error));
    }
  }

  private renderLoading(): void {
    this.root.innerHTML = `<main class="pairing-shell"><div class="wordmark"><span>NFi</span>DB</div><p class="eyebrow">LOCAL BRIDGE</p><div class="loader" aria-label="Loading"></div></main>`;
  }

  private renderPairing(error = ""): void {
    this.state = "pairing";
    this.pairingRequestActive = false;
    const host = escapeHtml(this.status?.host_name ?? "NFiDB host");
    const hostDevice = this.status?.host_platform === "macos" ? "Mac" : "Windows host";
    this.root.innerHTML = `
      <main class="pairing-shell">
        <section class="pair-panel" aria-labelledby="pair-title">
          <div class="brand-lockup"><div class="wordmark"><span>NFi</span>DB</div><p>No Frills iPad Drawing Bridge</p></div>
          <div class="host-found"><i></i><span>${host} found on your local network</span></div>
          <h1 id="pair-title">Enter the PIN shown on your ${hostDevice}</h1>
          <form id="pairForm" novalidate>
            <label for="pin">Six-digit PIN</label>
            <input id="pin" name="pin" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000 000" aria-describedby="pinHint pairError" autofocus />
            <p id="pinHint" class="pin-hint">Checks automatically after the sixth digit.</p>
            <p id="pairError" class="form-error" role="alert">${escapeHtml(error)}</p>
            <button class="primary-button" type="submit">Connect locally</button>
          </form>
          <div class="privacy-line"><span>NO CLOUD</span><span>NO ACCOUNT</span><span>NO APP</span></div>
          <p class="pair-help">Both devices must be on the same non-isolated Wi-Fi. This page talks directly to your ${hostDevice}.</p>
        </section>
      </main>`;
    const form = this.requiredElement<HTMLFormElement>("pairForm");
    const pin = this.requiredElement<HTMLInputElement>("pin");
    pin.addEventListener("input", () => {
      const entry = normalizePinEntry(pin.value);
      pin.value = entry.formatted;
      if (entry.complete && !this.pairingRequestActive) {
        form.requestSubmit();
      }
    });
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submitPin(pin.value);
    });
  }

  private async submitPin(pin: string): Promise<void> {
    const entry = normalizePinEntry(pin);
    if (!entry.complete || this.pairingRequestActive) {
      if (!entry.complete) {
        const error = this.root.querySelector<HTMLElement>("#pairError");
        if (error) {
          error.textContent = "Enter all six digits.";
        }
      }
      return;
    }
    this.pairingRequestActive = true;
    const button = this.root.querySelector<HTMLButtonElement>("button[type=submit]");
    if (button) {
      button.disabled = true;
      button.textContent = "Pairing…";
    }
    try {
      const result = await pairWithPin(entry.digits);
      this.token = result.access_token;
      await this.connect();
    } catch (error) {
      this.renderPairing(error instanceof Error ? error.message : String(error));
    }
  }

  private async connect(): Promise<void> {
    this.state = "connecting";
    try {
      this.inputControl = await getInputControl();
      this.touchEnabled = this.inputControl.settings.touch_enabled;
      this.gesturesEnabled = this.inputControl.settings.gestures_enabled;
    } catch (error) {
      console.warn("NFiDB could not synchronize input controls; using the advertised defaults", error);
    }
    this.renderSurface();
    this.startFilePolling();
    this.openControlSocket();
    await this.connectVideo();
  }

  private async connectVideo(): Promise<void> {
    try {
      const previousPeer = this.peer;
      this.peer = null;
      if (previousPeer) {
        previousPeer.close();
      }
      this.channel = null;
      if (this.inputTransport === "datachannel") {
        this.inputTransport = this.socket?.readyState === WebSocket.OPEN ? "websocket" : null;
      }
      this.browserVideoCapabilities = detectBrowserVideoCapabilities();
      this.videoControl = await sendBrowserVideoCapabilities(this.browserVideoCapabilities);
      this.videoPresentationReported = false;
      const peer = new RTCPeerConnection({ iceServers: [] });
      this.peer = peer;
      const channel = peer.createDataChannel("input", { ordered: true });
      channel.binaryType = "arraybuffer";
      this.channel = channel;
      channel.addEventListener("open", () => this.drainPendingInput());
      channel.addEventListener("close", () => this.handleInputTransportClose("datachannel"));
      const videoTransceiver = peer.addTransceiver("video", { direction: "recvonly" });
      applyCodecPreference(videoTransceiver, this.videoControl.runtime.codec);
      peer.addEventListener("track", (event) => {
        const video = this.requiredElement<HTMLVideoElement>("remoteVideo");
        this.videoTrackAtMs = performance.now();
        this.firstVideoFrameAtMs = 0;
        this.startVideoRecovery(peer);
        video.srcObject = event.streams[0] ?? new MediaStream([event.track]);
        this.startVideoFrameTelemetry(video);
        video.addEventListener("playing", () => this.markFirstVideoFrame(), { once: true });
        const startPlayback = () => void video.play().catch(() => undefined);
        video.addEventListener("loadedmetadata", startPlayback, { once: true });
        startPlayback();
        window.setTimeout(() => {
          if (this.peer === peer && peer.connectionState === "connected" && this.firstVideoFrameAtMs === 0) {
            this.showNotice("Waiting for a decodable frame. NFiDB is requesting a fresh video keyframe…");
          }
        }, 3000);
      });
      peer.addEventListener("connectionstatechange", () => this.updatePeerState());
      const offer = await peer.createOffer();
      await peer.setLocalDescription(offer);
      await waitForIceGathering(peer, 5000);
      if (!peer.localDescription) {
        throw new Error("Safari did not create a local WebRTC description");
      }
      updateSdpCapabilityEvidence(this.browserVideoCapabilities, peer.localDescription.sdp ?? "");
      this.videoControl = await sendBrowserVideoCapabilities(this.browserVideoCapabilities);
      const answer = await sendOffer(this.token, peer.localDescription.toJSON());
      await peer.setRemoteDescription(answer);
      this.state = "connected";
      this.updateHud();
      this.renderVideoPanel();
    } catch (error) {
      console.error(error);
      this.state = "input-only";
      this.showNotice("Video negotiation failed. Pencil input remains available over the diagnostic channel.");
      this.updateHud();
      this.renderVideoPanel();
    }
  }

  private renderSurface(): void {
    const macHost = this.status?.host_platform === "macos";
    const hostName = macHost ? "Mac" : "Windows";
    const previousAppLabel = macHost ? "Cmd+Shift+Tab" : "Alt+Shift+Tab";
    const nextAppLabel = macHost ? "Cmd+Tab" : "Alt+Tab";
    const overviewLabel = macHost ? "Mission Control" : "Task view";
    const secureAttention = macHost
      ? ""
      : `<button id="secureAttentionButton">Ctrl+Alt+Del</button>`;
    const keyboardHelp = macHost
      ? "Command = Command · Option = Option · Control = Control · Return = Enter · Delete = Backspace"
      : "Option = Alt · Control = Ctrl · Return = Enter · Delete = Backspace";
    this.root.innerHTML = `
      <main id="surface" class="surface fit-mode">
        <video id="remoteVideo" autoplay playsinline muted></video>
        <canvas id="interactionOverlay" aria-label="Remote drawing surface"></canvas>
        <div id="connectionNotice" class="connection-notice" hidden></div>
        <header id="toolbar" class="toolbar visible">
          <div class="toolbar-brand"><b><span>NFi</span>DB</b><small id="connectionState">Connecting</small></div>
          <div class="toolbar-actions">
            <div class="segmented" aria-label="Video sizing">
              <button data-fit="fit" class="active">Fit</button><button data-fit="fill">Fill</button><button data-fit="one-to-one">1:1</button>
            </div>
            <button id="touchButton" class="tool-button" aria-pressed="false">Touch off</button>
            <button id="gestureButton" class="tool-button" aria-pressed="true">Gestures on</button>
            <button id="keyboardButton" class="tool-button" aria-pressed="false">Keyboard</button>
            <button id="filesButton" class="tool-button files-button" aria-pressed="false">Files<span id="filesBadge" class="file-badge" hidden>0</span></button>
            <button id="videoButton" class="tool-button" aria-pressed="false">Video</button>
            <button id="statsButton" class="tool-button" aria-pressed="false">Stats</button>
            <button id="fullscreenButton" class="icon-button" aria-label="Fullscreen">⛶</button>
            <button id="controlsClose" class="icon-button" aria-label="Hide controls">⌃</button>
            <button id="disconnectButton" class="icon-button danger" aria-label="Disconnect">×</button>
          </div>
        </header>
        <aside id="statsPanel" class="stats-panel" hidden>
          <div><span>VIDEO</span><b id="videoStats">Waiting…</b></div>
          <div><span>NETWORK</span><b id="networkStats">Local</b></div>
          <div><span>PLAYOUT</span><b id="playoutStats">Waiting…</b></div>
          <div><span>PIPELINE</span><b id="pipelineStats">Waiting…</b></div>
          <div><span>PENCIL</span><b id="pencilStats">Waiting…</b></div>
          <div><span>PRESSURE / TILT</span><b id="pressureStats">0.00 · 0° / 0°</b></div>
          <div><span>REMOTE INPUT</span><b id="remoteInputStats">Waiting…</b></div>
          <div><span>INTEGRITY</span><b id="integrityStats">Waiting…</b></div>
          <div><span>RECORDER</span><b id="recorderStats">Starting…</b></div>
        </aside>
        <aside id="filesPanel" class="files-panel" hidden aria-label="File transfer">
          <div class="panel-header"><div><b>FILES</b><small>Paired session only</small></div><button id="filesClose" class="icon-button" aria-label="Close files">×</button></div>
          <div id="filesContent" class="files-content"><p class="panel-empty">Loading transfer queue…</p></div>
        </aside>
        <aside id="videoPanel" class="files-panel video-panel" hidden aria-label="Video settings">
          <div class="panel-header"><div><b>VIDEO</b><small>Changes apply live on ${hostName}</small></div><button id="videoClose" class="icon-button" aria-label="Close video settings">×</button></div>
          <div id="videoContent" class="files-content"><p class="panel-empty">Reading encoder capabilities…</p></div>
        </aside>
        <section id="keyboardPanel" class="keyboard-panel" hidden aria-label="Remote keyboard">
          <div class="keyboard-panel-header">
            <div><b>TYPE ON ${hostName.toUpperCase()}</b><small>${keyboardHelp}</small></div>
            <button id="keyboardClose" class="icon-button" aria-label="Close keyboard">×</button>
          </div>
          <textarea id="remoteTextInput" rows="2" inputmode="text" enterkeyhint="enter" autocomplete="off" autocapitalize="none" spellcheck="false" placeholder="Tap here to use the iPad keyboard…"></textarea>
          <div class="remote-key-row" aria-label="Special keys">
            <button data-remote-key="Escape" data-key-label="Escape">Esc</button>
            <button data-remote-key="Tab" data-key-label="Tab">Tab</button>
            <button data-remote-key="Backspace" data-key-label="Backspace">⌫</button>
            <button data-remote-key="Enter" data-key-label="Enter">Enter</button>
            <button data-remote-command="${RemoteCommand.AppPrevious}">${previousAppLabel}</button>
            <button data-remote-command="${RemoteCommand.AppNext}">${nextAppLabel}</button>
            <button data-remote-command="${RemoteCommand.TaskView}">${overviewLabel}</button>
            <button data-remote-command="${RemoteCommand.MinimizeForeground}">Minimize</button>
            ${secureAttention}
          </div>
          <p>Three fingers: swipe left/right to switch apps, up for ${overviewLabel}, down to minimize.${macHost ? "" : " Windows blocks synthetic Ctrl+Alt+Del on its secure screen."}</p>
        </section>
        <button id="toolbarReveal" class="toolbar-reveal" aria-label="Show controls" aria-controls="toolbar" aria-expanded="true"><b>NFi</b><span>Controls</span></button>
      </main>`;
    const overlay = this.requiredElement<HTMLCanvasElement>("interactionOverlay");
    const video = this.requiredElement<HTMLVideoElement>("remoteVideo");
    const resize = () => {
      const ratio = window.devicePixelRatio || 1;
      overlay.width = Math.round(overlay.clientWidth * ratio);
      overlay.height = Math.round(overlay.clientHeight * ratio);
    };
    resize();
    window.addEventListener("resize", resize, { passive: true });
    const inputEnabled = this.status?.mode !== "display-only";
    this.remoteInputEngine = new RemoteInputEngine({
      overlay,
      video,
      send: (packet) => this.sendInput(packet),
      getFitMode: () => this.fitMode,
      getTouchEnabled: () => this.touchEnabled,
      getGesturesEnabled: () => this.gesturesEnabled,
      inputEnabled,
      onNotice: (message) => this.showNotice(message),
    });
    this.remoteInputEngine.attachTextInput(this.requiredElement<HTMLTextAreaElement>("remoteTextInput"));
    this.pointerEngine = new PointerEngine({
      overlay,
      video,
      send: (packet) => this.sendInput(packet),
      getFitMode: () => this.fitMode,
      getTouchEnabled: () => this.touchEnabled,
      inputEnabled,
    });
    this.bindSurfaceControls();
    this.updateRemoteControlAvailability();
    this.scheduleToolbarHide();
  }

  private updateRemoteControlAvailability(): void {
    const touchButton = this.requiredElement<HTMLButtonElement>("touchButton");
    touchButton.textContent = this.touchEnabled ? "Touch on" : "Touch off";
    touchButton.setAttribute("aria-pressed", String(this.touchEnabled));
    touchButton.disabled = this.inputUpdateActive || this.status?.mode === "display-only";
    touchButton.title = this.status?.mode === "display-only"
      ? "Input forwarding is disabled on the host"
      : this.status?.host_platform === "macos"
        ? "Use one finger as the Mac pointer"
        : "Send fingers as native Windows touch";
    const gestureButton = this.requiredElement<HTMLButtonElement>("gestureButton");
    gestureButton.disabled = this.inputUpdateActive || this.touchEnabled || this.status?.mode === "display-only";
    gestureButton.textContent = this.touchEnabled ? "Gestures paused" : this.gesturesEnabled ? "Gestures on" : "Gestures off";
    gestureButton.setAttribute("aria-pressed", String(this.gesturesEnabled));
    const keyboardButton = this.requiredElement<HTMLButtonElement>("keyboardButton");
    keyboardButton.disabled = this.status?.keyboard_enabled === false;
    keyboardButton.title = keyboardButton.disabled ? "Keyboard forwarding is disabled on the host" : "Open remote keyboard";
    const filesButton = this.requiredElement<HTMLButtonElement>("filesButton");
    filesButton.disabled = this.status?.file_transfer_enabled === false;
    filesButton.title = filesButton.disabled ? "File transfer is disabled on the host" : "Exchange files with the host";
  }

  private markFirstVideoFrame(): void {
    if (this.firstVideoFrameAtMs === 0) {
      this.firstVideoFrameAtMs = performance.now();
      this.stopVideoRecovery();
      this.renderStats();
      if (!this.videoPresentationReported && this.videoControl) {
        this.videoPresentationReported = true;
        void reportVideoPresented(this.videoControl.runtime.codec, true, true)
          .then((control) => {
            this.videoControl = control;
            this.renderVideoPanel();
          })
          .catch((error) => console.debug("NFiDB presentation verification was not recorded", error));
      }
    }
  }

  private startVideoRecovery(peer: RTCPeerConnection): void {
    this.stopVideoRecovery();
    this.videoRecoveryAttempts = 0;
    const request = () => {
      if (
        this.peer !== peer ||
        this.firstVideoFrameAtMs > 0 ||
        peer.connectionState === "closed" ||
        peer.connectionState === "failed"
      ) {
        this.stopVideoRecovery();
        return;
      }
      if (peer.connectionState === "connected") {
        let sent = false;
        if (this.socket?.readyState === WebSocket.OPEN) {
          this.socket.send(JSON.stringify({ type: "request-keyframe" }));
          sent = true;
        } else if (this.channel?.readyState === "open") {
          this.channel.send("request-keyframe");
          sent = true;
        }
        if (sent) {
          this.videoRecoveryAttempts += 1;
        }
      }
      const delay = Math.min(5000, 1500 * 2 ** Math.min(this.videoRecoveryAttempts, 2));
      this.videoRecoveryTimer = window.setTimeout(request, delay);
    };
    this.videoRecoveryTimer = window.setTimeout(request, 1500);
  }

  private stopVideoRecovery(): void {
    window.clearTimeout(this.videoRecoveryTimer);
    this.videoRecoveryTimer = 0;
  }

  private startVideoFrameTelemetry(video: HTMLVideoElement): void {
    this.frameCallbackCount = 0;
    this.lastFrameCallbackAtMs = 0;
    this.frameGapsMs.length = 0;
    this.captureToPresentMs.length = 0;
    this.receiveToPresentMs.length = 0;
    this.frameProcessingMs.length = 0;
    if (!("requestVideoFrameCallback" in video)) {
      return;
    }
    const callback = (now: DOMHighResTimeStamp, metadata: VideoFrameCallbackMetadata) => {
      this.markFirstVideoFrame();
      this.frameCallbackCount += 1;
      if (this.lastFrameCallbackAtMs > 0) {
        pushBounded(this.frameGapsMs, now - this.lastFrameCallbackAtMs, 900);
      }
      this.lastFrameCallbackAtMs = now;
      const values = metadata as unknown as Record<string, unknown>;
      const presentation = finiteNumber(values.presentationTime) ?? now;
      const capture = finiteNumber(values.captureTime);
      const receive = finiteNumber(values.receiveTime);
      const processingSeconds = finiteNumber(values.processingDuration);
      if (capture !== null) {
        pushValidDuration(this.captureToPresentMs, presentation - capture, 900);
      }
      if (receive !== null) {
        pushValidDuration(this.receiveToPresentMs, presentation - receive, 900);
      }
      if (processingSeconds !== null) {
        pushValidDuration(this.frameProcessingMs, processingSeconds * 1000, 900);
      }
      this.videoFrameRequest = video.requestVideoFrameCallback(callback);
    };
    this.videoFrameRequest = video.requestVideoFrameCallback(callback);
  }

  private bindSurfaceControls(): void {
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-fit]")) {
      button.addEventListener("click", () => {
        this.fitMode = button.dataset.fit as FitMode;
        for (const candidate of this.root.querySelectorAll("[data-fit]")) {
          candidate.classList.toggle("active", candidate === button);
        }
        const video = this.requiredElement<HTMLVideoElement>("remoteVideo");
        video.style.objectFit = this.fitMode === "fit" ? "contain" : this.fitMode === "fill" ? "cover" : "none";
        this.scheduleToolbarHide();
      });
    }
    this.requiredElement("touchButton").addEventListener("click", () => {
      void this.updateInputSettings({ touch_enabled: !this.touchEnabled });
    });
    this.requiredElement("gestureButton").addEventListener("click", () => {
      void this.updateInputSettings({ gestures_enabled: !this.gesturesEnabled });
    });
    this.requiredElement("keyboardButton").addEventListener("click", () => {
      const panel = this.requiredElement<HTMLElement>("keyboardPanel");
      panel.hidden = !panel.hidden;
      this.requiredElement("keyboardButton").setAttribute("aria-pressed", String(!panel.hidden));
      if (!panel.hidden) {
        this.remoteInputEngine?.focusTextInput();
      }
      this.scheduleToolbarHide();
    });
    this.requiredElement("keyboardClose").addEventListener("click", () => {
      this.requiredElement<HTMLElement>("keyboardPanel").hidden = true;
      this.requiredElement("keyboardButton").setAttribute("aria-pressed", "false");
      this.scheduleToolbarHide();
    });
    this.requiredElement("filesButton").addEventListener("click", () => {
      const panel = this.requiredElement<HTMLElement>("filesPanel");
      if (panel.hidden) {
        this.openFilesPanel();
      } else {
        this.closeFilesPanel();
      }
    });
    this.requiredElement("filesClose").addEventListener("click", () => this.closeFilesPanel());
    this.requiredElement("videoButton").addEventListener("click", () => {
      const panel = this.requiredElement<HTMLElement>("videoPanel");
      panel.hidden = !panel.hidden;
      this.requiredElement("videoButton").setAttribute("aria-pressed", String(!panel.hidden));
      if (!panel.hidden) {
        this.renderVideoPanel();
      }
      this.scheduleToolbarHide();
    });
    this.requiredElement("videoClose").addEventListener("click", () => {
      this.requiredElement<HTMLElement>("videoPanel").hidden = true;
      this.requiredElement("videoButton").setAttribute("aria-pressed", "false");
      this.scheduleToolbarHide();
    });
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-remote-key]")) {
      button.addEventListener("click", () => {
        const code = button.dataset.remoteKey;
        if (code) {
          this.remoteInputEngine?.tapKey(code, button.dataset.keyLabel ?? code);
          this.remoteInputEngine?.focusTextInput();
        }
      });
    }
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-remote-command]")) {
      button.addEventListener("click", () => {
        const command = Number(button.dataset.remoteCommand) as RemoteCommandValue;
        this.remoteInputEngine?.sendCommand(command);
        this.remoteInputEngine?.focusTextInput();
      });
    }
    this.root.querySelector<HTMLElement>("#secureAttentionButton")?.addEventListener("click", () => {
      this.remoteInputEngine?.sendChord([
        { code: "ControlLeft", key: "Control" },
        { code: "AltLeft", key: "Alt" },
        { code: "Delete", key: "Delete" },
      ]);
      this.showNotice("Ctrl+Alt+Delete was forwarded; Windows blocks synthetic input on the secure screen.");
      this.remoteInputEngine?.focusTextInput();
    });
    this.requiredElement("statsButton").addEventListener("click", () => {
      const panel = this.requiredElement<HTMLElement>("statsPanel");
      panel.hidden = !panel.hidden;
      this.requiredElement("statsButton").setAttribute("aria-pressed", String(!panel.hidden));
      this.scheduleToolbarHide();
    });
    this.requiredElement("fullscreenButton").addEventListener("click", () => {
      const surface = this.requiredElement<HTMLElement>("surface");
      if (surface.requestFullscreen) {
        void surface.requestFullscreen().catch(() => this.showNotice("Fullscreen is unavailable in this Safari configuration."));
      } else {
        this.showNotice("Use Safari’s Add to Home Screen for the least browser chrome.");
      }
    });
    this.requiredElement("disconnectButton").addEventListener("click", () => void this.disconnect());
    this.requiredElement("controlsClose").addEventListener("click", () => this.hideToolbar());
    this.requiredElement("toolbarReveal").addEventListener("click", () => this.showToolbar());
    this.requiredElement("interactionOverlay").addEventListener("pointerdown", () => this.hideToolbar(), { passive: true });
  }

  private async updateInputSettings(
    changes: Partial<InputControl["settings"]>,
  ): Promise<void> {
    if (this.inputUpdateActive || this.status?.mode === "display-only") {
      return;
    }
    this.inputUpdateActive = true;
    this.updateRemoteControlAvailability();
    try {
      const current = this.inputControl ?? await getInputControl();
      const settings = { ...current.settings, ...changes };
      this.pointerEngine?.cancelAll();
      this.remoteInputEngine?.resetRemoteInput();
      this.inputControl = await setInputSettings(current.revision, settings);
      this.touchEnabled = this.inputControl.settings.touch_enabled;
      this.gesturesEnabled = this.inputControl.settings.gestures_enabled;
      const macHost = this.status?.host_platform === "macos";
      this.showNotice(this.touchEnabled
        ? macHost ? "Finger input now controls the Mac pointer." : "Finger input is now native Windows touch."
        : this.gesturesEnabled
          ? `Finger touch is off; three-finger ${macHost ? "Mac" : "Windows"} gestures are active.`
          : "Finger touch and shortcut gestures are off.");
    } catch (error) {
      try {
        this.inputControl = await getInputControl();
        this.touchEnabled = this.inputControl.settings.touch_enabled;
        this.gesturesEnabled = this.inputControl.settings.gestures_enabled;
      } catch {
        // Keep the last confirmed state when reconciliation is unavailable.
      }
      this.showNotice(`Input setting failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      this.inputUpdateActive = false;
      this.updateRemoteControlAvailability();
      this.scheduleToolbarHide();
    }
  }

  private async refreshInputControl(): Promise<void> {
    if (this.inputUpdateActive) {
      return;
    }
    try {
      const control = await getInputControl();
      if (this.inputControl?.revision === control.revision) {
        return;
      }
      this.pointerEngine?.cancelAll();
      this.remoteInputEngine?.resetRemoteInput();
      this.inputControl = control;
      this.touchEnabled = control.settings.touch_enabled;
      this.gesturesEnabled = control.settings.gestures_enabled;
      this.updateRemoteControlAvailability();
    } catch {
      // Input transport remains usable with the last confirmed settings.
    }
  }

  private openFilesPanel(): void {
    const panel = this.requiredElement<HTMLElement>("filesPanel");
    panel.hidden = false;
    this.requiredElement("filesButton").setAttribute("aria-pressed", "true");
    this.renderFilesContent();
    void this.refreshFileListing();
    this.scheduleToolbarHide();
  }

  private closeFilesPanel(): void {
    this.requiredElement<HTMLElement>("filesPanel").hidden = true;
    this.requiredElement("filesButton").setAttribute("aria-pressed", "false");
    this.scheduleToolbarHide();
  }

  private renderVideoPanel(): void {
    const content = this.root.querySelector<HTMLElement>("#videoContent");
    if (!content) {
      return;
    }
    const control = this.videoControl;
    if (!control) {
      content.innerHTML = `<p class="panel-empty">Waiting for host video capabilities…</p>`;
      return;
    }
    const settings = control.settings.settings;
    const preset = settings.presets[settings.profile];
    const activeCodec = control.runtime.codec;
    const bitrate = activeCodec === "h264"
      ? preset.bitrates.h264_mbps
      : activeCodec === "hevc"
        ? (preset.bitrates.hevc_mbps ?? preset.bitrates.h264_mbps)
        : (preset.bitrates.av1_mbps ?? preset.bitrates.h264_mbps);
    const modes: Array<[typeof settings.encoder, string]> = [
      ["auto", "Auto — Recommended"],
      ["h264-hardware", "H.264 Hardware"],
      ["hevc-hardware", "HEVC Hardware"],
      ["av1-hardware", "AV1 Hardware"],
      ["h264-software", "H.264 Software"],
    ];
    const encoderOptions = modes.map(([mode, label]) => {
      const availability = control.compatibility.find((item) => item.mode === mode);
      const disabled = mode !== "auto" && availability?.availability === "unavailable";
      return `<option value="${mode}" ${settings.encoder === mode ? "selected" : ""} ${disabled ? "disabled" : ""}>${escapeHtml(label)}${disabled ? " — unavailable" : ""}</option>`;
    }).join("");
    const capabilities = control.compatibility.map((item) => `
      <li><b>${escapeHtml(encoderModeLabel(item.mode))}</b><span class="video-state ${item.availability}">${escapeHtml(item.availability)}</span><small>${escapeHtml(item.reason)}</small></li>`).join("");
    const learnedDetails = control.learned_results.map((result) => `
      <li><b>${escapeHtml(encoderModeLabel(result.mode))}</b><span>${result.score.score === null ? "rejected" : `${result.score.score.toFixed(1)} score`}</span>
      <small>${result.metrics.presented_fps?.toFixed(1) ?? "—"} fps · ${result.metrics.actual_mbps.toFixed(2)} Mbps · ${result.metrics.encode_p95_ms.toFixed(2)} ms encode p95 · ${result.metrics.pipeline_p95_ms?.toFixed(1) ?? "—"} ms pipeline${result.score.reasons.length ? ` · ${escapeHtml(result.score.reasons.join("; "))}` : ""}</small></li>`).join("");
    content.innerHTML = `
      <section class="video-active">
        <span>ACTIVE PATH</span>
        <b>${escapeHtml(encoderModeLabel(control.runtime.active_mode))}</b>
        <small>${escapeHtml(control.runtime.encoder_name)} · ${escapeHtml(control.runtime.pipeline_memory_mode.replaceAll("-", " "))}</small>
        <p>${escapeHtml(control.runtime.auto_selection_reason)}</p>
      </section>
      <label class="video-field">ENCODER<select id="videoEncoder">${encoderOptions}</select></label>
      <div class="video-segments" aria-label="Quality preset">
        ${(["fast", "balanced", "sharp"] as const).map((profile) => `<button data-video-profile="${profile}" class="${settings.profile === profile ? "active" : ""}">${profile.charAt(0).toUpperCase()}${profile.slice(1)}</button>`).join("")}
      </div>
      <div class="video-editor">
        <label>MAX WIDTH<input id="videoWidth" type="number" inputmode="numeric" min="320" max="7680" step="2" value="${preset.max_width}"></label>
        <label>FPS<input id="videoFps" type="number" inputmode="numeric" min="1" max="120" value="${preset.max_fps}"></label>
        <label>MBPS<input id="videoBitrate" type="number" inputmode="decimal" min="0.5" max="200" step="0.1" value="${bitrate}"></label>
      </div>
      <div class="video-actions"><button id="videoApply" class="primary-button">Apply live</button><button id="videoReset">Reset preset</button></div>
      <section class="video-benchmark">
        <div><b>AUTO TEST</b><small>${control.learned_results.length} learned result${control.learned_results.length === 1 ? "" : "s"} stored locally</small></div>
        <button id="videoAutoTest" class="primary-button" ${this.videoBenchmarkRunning ? "disabled" : ""}>${this.videoBenchmarkRunning ? "Testing…" : "Run quick test"}</button>
        ${this.videoBenchmarkStatus ? `<p>${escapeHtml(this.videoBenchmarkStatus)}</p>` : ""}
      </section>
      ${learnedDetails ? `<details class="video-results"><summary>Auto test details</summary><ul>${learnedDetails}</ul></details>` : ""}
      <ul class="video-capabilities">${capabilities}</ul>`;
    content.querySelectorAll<HTMLButtonElement>("[data-video-profile]").forEach((button) => {
      button.addEventListener("click", () => {
        const profile = button.dataset.videoProfile as VideoConfig["profile"];
        void this.updateVideoSettings((next) => { next.profile = profile; });
      });
    });
    content.querySelector<HTMLButtonElement>("#videoApply")?.addEventListener("click", () => {
      void this.updateVideoSettings((next) => {
        const requestedMode = this.requiredElement<HTMLSelectElement>("videoEncoder").value as VideoConfig["encoder"];
        next.encoder = requestedMode;
        const selected = next.presets[next.profile];
        selected.max_width = Number(this.requiredElement<HTMLInputElement>("videoWidth").value);
        selected.max_fps = Number(this.requiredElement<HTMLInputElement>("videoFps").value);
        const value = Number(this.requiredElement<HTMLInputElement>("videoBitrate").value);
        const requestedCodec = requestedMode === "hevc-hardware"
          ? "hevc"
          : requestedMode === "av1-hardware"
            ? "av1"
            : requestedMode === "auto" ? activeCodec : "h264";
        if (requestedCodec === "h264") selected.bitrates.h264_mbps = value;
        if (requestedCodec === "hevc") selected.bitrates.hevc_mbps = value;
        if (requestedCodec === "av1") selected.bitrates.av1_mbps = value;
      });
    });
    content.querySelector<HTMLButtonElement>("#videoReset")?.addEventListener("click", () => {
      void this.updateVideoSettings((next) => {
        const defaults = next.profile === "fast"
          ? { max_width: 1280, max_fps: 60, bitrate: 5 }
          : next.profile === "sharp"
            ? { max_width: 2560, max_fps: 60, bitrate: 18 }
            : { max_width: 1920, max_fps: 60, bitrate: 10 };
        next.presets[next.profile] = {
          max_width: defaults.max_width,
          max_fps: defaults.max_fps,
          bitrates: { h264_mbps: defaults.bitrate, hevc_mbps: null, av1_mbps: null },
        };
      });
    });
    content.querySelector<HTMLButtonElement>("#videoAutoTest")?.addEventListener("click", () => {
      void this.runQuickAutoTest();
    });
  }

  private async runQuickAutoTest(): Promise<void> {
    if (this.videoBenchmarkRunning || !this.videoControl || !this.browserVideoCapabilities) {
      return;
    }
    const candidates = this.videoControl.compatibility
      .filter((entry) => entry.availability !== "unavailable")
      .map((entry) => entry.mode);
    if (candidates.length === 0) {
      this.showNotice("No mutually supported encoders are available to test.");
      return;
    }
    this.videoBenchmarkRunning = true;
    const original = JSON.parse(JSON.stringify(this.videoControl.settings.settings)) as VideoConfig;
    try {
      for (let index = 0; index < candidates.length; index += 1) {
        const mode = candidates[index]!;
        this.videoBenchmarkStatus = `${index + 1}/${candidates.length} · ${encoderModeLabel(mode)} · measuring 4 seconds`;
        this.renderVideoPanel();
        await this.activateBenchmarkMode(mode);
        await this.waitForPresentedFrame(10_000);
        // Explicitly await the verification write; the normal playing callback
        // is deliberately fire-and-forget for startup responsiveness.
        this.videoControl = await reportVideoPresented(this.videoControl!.runtime.codec, true, true);
        await delay(1_000);
        const before = await this.benchmarkCounters();
        const durationMs = 4_000;
        await delay(durationMs);
        const after = await this.benchmarkCounters();
        const seconds = durationMs / 1000;
        const encodedFrames = nonnegativeDelta(after.host.encoded_frames, before.host.encoded_frames);
        const encodedBytes = nonnegativeDelta(after.host.encoded_bytes, before.host.encoded_bytes);
        const presentedFrames = nonnegativeDelta(after.totalFrames, before.totalFrames);
        const presentedDrops = nonnegativeDelta(after.droppedFrames, before.droppedFrames);
        const decoderDrops = nonnegativeDelta(
          numeric(after.inbound?.framesDropped),
          numeric(before.inbound?.framesDropped),
        );
        const freezes = nonnegativeDelta(numeric(after.inbound?.freezeCount), numeric(before.inbound?.freezeCount));
        const encoderCapability = this.videoControl!.host_capabilities.find((candidate) =>
          encoderModeForCapability(candidate.codec, candidate.hardware) === mode &&
          (candidate.state === "functional" || candidate.state === "benchmark-tested")
        );
        if (!encoderCapability) {
          throw new Error(`${encoderModeLabel(mode)} disappeared during its test`);
        }
        const observation: AutoBenchmarkObservation = {
          schema_version: 1,
          nfidb_version: __NFIDB_CLIENT_VERSION__,
          receiver_runtime: this.browserVideoCapabilities!.user_agent,
          encoder_id: encoderCapability.id,
          mode,
          profile: original.profile,
          max_width: original.presets[original.profile].max_width,
          requested_fps: original.presets[original.profile].max_fps,
          end_to_end_verified: true,
          recorded_unix_ms: Date.now(),
          metrics: {
            requested_fps: original.presets[original.profile].max_fps,
            encoded_fps: encodedFrames / seconds,
            presented_fps: presentedFrames / seconds,
            encode_mean_ms: after.host.recent_encode_mean_ms,
            encode_p95_ms: after.host.encode_p95_ms,
            preprocess_mean_ms: after.host.recent_preprocess_mean_ms,
            preprocess_p95_ms: after.host.preprocess_p95_ms,
            actual_mbps: (encodedBytes * 8) / durationMs / 1000,
            cpu_percent: after.host.process_cpu_percent,
            working_set_mib: after.host.working_set_mib,
            drop_percent: ((presentedDrops + decoderDrops) / Math.max(1, presentedFrames)) * 100,
            freeze_count: freezes,
            pipeline_p95_ms: this.liveClientDiagnostic?.frameTiming.captureToPresentP95Ms ?? this.liveClientDiagnostic?.frameTiming.estimatedPipelineMs ?? null,
            quality_score: null,
          },
          score: { mode, passed_gates: false, score: null, components: {}, reasons: [] },
        };
        this.videoControl = await recordAutoBenchmark(observation);
      }
      const autoSettings = JSON.parse(JSON.stringify(original)) as VideoConfig;
      autoSettings.encoder = "auto";
      const previousCodec = this.videoControl.runtime.codec;
      this.videoControl = await setVideoSettings(this.videoControl.settings.revision, autoSettings);
      if (this.videoControl.runtime.codec !== previousCodec) {
        await this.connectVideo();
      }
      this.videoBenchmarkStatus = `Auto selected ${encoderModeLabel(this.videoControl.runtime.active_mode)}.`;
      this.showNotice(this.videoBenchmarkStatus);
    } catch (error) {
      this.videoBenchmarkStatus = error instanceof Error ? error.message : String(error);
      this.showNotice(`Auto test stopped: ${this.videoBenchmarkStatus}`);
      try {
        if (this.videoControl) {
          this.videoControl = await setVideoSettings(this.videoControl.settings.revision, original);
          await this.connectVideo();
        }
      } catch {
        // The authenticated WebSocket remains available even if video recovery fails.
      }
    } finally {
      this.videoBenchmarkRunning = false;
      this.renderVideoPanel();
    }
  }

  private async activateBenchmarkMode(mode: EncoderMode): Promise<void> {
    if (!this.videoControl) return;
    const settings = JSON.parse(JSON.stringify(this.videoControl.settings.settings)) as VideoConfig;
    settings.encoder = mode;
    const previousCodec = this.videoControl.runtime.codec;
    this.videoControl = await setVideoSettings(this.videoControl.settings.revision, settings);
    if (this.videoControl.runtime.codec !== previousCodec || this.state !== "connected") {
      await this.connectVideo();
    }
  }

  private async waitForPresentedFrame(timeoutMs: number): Promise<void> {
    const started = performance.now();
    const video = this.requiredElement<HTMLVideoElement>("remoteVideo");
    const initial = video.getVideoPlaybackQuality().totalVideoFrames;
    while (performance.now() - started < timeoutMs) {
      if (this.state === "connected" && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA && video.getVideoPlaybackQuality().totalVideoFrames > initial) {
        return;
      }
      await delay(100);
    }
    throw new Error("Timed out waiting for a decoded frame");
  }

  private async benchmarkCounters(): Promise<{
    host: HostMetrics;
    inbound: Record<string, unknown> | null;
    totalFrames: number;
    droppedFrames: number;
  }> {
    const video = this.requiredElement<HTMLVideoElement>("remoteVideo");
    const quality = video.getVideoPlaybackQuality();
    return {
      host: await getMetrics(),
      inbound: (await this.readRtcStats()).inboundVideo,
      totalFrames: quality.totalVideoFrames,
      droppedFrames: quality.droppedVideoFrames,
    };
  }

  private async updateVideoSettings(change: (settings: VideoControl["settings"]["settings"]) => void): Promise<void> {
    if (!this.videoControl) {
      return;
    }
    const previousCodec = this.videoControl.runtime.codec;
    const settings = JSON.parse(JSON.stringify(this.videoControl.settings.settings)) as VideoControl["settings"]["settings"];
    change(settings);
    this.showNotice("Applying video settings on the host…");
    try {
      this.videoControl = await setVideoSettings(this.videoControl.settings.revision, settings);
      this.renderVideoPanel();
      if (this.videoControl.runtime.codec !== previousCodec) {
        this.showNotice(`Switching to ${encoderModeLabel(this.videoControl.runtime.active_mode)}…`);
        await this.connectVideo();
      } else {
        this.showNotice(`${encoderModeLabel(this.videoControl.runtime.active_mode)} is active.`);
      }
    } catch (error) {
      this.showNotice(error instanceof Error ? error.message : String(error));
      this.renderVideoPanel();
    }
  }

  private startFilePolling(): void {
    window.clearInterval(this.fileRefreshTimer);
    this.knownOutgoingIds = null;
    if (this.status?.file_transfer_enabled === false) {
      return;
    }
    void this.refreshFileListing();
    this.fileRefreshTimer = window.setInterval(() => void this.refreshFileListing(), 1500);
  }

  private stopFilePolling(): void {
    window.clearInterval(this.fileRefreshTimer);
    this.fileRefreshTimer = 0;
    this.fileRefreshActive = false;
    this.knownOutgoingIds = null;
  }

  private async refreshFileListing(): Promise<void> {
    if (this.fileRefreshActive) {
      return;
    }
    this.fileRefreshActive = true;
    try {
      const listing = await getFileListing();
      const nextOutgoingIds = new Set(listing.outbox.map((file) => file.id));
      const addedCount = this.knownOutgoingIds === null
        ? nextOutgoingIds.size
        : Array.from(nextOutgoingIds).filter((id) => !this.knownOutgoingIds?.has(id)).length;
      this.knownOutgoingIds = nextOutgoingIds;
      this.fileListing = listing;
      this.updateFilesBadge();
      this.renderFilesContent();
      if (addedCount > 0) {
        const noun = addedCount === 1 ? "file is" : "files are";
        this.showNotice(`${addedCount} ${noun} ready from the host. Open Files to download.`);
      }
    } catch (error) {
      const content = this.root.querySelector<HTMLElement>("#filesContent");
      if (content) {
        content.innerHTML = `<p class="transfer-error">${escapeHtml(error instanceof Error ? error.message : String(error))}</p>`;
      }
    } finally {
      this.fileRefreshActive = false;
    }
  }

  private updateFilesBadge(): void {
    const badge = this.root.querySelector<HTMLElement>("#filesBadge");
    if (!badge) {
      return;
    }
    const count = this.fileListing?.outbox.length ?? 0;
    badge.textContent = String(count);
    badge.hidden = count === 0;
  }

  private renderFilesContent(): void {
    const content = this.root.querySelector<HTMLElement>("#filesContent");
    if (!content) {
      return;
    }
    const listing = this.fileListing;
    const queued = this.uploadQueue;
    const hostName = this.status?.host_platform === "macos" ? "Mac" : "Windows";
    const uploadRows = queued.length === 0
      ? `<p class="panel-empty">Nothing queued from this iPad.</p>`
      : queued.map((item) => {
          const percent = item.file.size === 0 ? 100 : Math.min(100, item.uploaded / item.file.size * 100);
          const action = item.state === "queued" || item.state === "uploading"
            ? `<button data-cancel-upload="${item.localId}">Cancel</button>`
            : item.state === "failed" || item.state === "canceled"
              ? `<div class="transfer-actions"><button data-retry-upload="${item.localId}">Retry</button><button data-dismiss-upload="${item.localId}">Remove</button></div>`
              : `<button data-dismiss-upload="${item.localId}">Dismiss</button>`;
          const detail = item.message || `${formatBytes(item.uploaded)} / ${formatBytes(item.file.size)}`;
          return `<div class="transfer-row upload-row" data-state="${item.state}">
            <div><b>${escapeHtml(item.file.name)}</b><small>${escapeHtml(detail)}</small></div>${action}
            <i><span style="width:${percent.toFixed(2)}%"></span></i>
          </div>`;
        }).join("");
    const outboxRows = !listing || listing.outbox.length === 0
      ? `<p class="panel-empty">No host files are queued for this iPad.</p>`
      : listing.outbox.map((file) => `<div class="transfer-row">
          <div><b>${escapeHtml(file.name)}</b><small>${formatBytes(file.size)} · ${escapeHtml(file.mime)}${file.sha256 ? ` · SHA-256 ${file.sha256.slice(0, 12)}…` : " · checksum pending"}</small></div>
          <div class="transfer-actions"><a data-download-id="${file.id}" href="${outgoingDownloadUrl(file.id, this.autoClearDownloads)}" download="${escapeHtml(file.name)}">Download</a><button data-remove-outgoing="${file.id}">Remove</button></div>
        </div>`).join("");
    const recentRows = !listing || listing.recent.length === 0
      ? `<p class="panel-empty">No transfers completed in this run.</p>`
      : listing.recent.slice(0, 6).map((transfer) => `<div class="recent-transfer">
          <span>${transfer.direction === "ipad-to-windows" ? `iPad to ${hostName}` : `${hostName} to iPad`}</span>
          <b>${escapeHtml(transfer.name)}</b><small>${formatBytes(transfer.bytes)} · ${escapeHtml(transfer.status)} · ${transfer.average_mbps.toFixed(2)} Mbps</small>
        </div>`).join("");
    const stats = listing?.stats;
    const outboxCount = listing?.outbox.length ?? 0;
    content.innerHTML = `
      <div class="transfer-summary">
        <span>UP ${stats?.upload_mbps.toFixed(2) ?? "0.00"} Mbps</span><span>DOWN ${stats?.download_mbps.toFixed(2) ?? "0.00"} Mbps</span><span>${listing?.rate_limit_mbps ?? 0} Mbps limit</span>
      </div>
      <section class="file-section">
        <div class="file-section-heading"><div><b>SEND TO ${hostName.toUpperCase()}</b><small>${listing ? `Saves to ${escapeHtml(listing.inbox_name)}` : "Verified 1 MiB chunks"}</small></div><button id="chooseFilesButton" class="file-primary" ${listing?.enabled === false ? "disabled" : ""}>Choose files</button></div>
        <input id="filePicker" type="file" multiple hidden />
        ${uploadRows}
      </section>
      <section class="file-section">
        <div class="file-section-heading"><div><b>FROM ${hostName.toUpperCase()}</b><small>Only files queued in the desktop app</small></div>${outboxCount > 1 ? `<button id="downloadAllButton" class="file-secondary">Download all (${outboxCount})</button>` : ""}</div>
        ${outboxRows}
        <label class="file-option"><input id="autoClearDownloads" type="checkbox" ${this.autoClearDownloads ? "checked" : ""} /><span>Clear each queue item after download</span></label>
        <p class="file-option-help">On by default. The host removes an item only after its full file stream is delivered.</p>
      </section>
      <section class="file-section recent-section">
        <div class="file-section-heading"><div><b>RECENT</b><small>Local session history</small></div></div>
        ${recentRows}
      </section>
      <p class="file-safety">Bulk traffic is rate-limited${listing?.pause_while_drawing ? " and pauses while Pencil or touch is down" : ""}. Safari’s download manager shows download progress.</p>`;
    this.bindFileControls();
  }

  private bindFileControls(): void {
    const picker = this.root.querySelector<HTMLInputElement>("#filePicker");
    this.root.querySelector("#chooseFilesButton")?.addEventListener("click", () => picker?.click());
    picker?.addEventListener("change", () => {
      this.queueUploads(Array.from(picker.files ?? []));
      picker.value = "";
    });
    this.root.querySelector<HTMLInputElement>("#autoClearDownloads")?.addEventListener("change", (event) => {
      this.autoClearDownloads = (event.currentTarget as HTMLInputElement).checked;
      saveAutoClearDownloads(this.autoClearDownloads);
      this.renderFilesContent();
    });
    this.root.querySelector<HTMLButtonElement>("#downloadAllButton")?.addEventListener("click", () => {
      const links = Array.from(this.root.querySelectorAll<HTMLAnchorElement>("[data-download-id]"));
      for (const link of links) {
        link.click();
      }
      this.showNotice(`${links.length} downloads started. Safari may ask once for permission to download multiple files.`);
    });
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-cancel-upload]")) {
      button.addEventListener("click", () => this.cancelQueuedUpload(Number(button.dataset.cancelUpload)));
    }
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-retry-upload]")) {
      button.addEventListener("click", () => {
        const item = this.uploadQueue.find((candidate) => candidate.localId === Number(button.dataset.retryUpload));
        if (item) {
          item.uploaded = 0;
          item.retries = 0;
          item.message = "Waiting…";
          item.state = "queued";
          void this.processUploadQueue();
          this.renderFilesContent();
        }
      });
    }
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-dismiss-upload]")) {
      button.addEventListener("click", () => {
        const index = this.uploadQueue.findIndex((candidate) => candidate.localId === Number(button.dataset.dismissUpload));
        if (index >= 0) {
          this.uploadQueue.splice(index, 1);
          this.renderFilesContent();
        }
      });
    }
    for (const button of this.root.querySelectorAll<HTMLButtonElement>("[data-remove-outgoing]")) {
      button.addEventListener("click", async () => {
        const id = button.dataset.removeOutgoing;
        if (!id) {
          return;
        }
        button.disabled = true;
        try {
          await removeOutgoingFile(id);
          await this.refreshFileListing();
        } catch (error) {
          this.showNotice(error instanceof Error ? error.message : String(error));
        }
      });
    }
    for (const link of this.root.querySelectorAll<HTMLAnchorElement>("[data-download-id]")) {
      link.addEventListener("click", () => {
        this.showNotice(this.autoClearDownloads
          ? "Download started. Its host queue item clears after every byte is delivered."
          : "Download handed to Safari. Use Safari’s download button to view progress or save to Files.");
      });
    }
  }

  private queueUploads(files: File[]): void {
    const maximum = this.fileListing?.max_file_size_bytes ?? Number.MAX_SAFE_INTEGER;
    let rejected = 0;
    for (const file of files) {
      if (file.size > maximum) {
        rejected += 1;
        continue;
      }
      this.uploadQueue.push({
        localId: this.nextUploadId++,
        file,
        uploaded: 0,
        state: "queued",
        message: "Waiting…",
        retries: 0,
      });
    }
    if (rejected > 0) {
      this.showNotice(`${rejected} file(s) exceeded the host file-size limit.`);
    }
    this.renderFilesContent();
    void this.processUploadQueue();
  }

  private async processUploadQueue(): Promise<void> {
    if (this.uploadQueueActive) {
      return;
    }
    this.uploadQueueActive = true;
    try {
      for (;;) {
        const item = this.uploadQueue.find((candidate) => candidate.state === "queued");
        if (!item) {
          break;
        }
        item.state = "uploading";
        item.message = "Starting…";
        this.activeUploadId = item.localId;
        this.activeUploadAbort = new AbortController();
        this.renderFilesContent();
        try {
          const complete = await uploadFile(
            item.file,
            {
              onProgress: (uploaded, total) => {
                item.uploaded = uploaded;
                item.message = uploaded >= total
                  ? "Verifying on the host…"
                  : `${formatBytes(uploaded)} / ${formatBytes(total)}`;
                this.renderFilesContent();
              },
              onRetry: (attempt) => {
                item.retries += 1;
                item.message = `Connection retry ${attempt}…`;
                this.renderFilesContent();
              },
            },
            this.activeUploadAbort.signal,
          );
          item.uploaded = item.file.size;
          item.state = "completed";
          item.message = `Saved as ${complete.name} · SHA-256 ${complete.sha256.slice(0, 12)}…`;
        } catch (error) {
          if (error instanceof DOMException && error.name === "AbortError") {
            item.state = "canceled";
            item.message = "Canceled; partial data removed";
          } else {
            item.state = "failed";
            item.message = error instanceof Error ? error.message : String(error);
          }
        } finally {
          this.activeUploadAbort = null;
          this.activeUploadId = 0;
          this.renderFilesContent();
          await this.refreshFileListing();
        }
      }
    } finally {
      this.uploadQueueActive = false;
    }
  }

  private cancelQueuedUpload(localId: number): void {
    const item = this.uploadQueue.find((candidate) => candidate.localId === localId);
    if (!item) {
      return;
    }
    if (this.activeUploadId === localId) {
      this.activeUploadAbort?.abort();
    } else if (item.state === "queued") {
      item.state = "canceled";
      item.message = "Canceled before upload";
      this.renderFilesContent();
    }
  }

  private openControlSocket(): void {
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${scheme}//${location.host}/api/ws`);
    socket.binaryType = "arraybuffer";
    socket.addEventListener("open", () => {
      this.sendPing();
      this.drainPendingInput();
      this.startDiagnosticRecording();
    });
    socket.addEventListener("message", (event) => this.handleControlMessage(event.data));
    socket.addEventListener("close", () => {
      this.stopDiagnosticRecording();
      this.handleInputTransportClose("websocket");
      if (this.state !== "pairing" && this.channel?.readyState !== "open") {
        this.showNotice("Control channel disconnected. Reopen this page to reconnect.");
      }
    });
    this.socket = socket;
  }

  private sendInput(packet: ArrayBuffer): void {
    if (this.inputTransport === "datachannel" && this.channel?.readyState === "open") {
      this.channel.send(packet);
      return;
    }
    if (this.inputTransport === "websocket" && this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(packet);
      return;
    }
    if (this.inputTransport === null) {
      if (this.channel?.readyState === "open") {
        this.inputTransport = "datachannel";
        this.channel.send(packet);
        return;
      }
      if (this.socket?.readyState === WebSocket.OPEN) {
        this.inputTransport = "websocket";
        this.socket.send(packet);
        return;
      }
      if (this.pendingInput.length < 16_384) {
        this.pendingInput.push(packet);
      } else {
        this.pendingInput.length = 0;
        this.pointerEngine?.abandonAll();
        this.remoteInputEngine?.abandonAll();
        this.showNotice("Input transport is unavailable. Release all input and reconnect before continuing.");
      }
    }
  }

  private drainPendingInput(): void {
    if (this.pendingInput.length === 0 || this.inputTransport !== null) {
      return;
    }
    if (this.channel?.readyState === "open") {
      this.inputTransport = "datachannel";
      for (const packet of this.pendingInput.splice(0)) {
        this.channel.send(packet);
      }
    } else if (this.socket?.readyState === WebSocket.OPEN) {
      this.inputTransport = "websocket";
      for (const packet of this.pendingInput.splice(0)) {
        this.socket.send(packet);
      }
    }
  }

  private handleInputTransportClose(transport: InputTransport): void {
    if (this.inputTransport !== transport) {
      return;
    }
    this.inputTransport = null;
    this.pendingInput.length = 0;
    this.pointerEngine?.abandonAll();
    this.remoteInputEngine?.abandonAll();
    this.showNotice("Input transport changed. Release Pencil, mouse buttons, and keys before continuing.");
  }

  private handleControlMessage(data: unknown): void {
    if (typeof data !== "string") {
      return;
    }
    try {
      const message = JSON.parse(data) as { type?: string; t0?: number; t1?: number; t2?: number; stats?: HostMetrics };
      if (message.type === "stats" && message.stats) {
        this.metrics = message.stats;
        this.renderStats();
      } else if (
        message.type === "pong" &&
        typeof message.t0 === "number" &&
        typeof message.t1 === "number" &&
        typeof message.t2 === "number"
      ) {
        const t3 = Date.now();
        const rtt = Math.max(0, t3 - message.t0 - (message.t2 - message.t1));
        this.rttMs = rtt;
        this.clockOffsetMs = (message.t1 - message.t0 + (message.t2 - t3)) / 2;
        if (this.metrics) {
          this.metrics.rtt_ms = rtt;
          this.metrics.client_clock_offset_ms = this.clockOffsetMs;
        }
        window.setTimeout(() => this.sendPing(), 2000);
      }
    } catch {
      // Ignore malformed diagnostic messages; pointer transport is independent.
    }
  }

  private sendPing(): void {
    if (this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify({ type: "ping", t0: Date.now() }));
    }
  }

  private startDiagnosticRecording(): void {
    this.stopDiagnosticRecording();
    this.previousRtcCounters = null;
    this.diagnosticSequence = 0;
    this.diagnosticFailures = 0;
    void this.captureClientDiagnostic();
    this.diagnosticTimer = window.setInterval(() => void this.captureClientDiagnostic(), 1000);
  }

  private stopDiagnosticRecording(): void {
    window.clearInterval(this.diagnosticTimer);
    this.diagnosticTimer = 0;
  }

  private async captureClientDiagnostic(): Promise<void> {
    if (
      this.diagnosticCollectionActive ||
      this.socket?.readyState !== WebSocket.OPEN ||
      this.socket.bufferedAmount > 256 * 1024
    ) {
      return;
    }
    this.diagnosticCollectionActive = true;
    try {
      await this.refreshInputControl();
      const rtc = await this.readRtcStats();
      const video = this.root.querySelector<HTMLVideoElement>("#remoteVideo");
      const quality =
        video && typeof video.getVideoPlaybackQuality === "function" ? video.getVideoPlaybackQuality() : undefined;
      const inbound = rtc.inboundVideo;
      const pair = rtc.candidatePair;
      const nowMs = performance.now();
      const current: PreviousRtcCounters = {
        atMs: nowMs,
        bytesReceived: numeric(inbound?.bytesReceived),
        framesDecoded: numeric(inbound?.framesDecoded),
        packetsLost: numeric(inbound?.packetsLost),
        jitterBufferDelay: numeric(inbound?.jitterBufferDelay),
        jitterBufferEmittedCount: numeric(inbound?.jitterBufferEmittedCount),
        totalDecodeTime: numeric(inbound?.totalDecodeTime),
        totalVideoFrames: quality?.totalVideoFrames ?? 0,
        droppedVideoFrames: quality?.droppedVideoFrames ?? 0,
      };
      const previous = this.previousRtcCounters;
      const intervalMs = Math.max(1, previous ? nowMs - previous.atMs : 1000);
      const intervalSeconds = intervalMs / 1000;
      const bytesDelta = nonnegativeDelta(current.bytesReceived, previous?.bytesReceived);
      const decodedDelta = nonnegativeDelta(current.framesDecoded, previous?.framesDecoded);
      const playbackDelta = nonnegativeDelta(current.totalVideoFrames, previous?.totalVideoFrames);
      const playbackDropDelta = nonnegativeDelta(current.droppedVideoFrames, previous?.droppedVideoFrames);
      const jitterDelayDelta = nonnegativeDelta(current.jitterBufferDelay, previous?.jitterBufferDelay);
      const jitterCountDelta = nonnegativeDelta(
        current.jitterBufferEmittedCount,
        previous?.jitterBufferEmittedCount,
      );
      const decodeTimeDelta = nonnegativeDelta(current.totalDecodeTime, previous?.totalDecodeTime);
      const lossDelta = current.packetsLost - (previous?.packetsLost ?? current.packetsLost);
      const frameGapValues = this.frameGapsMs.splice(0);
      const captureValues = this.captureToPresentMs.splice(0);
      const receiveValues = this.receiveToPresentMs.splice(0);
      const processingValues = this.frameProcessingMs.splice(0);
      const frameGapP95Ms = percentile(frameGapValues, 0.95);
      const captureToPresentP95Ms = optionalPercentile(captureValues, 0.95);
      const receiveToPresentP95Ms = optionalPercentile(receiveValues, 0.95);
      const decodeMsPerFrame = decodedDelta > 0 ? (decodeTimeDelta * 1000) / decodedDelta : 0;
      const jitterBufferMsPerFrame = jitterCountDelta > 0 ? (jitterDelayDelta * 1000) / jitterCountDelta : 0;
      const receiveMbps = (bytesDelta * 8) / intervalMs / 1000;
      const availableIncomingMbps = numeric(pair?.availableIncomingBitrate) / 1_000_000;
      const estimatedPipelineMs =
        captureToPresentP95Ms ??
        (this.metrics?.preprocess_ms ?? 0) +
          (this.metrics?.encode_ms ?? 0) +
          this.rttMs / 2 +
          jitterBufferMsPerFrame +
          decodeMsPerFrame +
          (receiveToPresentP95Ms ?? 0);
      const playbackFps = playbackDelta / intervalSeconds;
      const decodeFps = decodedDelta / intervalSeconds;
      const presentationDropPercent = (playbackDropDelta / Math.max(1, playbackDelta)) * 100;
      const sample = {
        sequence: this.diagnosticSequence++,
        clientEpochMs: Date.now(),
        sampleIntervalMs: intervalMs,
        device: {
          clientVersion: __NFIDB_CLIENT_VERSION__,
          userAgent: navigator.userAgent,
          platform: navigator.platform,
          maxTouchPoints: navigator.maxTouchPoints,
          viewportWidth: window.innerWidth,
          viewportHeight: window.innerHeight,
          screenWidth: screen.width,
          screenHeight: screen.height,
          devicePixelRatio: window.devicePixelRatio,
          orientation: screen.orientation?.type ?? "unknown",
          visibilityState: document.visibilityState,
        },
        connection: {
          appState: this.state,
          peerConnectionState: this.peer?.connectionState ?? "none",
          iceConnectionState: this.peer?.iceConnectionState ?? "none",
          iceGatheringState: this.peer?.iceGatheringState ?? "none",
          signalingState: this.peer?.signalingState ?? "none",
          inputTransport: this.inputTransport ?? "pending",
        },
        video: {
          width: video?.videoWidth ?? 0,
          height: video?.videoHeight ?? 0,
          readyState: video?.readyState ?? 0,
          currentTimeSeconds: video?.currentTime ?? 0,
          totalFrames: current.totalVideoFrames,
          droppedFrames: current.droppedVideoFrames,
          framesReceived: numeric(inbound?.framesReceived),
          framesDecoded: current.framesDecoded,
          decoderDroppedFrames: numeric(inbound?.framesDropped),
          decodeFps,
          playbackFps,
          presentationDropPercent,
          decodeMsPerFrame,
          jitterBufferMsPerFrame,
          freezeCount: numeric(inbound?.freezeCount),
          totalFreezeSeconds: numeric(inbound?.totalFreezesDuration),
          startupMs:
            this.firstVideoFrameAtMs > 0 && this.videoTrackAtMs > 0
              ? this.firstVideoFrameAtMs - this.videoTrackAtMs
              : null,
        },
        network: {
          rttMs: this.rttMs,
          clockOffsetMs: this.clockOffsetMs,
          oneWayEstimateMs: this.rttMs / 2,
          receiveMbps,
          availableIncomingMbps,
          bytesReceived: current.bytesReceived,
          packetsReceived: numeric(inbound?.packetsReceived),
          packetsLost: current.packetsLost,
          packetLossDelta: lossDelta,
          jitterMs: numeric(inbound?.jitter) * 1000,
          candidateType: String(rtc.remoteCandidate?.candidateType ?? "unknown"),
          protocol: String(rtc.remoteCandidate?.protocol ?? "unknown"),
        },
        frameTiming: {
          callbackCount: this.frameCallbackCount,
          frameGapP50Ms: percentile(frameGapValues, 0.5),
          frameGapP95Ms,
          frameGapP99Ms: percentile(frameGapValues, 0.99),
          frameGapMaxMs: maximum(frameGapValues),
          captureToPresentP50Ms: optionalPercentile(captureValues, 0.5),
          captureToPresentP95Ms,
          captureToPresentP99Ms: optionalPercentile(captureValues, 0.99),
          receiveToPresentP95Ms,
          processingP95Ms: optionalPercentile(processingValues, 0.95),
          estimatedPipelineMs,
        },
        buffers: {
          dataChannelBytes: this.channel?.bufferedAmount ?? 0,
          webSocketBytes: this.socket?.bufferedAmount ?? 0,
        },
        rawRtc: {
          inboundVideo: rtc.inboundVideo,
          candidatePair: rtc.candidatePair,
          localCandidate: rtc.localCandidate,
          remoteCandidate: rtc.remoteCandidate,
        },
      };
      this.previousRtcCounters = current;
      this.liveClientDiagnostic = {
        video: {
          decodeFps,
          playbackFps,
          presentationDropPercent,
          decodeMsPerFrame,
          jitterBufferMsPerFrame,
        },
        network: {
          rttMs: this.rttMs,
          receiveMbps,
          availableIncomingMbps,
          packetsLost: current.packetsLost,
          jitterMs: numeric(inbound?.jitter) * 1000,
        },
        frameTiming: {
          frameGapP95Ms,
          frameGapMaxMs: maximum(frameGapValues),
          captureToPresentP95Ms,
          estimatedPipelineMs,
        },
      };
      this.socket.send(JSON.stringify({ type: "client-diagnostics", sample }));
      this.renderStats();
    } catch (error) {
      this.diagnosticFailures += 1;
      console.debug("NFiDB diagnostic sample failed", error);
      this.sendFallbackDiagnostic(error);
    } finally {
      this.diagnosticCollectionActive = false;
    }
  }

  private sendFallbackDiagnostic(error: unknown): void {
    if (this.socket?.readyState !== WebSocket.OPEN) {
      return;
    }
    const video = this.root.querySelector<HTMLVideoElement>("#remoteVideo");
    const sample = {
      sequence: this.diagnosticSequence++,
      clientEpochMs: Date.now(),
      sampleIntervalMs: 0,
      device: {
        clientVersion: __NFIDB_CLIENT_VERSION__,
        userAgent: navigator.userAgent,
        platform: navigator.platform,
        maxTouchPoints: navigator.maxTouchPoints,
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
        screenWidth: screen.width,
        screenHeight: screen.height,
        devicePixelRatio: window.devicePixelRatio,
        visibilityState: document.visibilityState,
        diagnosticFallback: true,
        diagnosticError: String(error).slice(0, 512),
      },
      connection: {
        appState: this.state,
        peerConnectionState: this.peer?.connectionState ?? "none",
        iceConnectionState: this.peer?.iceConnectionState ?? "none",
        inputTransport: this.inputTransport ?? "pending",
      },
      video: {
        width: video?.videoWidth ?? 0,
        height: video?.videoHeight ?? 0,
        readyState: video?.readyState ?? 0,
        currentTimeSeconds: video?.currentTime ?? 0,
      },
      network: {
        rttMs: this.rttMs,
        clockOffsetMs: this.clockOffsetMs,
        oneWayEstimateMs: this.rttMs / 2,
      },
      frameTiming: {
        callbackCount: this.frameCallbackCount,
      },
      buffers: {
        dataChannelBytes: this.channel?.bufferedAmount ?? 0,
        webSocketBytes: this.socket.bufferedAmount,
      },
      rawRtc: {},
    };
    try {
      this.socket.send(JSON.stringify({ type: "client-diagnostics", sample }));
      this.renderStats();
    } catch (sendError) {
      console.debug("NFiDB fallback diagnostic sample failed", sendError);
    }
  }

  private async readRtcStats(): Promise<RtcDiagnosticStats> {
    let inboundVideo: Record<string, unknown> | null = null;
    let candidatePair: Record<string, unknown> | null = null;
    let localCandidate: Record<string, unknown> | null = null;
    let remoteCandidate: Record<string, unknown> | null = null;
    if (!this.peer) {
      return { inboundVideo, candidatePair, localCandidate, remoteCandidate };
    }
    try {
      const reports = await this.peer.getStats();
      const records = new Map<string, Record<string, unknown>>();
      reports.forEach((report) => records.set(report.id, report as unknown as Record<string, unknown>));
      for (const record of records.values()) {
        if (record.type === "inbound-rtp" && (record.kind === "video" || record.mediaType === "video")) {
          inboundVideo = pickStats(record, INBOUND_VIDEO_STAT_KEYS);
        } else if (record.type === "candidate-pair" && record.state === "succeeded" && record.nominated === true) {
          candidatePair = pickStats(record, CANDIDATE_PAIR_STAT_KEYS);
          const local = records.get(String(record.localCandidateId ?? ""));
          const remote = records.get(String(record.remoteCandidateId ?? ""));
          localCandidate = local ? pickStats(local, CANDIDATE_STAT_KEYS) : null;
          remoteCandidate = remote ? pickStats(remote, CANDIDATE_STAT_KEYS) : null;
        }
      }
    } catch (error) {
      // Safari versions expose different subsets of the RTC stats surface.
      // A missing/failed report must not stop the one-second host recorder.
      console.debug("NFiDB RTC stats unavailable; recording fallback metrics", error);
    }
    return { inboundVideo, candidatePair, localCandidate, remoteCandidate };
  }

  private updatePeerState(): void {
    const state = this.peer?.connectionState;
    if (state === "connected") {
      this.state = "connected";
    } else if (state === "failed" || state === "closed") {
      this.state = "input-only";
      this.pointerEngine?.cancelAll();
      this.remoteInputEngine?.resetRemoteInput();
    }
    this.updateHud();
  }

  private updateHud(): void {
    const label = this.root.querySelector<HTMLElement>("#connectionState");
    if (!label) {
      return;
    }
    label.textContent =
      this.state === "connected"
        ? "Connected locally"
        : this.state === "input-only"
          ? "Input only"
          : this.state === "connecting"
            ? "Connecting"
            : "Disconnected";
    label.dataset.state = this.state;
  }

  private renderStats(): void {
    if (!this.metrics) {
      return;
    }
    const metrics = this.metrics;
    this.setText("videoStats", `${metrics.output_width}×${metrics.output_height} from ${metrics.source_width}×${metrics.source_height} · ${metrics.encoded_fps.toFixed(0)} fps · ${metrics.preprocess_ms.toFixed(1)} + ${metrics.encode_ms.toFixed(1)} ms`);
    const playbackStartup = this.firstVideoFrameAtMs > 0 && this.videoTrackAtMs > 0
      ? `${(this.firstVideoFrameAtMs - this.videoTrackAtMs).toFixed(0)} ms playback`
      : "playback pending";
    const client = this.liveClientDiagnostic;
    this.setText(
      "networkStats",
      `${(client?.network.rttMs ?? metrics.rtt_ms).toFixed(1)} ms RTT · ${(client?.network.receiveMbps ?? 0).toFixed(2)} Mbps receive · ${(client?.network.jitterMs ?? 0).toFixed(2)} ms jitter · ${client?.network.packetsLost ?? 0} lost`,
    );
    this.setText(
      "playoutStats",
      `${(client?.video.decodeFps ?? 0).toFixed(1)} decode / ${(client?.video.playbackFps ?? 0).toFixed(1)} present fps · ${(client?.video.jitterBufferMsPerFrame ?? 0).toFixed(2)} ms buffer · ${(client?.video.decodeMsPerFrame ?? 0).toFixed(2)} ms decode`,
    );
    this.setText(
      "pipelineStats",
      `${playbackStartup} · ${metrics.video_startup_wait_ms.toFixed(0)} ms IDR · ${metrics.video_recovery_requests} recovery requests · ${(client?.frameTiming.captureToPresentP95Ms ?? client?.frameTiming.estimatedPipelineMs ?? 0).toFixed(1)} ms p95 measured/estimated · ${(client?.frameTiming.frameGapP95Ms ?? 0).toFixed(1)} ms frame-gap p95`,
    );
    this.setText("pencilStats", `${metrics.input_samples_per_sec.toFixed(0)} samples/s · ${metrics.input_arrival_ms.toFixed(2)} ms arrival estimate · ${metrics.input_inject_ms.toFixed(3)} ms inject · ${metrics.input_samples} total`);
    this.setText("pressureStats", `${metrics.pressure.toFixed(2)} · ${metrics.tilt_x.toFixed(0)}° / ${metrics.tilt_y.toFixed(0)}°`);
    this.setText(
      "remoteInputStats",
      `${metrics.mouse_samples} mouse · ${metrics.wheel_events} wheel · ${metrics.keyboard_events} keys · ${metrics.text_events} text · ${metrics.command_events} gestures`,
    );
    this.setText(
      "integrityStats",
      `${metrics.sample_sequence_gaps} input gaps · ${metrics.input_errors} input errors · ${metrics.video_transport_drops} video transport skips · ${metrics.dropped_frames} stale captures`,
    );
    this.setText(
      "recorderStats",
      `client ${__NFIDB_CLIENT_VERSION__} · ${this.diagnosticSequence} samples · ${this.diagnosticFailures} fallbacks · 1 Hz · ${formatBytes(metrics.encoded_bytes)} encoded`,
    );
  }

  private setText(id: string, text: string): void {
    const element = this.root.querySelector<HTMLElement>(`#${id}`);
    if (element) {
      element.textContent = text;
    }
  }

  private showNotice(message: string): void {
    const notice = this.root.querySelector<HTMLElement>("#connectionNotice");
    if (!notice) {
      return;
    }
    notice.textContent = message;
    notice.hidden = false;
    window.setTimeout(() => {
      notice.hidden = true;
    }, 7000);
  }

  private showToolbar(): void {
    this.requiredElement("toolbar").classList.add("visible");
    this.requiredElement("toolbarReveal").setAttribute("aria-expanded", "true");
    this.scheduleToolbarHide();
  }

  private hideToolbar(): void {
    window.clearTimeout(this.hideToolbarTimer);
    this.root.querySelector("#toolbar")?.classList.remove("visible");
    this.root.querySelector("#toolbarReveal")?.setAttribute("aria-expanded", "false");
  }

  private scheduleToolbarHide(): void {
    window.clearTimeout(this.hideToolbarTimer);
    if (!this.requiredElement("toolbar").classList.contains("visible")) {
      return;
    }
    this.hideToolbarTimer = window.setTimeout(() => {
      if (
        this.root.querySelector<HTMLElement>("#statsPanel")?.hidden !== false &&
        this.root.querySelector<HTMLElement>("#keyboardPanel")?.hidden !== false &&
        this.root.querySelector<HTMLElement>("#filesPanel")?.hidden !== false &&
        this.root.querySelector<HTMLElement>("#videoPanel")?.hidden !== false
      ) {
        this.hideToolbar();
      }
    }, 2400);
  }

  private async disconnect(): Promise<void> {
    this.stopFilePolling();
    this.activeUploadAbort?.abort();
    this.activeUploadAbort = null;
    this.stopDiagnosticRecording();
    this.stopVideoRecovery();
    this.pointerEngine?.cancelAll();
    this.pointerEngine?.dispose();
    this.remoteInputEngine?.resetRemoteInput();
    this.remoteInputEngine?.dispose();
    this.channel?.close();
    this.peer?.close();
    this.socket?.close();
    this.inputTransport = null;
    this.videoTrackAtMs = 0;
    this.firstVideoFrameAtMs = 0;
    const video = this.root.querySelector<HTMLVideoElement>("#remoteVideo");
    if (video && this.videoFrameRequest > 0 && "cancelVideoFrameCallback" in video) {
      video.cancelVideoFrameCallback(this.videoFrameRequest);
    }
    this.videoFrameRequest = 0;
    this.previousRtcCounters = null;
    this.liveClientDiagnostic = null;
    this.pendingInput.length = 0;
    try {
      await disconnect(this.token);
    } catch {
      // Local teardown still completes when the host disappeared.
    }
    this.token = "";
    this.status = await getStatus().catch(() => this.status);
    this.renderPairing();
  }

  async diagnosticSnapshot(): Promise<ClientDiagnosticSnapshot> {
    const video = this.root.querySelector<HTMLVideoElement>("#remoteVideo");
    const quality = video?.getVideoPlaybackQuality();
    const rtc = await this.readRtcStats();
    return {
      connectionState: this.state,
      peerConnectionState: this.peer?.connectionState ?? "none",
      inputTransport: this.inputTransport ?? "pending",
      dataChannelBufferedBytes: this.channel?.bufferedAmount ?? 0,
      webSocketBufferedBytes: this.socket?.bufferedAmount ?? 0,
      video: {
        width: video?.videoWidth ?? 0,
        height: video?.videoHeight ?? 0,
        readyState: video?.readyState ?? 0,
        currentTime: video?.currentTime ?? 0,
        totalFrames: quality?.totalVideoFrames ?? 0,
        droppedFrames: quality?.droppedVideoFrames ?? 0,
        startupMs:
          this.firstVideoFrameAtMs > 0 && this.videoTrackAtMs > 0
            ? this.firstVideoFrameAtMs - this.videoTrackAtMs
            : null,
      },
      inboundVideo: rtc.inboundVideo,
      candidatePair: rtc.candidatePair,
      host: await getMetrics(),
      hostDiagnostics: await getDiagnosticSummary(),
    };
  }

  private renderFatal(message: string): void {
    this.state = "failed";
    this.root.innerHTML = `
      <main class="pairing-shell"><section class="pair-panel error-panel">
        <div class="wordmark"><span>NFi</span>DB</div><p class="eyebrow">HOST NOT REACHABLE</p>
        <h1>Couldn’t reach the NFiDB host</h1><p>${escapeHtml(message)}</p>
        <button class="primary-button" id="retryButton">Try again</button>
        <p class="pair-help">Confirm both devices are on the same Wi-Fi without guest isolation.${this.status?.host_platform === "windows" ? " Check Windows Firewall Private network access." : ""}</p>
      </section></main>`;
    this.requiredElement("retryButton").addEventListener("click", () => void this.start());
  }

  private requiredElement<T extends HTMLElement = HTMLElement>(id: string): T {
    const element = this.root.querySelector<T>(`#${id}`);
    if (!element) {
      throw new Error(`Required UI element #${id} is missing`);
    }
    return element;
  }
}

function detectBrowserVideoCapabilities(): BrowserVideoCapabilities {
  const receiver = typeof RTCRtpReceiver !== "undefined" && "getCapabilities" in RTCRtpReceiver
    ? RTCRtpReceiver.getCapabilities("video")
    : null;
  const codecs = receiver?.codecs ?? [];
  const collect = (matches: (mime: string) => boolean): BrowserCodecCapability => {
    const mimeTypes = [...new Set(codecs.map((codec) => codec.mimeType).filter((mime) => matches(mime.toLowerCase())))];
    return {
      reported: mimeTypes.length > 0,
      included_in_sdp: false,
      negotiated: false,
      first_keyframe_received: false,
      presented: false,
      mime_types: mimeTypes,
      failure_reason: null,
    };
  };
  return {
    user_agent: navigator.userAgent.slice(0, 1024),
    set_codec_preferences: "setCodecPreferences" in RTCRtpTransceiver.prototype,
    h264: collect((mime) => mime === "video/h264"),
    hevc: collect((mime) => mime === "video/h265" || mime === "video/hevc"),
    av1: collect((mime) => mime === "video/av1" || mime === "video/av01"),
  };
}

function applyCodecPreference(transceiver: RTCRtpTransceiver, codec: VideoCodec): void {
  if (!("setCodecPreferences" in transceiver)) {
    return;
  }
  const capabilities = RTCRtpReceiver.getCapabilities("video")?.codecs ?? [];
  const target = capabilities.filter((candidate) => codecMatches(candidate.mimeType, codec));
  if (target.length === 0) {
    return;
  }
  // Keep matching RTX/RED/FEC entries after the primary codec when Safari
  // reports them; setCodecPreferences rejects an auxiliary-only list.
  const auxiliaries = capabilities.filter((candidate) => /\/(rtx|red|ulpfec)$/i.test(candidate.mimeType));
  try {
    transceiver.setCodecPreferences([...target, ...auxiliaries]);
  } catch (error) {
    console.debug("NFiDB could not set an explicit codec preference", error);
  }
}

function updateSdpCapabilityEvidence(capabilities: BrowserVideoCapabilities, sdp: string): void {
  capabilities.h264.included_in_sdp = /a=rtpmap:\d+ H264\//i.test(sdp);
  capabilities.hevc.included_in_sdp = /a=rtpmap:\d+ (H265|HEVC)\//i.test(sdp);
  capabilities.av1.included_in_sdp = /a=rtpmap:\d+ (AV1|AV01)\//i.test(sdp);
}

function codecMatches(mimeType: string, codec: VideoCodec): boolean {
  const mime = mimeType.toLowerCase();
  if (codec === "h264") return mime === "video/h264";
  if (codec === "hevc") return mime === "video/h265" || mime === "video/hevc";
  return mime === "video/av1" || mime === "video/av01";
}

function encoderModeLabel(mode: VideoControl["runtime"]["active_mode"]): string {
  if (mode === "auto") return "Auto";
  if (mode === "h264-hardware") return "H.264 Hardware";
  if (mode === "hevc-hardware") return "HEVC Hardware";
  if (mode === "av1-hardware") return "AV1 Hardware";
  return "H.264 Software";
}

function encoderModeForCapability(codec: VideoCodec, hardware: boolean): EncoderMode {
  if (!hardware) return "h264-software";
  if (codec === "h264") return "h264-hardware";
  if (codec === "hevc") return "hevc-hardware";
  return "av1-hardware";
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

async function waitForIceGathering(peer: RTCPeerConnection, timeoutMs: number): Promise<void> {
  if (peer.iceGatheringState === "complete") {
    return;
  }
  await new Promise<void>((resolve) => {
    const timeout = window.setTimeout(resolve, timeoutMs);
    const listener = () => {
      if (peer.iceGatheringState === "complete") {
        window.clearTimeout(timeout);
        peer.removeEventListener("icegatheringstatechange", listener);
        resolve();
      }
    };
    peer.addEventListener("icegatheringstatechange", listener);
  });
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes.toFixed(0)} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function escapeHtml(value: string): string {
  const element = document.createElement("span");
  element.textContent = value;
  return element.innerHTML;
}

function pickStats(source: Record<string, unknown>, keys: readonly string[]): Record<string, unknown> {
  return Object.fromEntries(keys.filter((key) => source[key] !== undefined).map((key) => [key, source[key]]));
}

function numeric(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function nonnegativeDelta(current: number, previous: number | undefined): number {
  return previous === undefined ? 0 : Math.max(0, current - previous);
}

function percentile(values: readonly number[], fraction: number): number {
  if (values.length === 0) {
    return 0;
  }
  const ordered = [...values].filter(Number.isFinite).sort((left, right) => left - right);
  if (ordered.length === 0) {
    return 0;
  }
  return ordered[Math.round((ordered.length - 1) * fraction)] ?? 0;
}

function optionalPercentile(values: readonly number[], fraction: number): number | null {
  return values.length > 0 ? percentile(values, fraction) : null;
}

function maximum(values: readonly number[]): number {
  return values.length > 0 ? Math.max(...values) : 0;
}

function pushBounded(values: number[], value: number, limit: number): void {
  if (!Number.isFinite(value)) {
    return;
  }
  values.push(value);
  if (values.length > limit) {
    values.splice(0, values.length - limit);
  }
}

function pushValidDuration(values: number[], value: number, limit: number): void {
  if (value >= 0 && value <= 10_000) {
    pushBounded(values, value, limit);
  }
}

const INBOUND_VIDEO_STAT_KEYS = [
  "timestamp",
  "ssrc",
  "kind",
  "transportId",
  "codecId",
  "packetsReceived",
  "packetsLost",
  "jitter",
  "bytesReceived",
  "headerBytesReceived",
  "lastPacketReceivedTimestamp",
  "framesReceived",
  "framesDecoded",
  "framesDropped",
  "framesPerSecond",
  "frameWidth",
  "frameHeight",
  "keyFramesDecoded",
  "totalDecodeTime",
  "totalInterFrameDelay",
  "totalSquaredInterFrameDelay",
  "jitterBufferDelay",
  "jitterBufferTargetDelay",
  "jitterBufferMinimumDelay",
  "jitterBufferEmittedCount",
  "freezeCount",
  "pauseCount",
  "totalFreezesDuration",
  "totalPausesDuration",
  "nackCount",
  "firCount",
  "pliCount",
  "qpSum",
] as const;

const CANDIDATE_PAIR_STAT_KEYS = [
  "timestamp",
  "state",
  "nominated",
  "localCandidateId",
  "remoteCandidateId",
  "packetsSent",
  "packetsReceived",
  "bytesSent",
  "bytesReceived",
  "lastPacketSentTimestamp",
  "lastPacketReceivedTimestamp",
  "totalRoundTripTime",
  "currentRoundTripTime",
  "availableOutgoingBitrate",
  "availableIncomingBitrate",
  "requestsReceived",
  "requestsSent",
  "responsesReceived",
  "responsesSent",
  "consentRequestsSent",
] as const;

const CANDIDATE_STAT_KEYS = [
  "timestamp",
  "candidateType",
  "protocol",
  "networkType",
  "tcpType",
  "relayProtocol",
  "port",
  "vpn",
] as const;
