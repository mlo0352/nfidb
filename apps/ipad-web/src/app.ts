import { disconnect, getMetrics, getStatus, pairWithPin, pairWithQr, sendOffer, type HostMetrics, type HostStatus } from "./api";
import type { FitMode } from "./geometry";
import { PointerEngine } from "./pointer-engine";

type ConnectionState = "pairing" | "connecting" | "connected" | "input-only" | "failed";
type InputTransport = "datachannel" | "websocket";

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
  };
  inboundVideo: Record<string, unknown> | null;
  candidatePair: Record<string, unknown> | null;
  host: HostMetrics;
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
  private inputTransport: InputTransport | null = null;
  private readonly pendingInput: ArrayBuffer[] = [];
  private fitMode: FitMode = "fit";
  private touchEnabled = false;
  private metrics: HostMetrics | null = null;
  private hideToolbarTimer = 0;

  constructor(root: HTMLElement) {
    this.root = root;
  }

  async start(): Promise<void> {
    this.renderLoading();
    try {
      this.status = await getStatus();
      this.touchEnabled = this.status.touch_default;
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
    const host = escapeHtml(this.status?.host_name ?? "Windows PC");
    this.root.innerHTML = `
      <main class="pairing-shell">
        <section class="pair-panel" aria-labelledby="pair-title">
          <div class="brand-lockup"><div class="wordmark"><span>NFi</span>DB</div><p>No Frills iPad Drawing Bridge</p></div>
          <div class="host-found"><i></i><span>${host} found on your local network</span></div>
          <h1 id="pair-title">Enter the PIN shown on Windows</h1>
          <form id="pairForm" novalidate>
            <label for="pin">Six-digit PIN</label>
            <input id="pin" name="pin" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000 000" aria-describedby="pairError" autofocus />
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
      const digits = pin.value.replace(/\D/g, "").slice(0, 6);
      pin.value = digits.length > 3 ? `${digits.slice(0, 3)} ${digits.slice(3)}` : digits;
    });
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submitPin(pin.value);
    });
  }

  private async submitPin(pin: string): Promise<void> {
    const button = this.root.querySelector<HTMLButtonElement>("button[type=submit]");
    if (button) {
      button.disabled = true;
      button.textContent = "Pairing…";
    }
    try {
      const result = await pairWithPin(pin);
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
        video.srcObject = event.streams[0] ?? new MediaStream([event.track]);
        void video.play().catch(() => undefined);
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
            <button id="statsButton" class="tool-button" aria-pressed="false">Stats</button>
            <button id="fullscreenButton" class="icon-button" aria-label="Fullscreen">⛶</button>
            <button id="disconnectButton" class="icon-button danger" aria-label="Disconnect">×</button>
          </div>
        </header>
        <aside id="statsPanel" class="stats-panel" hidden>
          <div><span>VIDEO</span><b id="videoStats">Waiting…</b></div>
          <div><span>NETWORK</span><b id="networkStats">Local</b></div>
          <div><span>PENCIL</span><b id="pencilStats">Waiting…</b></div>
          <div><span>PRESSURE / TILT</span><b id="pressureStats">0.00 · 0° / 0°</b></div>
        </aside>
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
    this.pointerEngine = new PointerEngine({
      overlay,
      video,
      send: (packet) => this.sendInput(packet),
      getFitMode: () => this.fitMode,
      getTouchEnabled: () => this.touchEnabled,
      inputEnabled: this.status?.mode !== "display-only",
    });
    this.bindSurfaceControls();
    this.scheduleToolbarHide();
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
      this.scheduleToolbarHide();
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
    });
    socket.addEventListener("message", (event) => this.handleControlMessage(event.data));
    socket.addEventListener("close", () => {
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
        this.showNotice("Input transport is unavailable. Lift the Pencil and reconnect before drawing.");
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
    this.showNotice("Input transport changed. Lift the Pencil once before continuing.");
  }

  private handleControlMessage(data: unknown): void {
    if (typeof data !== "string") {
      return;
    }
    try {
      const message = JSON.parse(data) as { type?: string; t0?: number; stats?: HostMetrics };
      if (message.type === "stats" && message.stats) {
        this.metrics = message.stats;
        this.renderStats();
      } else if (message.type === "pong" && typeof message.t0 === "number") {
        const rtt = performance.now() - message.t0;
        if (this.metrics) {
          this.metrics.rtt_ms = rtt;
        }
        window.setTimeout(() => this.sendPing(), 2000);
      }
    } catch {
      // Ignore malformed diagnostic messages; pointer transport is independent.
    }
  }

  private sendPing(): void {
    if (this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify({ type: "ping", t0: performance.now() }));
    }
  }

  private updatePeerState(): void {
    const state = this.peer?.connectionState;
    if (state === "connected") {
      this.state = "connected";
    } else if (state === "failed" || state === "closed") {
      this.state = "input-only";
      this.pointerEngine?.cancelAll();
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
    this.setText("networkStats", `${metrics.rtt_ms.toFixed(1)} ms RTT · ${formatBytes(metrics.encoded_bytes)} sent · ${metrics.dropped_frames} stale / ${metrics.video_transport_drops} transport skips`);
    this.setText("pencilStats", `${metrics.input_samples_per_sec.toFixed(0)} samples/s · ${metrics.input_samples} total · ${metrics.sample_sequence_gaps} gaps`);
    this.setText("pressureStats", `${metrics.pressure.toFixed(2)} · ${metrics.tilt_x.toFixed(0)}° / ${metrics.tilt_y.toFixed(0)}°`);
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
      if (this.root.querySelector<HTMLElement>("#statsPanel")?.hidden !== false) {
        this.root.querySelector("#toolbar")?.classList.remove("visible");
      }
    }, 2400);
  }

  private async disconnect(): Promise<void> {
    this.pointerEngine?.cancelAll();
    this.pointerEngine?.dispose();
    this.channel?.close();
    this.peer?.close();
    this.socket?.close();
    this.inputTransport = null;
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
    let inboundVideo: Record<string, unknown> | null = null;
    let candidatePair: Record<string, unknown> | null = null;
    if (this.peer) {
      const reports = await this.peer.getStats();
      for (const report of reports.values()) {
        const record = report as unknown as Record<string, unknown>;
        if (report.type === "inbound-rtp" && (record.kind === "video" || record.mediaType === "video")) {
          inboundVideo = pickStats(record, [
            "bytesReceived",
            "packetsReceived",
            "packetsLost",
            "framesReceived",
            "framesDecoded",
            "framesDropped",
            "framesPerSecond",
            "frameWidth",
            "frameHeight",
            "jitter",
            "jitterBufferDelay",
            "jitterBufferEmittedCount",
            "totalDecodeTime",
            "totalInterFrameDelay",
            "freezeCount",
            "totalFreezesDuration",
          ]);
        } else if (report.type === "candidate-pair" && record.state === "succeeded" && record.nominated === true) {
          candidatePair = pickStats(record, [
            "currentRoundTripTime",
            "availableIncomingBitrate",
            "bytesReceived",
            "bytesSent",
            "localCandidateId",
            "remoteCandidateId",
          ]);
        }
      }
    }
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
      },
      inboundVideo,
      candidatePair,
      host: await getMetrics(),
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
