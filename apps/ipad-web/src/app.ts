import { disconnect, getDiagnosticSummary, getMetrics, getStatus, pairWithPin, pairWithQr, sendOffer, type HostDiagnosticSummary, type HostMetrics, type HostStatus } from "./api";
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
    const host = escapeHtml(this.status?.host_name ?? "Windows PC");
    this.root.innerHTML = `
      <main class="pairing-shell">
        <section class="pair-panel" aria-labelledby="pair-title">
          <div class="brand-lockup"><div class="wordmark"><span>NFi</span>DB</div><p>No Frills iPad Drawing Bridge</p></div>
          <div class="host-found"><i></i><span>${host} found on your local network</span></div>
          <h1 id="pair-title">Enter the PIN shown on Windows</h1>
          <form id="pairForm" novalidate>
            <label for="pin">Six-digit PIN</label>
            <input id="pin" name="pin" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000 000" aria-describedby="pinHint pairError" autofocus />
            <p id="pinHint" class="pin-hint">Checks automatically after the sixth digit.</p>
            <p id="pairError" class="form-error" role="alert">${escapeHtml(error)}</p>
            <button class="primary-button" type="submit">Connect locally</button>
          </form>
          <div class="privacy-line"><span>NO CLOUD</span><span>NO ACCOUNT</span><span>NO APP</span></div>
          <p class="pair-help">Both devices must be on the same non-isolated Wi-Fi. This page talks directly to your PC.</p>
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
    this.renderSurface();
    this.openControlSocket();
    try {
      const peer = new RTCPeerConnection({ iceServers: [] });
      this.peer = peer;
      const channel = peer.createDataChannel("input", { ordered: true });
      channel.binaryType = "arraybuffer";
      this.channel = channel;
      channel.addEventListener("open", () => this.drainPendingInput());
      channel.addEventListener("close", () => this.handleInputTransportClose("datachannel"));
      peer.addTransceiver("video", { direction: "recvonly" });
      peer.addEventListener("track", (event) => {
        const video = this.requiredElement<HTMLVideoElement>("remoteVideo");
        this.videoTrackAtMs = performance.now();
        this.firstVideoFrameAtMs = 0;
        video.srcObject = event.streams[0] ?? new MediaStream([event.track]);
        this.startVideoFrameTelemetry(video);
        video.addEventListener("playing", () => this.markFirstVideoFrame(), { once: true });
        const startPlayback = () => void video.play().catch(() => undefined);
        video.addEventListener("loadedmetadata", startPlayback, { once: true });
        startPlayback();
        window.setTimeout(() => {
          if (this.peer === peer && peer.connectionState === "connected" && this.firstVideoFrameAtMs === 0) {
            this.showNotice("Video is connected but still waiting for its first decodable frame.");
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
      const answer = await sendOffer(this.token, peer.localDescription.toJSON());
      await peer.setRemoteDescription(answer);
      this.state = "connected";
      this.updateHud();
    } catch (error) {
      console.error(error);
      this.state = "input-only";
      this.showNotice("Video negotiation failed. Pencil input remains available over the diagnostic channel.");
      this.updateHud();
    }
  }

  private renderSurface(): void {
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
            <button id="statsButton" class="tool-button" aria-pressed="false">Stats</button>
            <button id="fullscreenButton" class="icon-button" aria-label="Fullscreen">⛶</button>
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
        <section id="keyboardPanel" class="keyboard-panel" hidden aria-label="Remote keyboard">
          <div class="keyboard-panel-header">
            <div><b>TYPE ON WINDOWS</b><small>Option = Alt · Control = Ctrl · Return = Enter · Delete = Backspace</small></div>
            <button id="keyboardClose" class="icon-button" aria-label="Close keyboard">×</button>
          </div>
          <textarea id="remoteTextInput" rows="2" inputmode="text" enterkeyhint="enter" autocomplete="off" autocapitalize="none" spellcheck="false" placeholder="Tap here to use the iPad keyboard…"></textarea>
          <div class="remote-key-row" aria-label="Special keys">
            <button data-remote-key="Escape" data-key-label="Escape">Esc</button>
            <button data-remote-key="Tab" data-key-label="Tab">Tab</button>
            <button data-remote-key="Backspace" data-key-label="Backspace">⌫</button>
            <button data-remote-key="Enter" data-key-label="Enter">Enter</button>
            <button data-remote-command="${RemoteCommand.AppPrevious}">Alt+Shift+Tab</button>
            <button data-remote-command="${RemoteCommand.AppNext}">Alt+Tab</button>
            <button data-remote-command="${RemoteCommand.TaskView}">Task view</button>
            <button data-remote-command="${RemoteCommand.MinimizeForeground}">Minimize</button>
            <button id="secureAttentionButton">Ctrl+Alt+Del</button>
          </div>
          <p>Three fingers: swipe left/right to switch apps, up for Task View, down to minimize. Windows blocks synthetic Ctrl+Alt+Del on its secure screen.</p>
        </section>
        <button id="toolbarReveal" class="toolbar-reveal" aria-label="Show controls"></button>
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
    const gestureButton = this.requiredElement<HTMLButtonElement>("gestureButton");
    gestureButton.disabled = this.touchEnabled;
    gestureButton.textContent = this.touchEnabled ? "Gestures paused" : this.gesturesEnabled ? "Gestures on" : "Gestures off";
    gestureButton.setAttribute("aria-pressed", String(this.gesturesEnabled));
    const keyboardButton = this.requiredElement<HTMLButtonElement>("keyboardButton");
    keyboardButton.disabled = this.status?.keyboard_enabled === false;
    keyboardButton.title = keyboardButton.disabled ? "Keyboard forwarding is disabled on Windows" : "Open remote keyboard";
  }

  private markFirstVideoFrame(): void {
    if (this.firstVideoFrameAtMs === 0) {
      this.firstVideoFrameAtMs = performance.now();
      this.renderStats();
    }
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
      this.touchEnabled = !this.touchEnabled;
      const button = this.requiredElement<HTMLButtonElement>("touchButton");
      button.textContent = this.touchEnabled ? "Touch on" : "Touch off";
      button.setAttribute("aria-pressed", String(this.touchEnabled));
      const gestureButton = this.requiredElement<HTMLButtonElement>("gestureButton");
      gestureButton.disabled = this.touchEnabled;
      gestureButton.textContent = this.touchEnabled ? "Gestures paused" : this.gesturesEnabled ? "Gestures on" : "Gestures off";
      this.scheduleToolbarHide();
    });
    this.requiredElement("gestureButton").addEventListener("click", () => {
      this.gesturesEnabled = !this.gesturesEnabled;
      const button = this.requiredElement<HTMLButtonElement>("gestureButton");
      button.textContent = this.gesturesEnabled ? "Gestures on" : "Gestures off";
      button.setAttribute("aria-pressed", String(this.gesturesEnabled));
      this.scheduleToolbarHide();
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
    this.requiredElement("secureAttentionButton").addEventListener("click", () => {
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
    this.requiredElement("toolbarReveal").addEventListener("pointerdown", () => this.showToolbar());
    this.requiredElement("surface").addEventListener("pointerdown", () => this.scheduleToolbarHide(), { passive: true });
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
      `${playbackStartup} · ${metrics.video_startup_wait_ms.toFixed(0)} ms IDR · ${(client?.frameTiming.captureToPresentP95Ms ?? client?.frameTiming.estimatedPipelineMs ?? 0).toFixed(1)} ms p95 measured/estimated · ${(client?.frameTiming.frameGapP95Ms ?? 0).toFixed(1)} ms frame-gap p95`,
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
    this.scheduleToolbarHide();
  }

  private scheduleToolbarHide(): void {
    window.clearTimeout(this.hideToolbarTimer);
    this.requiredElement("toolbar").classList.add("visible");
    this.hideToolbarTimer = window.setTimeout(() => {
      if (
        this.root.querySelector<HTMLElement>("#statsPanel")?.hidden !== false &&
        this.root.querySelector<HTMLElement>("#keyboardPanel")?.hidden !== false
      ) {
        this.root.querySelector("#toolbar")?.classList.remove("visible");
      }
    }, 2400);
  }

  private async disconnect(): Promise<void> {
    this.stopDiagnosticRecording();
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
      // Local teardown still completes when the PC disappeared.
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
        <h1>Couldn’t reach the Windows bridge</h1><p>${escapeHtml(message)}</p>
        <button class="primary-button" id="retryButton">Try again</button>
        <p class="pair-help">Check Windows Firewall Private network access and confirm both devices are on the same Wi-Fi without guest isolation.</p>
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
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(0)} KiB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
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
