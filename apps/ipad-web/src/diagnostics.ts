import { PointerEngine, type PointerTelemetry } from "./pointer-engine";

export class PointerDiagnostics {
  private readonly root: HTMLElement;
  private history: number[] = [];

  constructor(root: HTMLElement) {
    this.root = root;
  }

  start(): void {
    this.root.innerHTML = `
      <main class="diagnostics-page">
        <header><div class="wordmark"><span>NFi</span>DB</div><div><p class="eyebrow">POINTER DIAGNOSTICS</p><h1>Apple Pencil signal check</h1></div><a href="/">Back to pairing</a></header>
        <section class="diagnostic-grid">
          <div class="diagnostic-surface-wrap">
            <canvas id="diagnosticSurface"></canvas>
            <div class="surface-prompt"><b>Draw here</b><span>Real events only. Predicted samples are never sent as ink.</span></div>
          </div>
          <aside>
            <div class="metric"><span>TYPE / ID</span><b id="dType">Waiting for Pencil</b></div>
            <div class="metric"><span>POSITION</span><b id="dPosition">—</b></div>
            <div class="metric pressure-metric"><span>PRESSURE</span><b id="dPressure">0.000</b><i id="pressureBar"></i></div>
            <div class="metric"><span>TILT X / Y</span><b id="dTilt">0° / 0°</b></div>
            <div class="metric"><span>ALTITUDE / AZIMUTH</span><b id="dAngles">0.000 / 0.000</b></div>
            <div class="metric"><span>TWIST / BUTTONS</span><b id="dButtons">0° / 0</b></div>
            <div class="metric"><span>COALESCED</span><b id="dCoalesced">0 per event</b></div>
            <div class="metric"><span>RATE</span><b id="dRate">0 events/s · 0 samples/s</b></div>
            <canvas id="pressureGraph" width="600" height="120" aria-label="Pressure history"></canvas>
          </aside>
        </section>
      </main>`;
    const overlay = this.required<HTMLCanvasElement>("diagnosticSurface");
    const resize = () => {
      const scale = window.devicePixelRatio || 1;
      overlay.width = Math.round(overlay.clientWidth * scale);
      overlay.height = Math.round(overlay.clientHeight * scale);
    };
    resize();
    window.addEventListener("resize", resize, { passive: true });
    new PointerEngine({
      overlay,
      video: { videoWidth: overlay.clientWidth, videoHeight: overlay.clientHeight } as HTMLVideoElement,
      send: () => undefined,
      getFitMode: () => "fit",
      getTouchEnabled: () => true,
      onTelemetry: (telemetry) => this.update(telemetry),
    });
  }

  private update(telemetry: PointerTelemetry): void {
    this.set("dType", `${telemetry.pointerType || "unknown"} / ${telemetry.pointerId}`);
    this.set("dPosition", `${telemetry.x.toFixed(1)} / ${telemetry.y.toFixed(1)}`);
    this.set("dPressure", telemetry.pressure.toFixed(3));
    this.set("dTilt", `${telemetry.tiltX.toFixed(1)}° / ${telemetry.tiltY.toFixed(1)}°`);
    this.set("dAngles", `${telemetry.altitudeAngle.toFixed(3)} / ${telemetry.azimuthAngle.toFixed(3)}`);
    this.set("dButtons", `${telemetry.twist.toFixed(0)}° / ${telemetry.buttons}`);
    this.set("dCoalesced", `${telemetry.coalesced} per event`);
    this.set("dRate", `${telemetry.eventsPerSecond.toFixed(0)} events/s · ${telemetry.samplesPerSecond.toFixed(0)} samples/s`);
    this.required<HTMLElement>("pressureBar").style.transform = `scaleX(${telemetry.pressure})`;
    this.history.push(telemetry.pressure);
    if (this.history.length > 180) {
      this.history.shift();
    }
    this.drawHistory();
  }

  private drawHistory(): void {
    const canvas = this.required<HTMLCanvasElement>("pressureGraph");
    const context = canvas.getContext("2d");
    if (!context) {
      return;
    }
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.strokeStyle = "#263033";
    context.beginPath();
    context.moveTo(0, canvas.height - 1);
    context.lineTo(canvas.width, canvas.height - 1);
    context.stroke();
    context.strokeStyle = "#5be0c2";
    context.lineWidth = 3;
    context.beginPath();
    this.history.forEach((pressure, index) => {
      const x = (index / 179) * canvas.width;
      const y = canvas.height - pressure * (canvas.height - 8) - 4;
      if (index === 0) context.moveTo(x, y);
      else context.lineTo(x, y);
    });
    context.stroke();
  }

  private set(id: string, text: string): void {
    this.required(id).textContent = text;
  }

  private required<T extends HTMLElement = HTMLElement>(id: string): T {
    const element = this.root.querySelector<T>(`#${id}`);
    if (!element) throw new Error(`Missing #${id}`);
    return element;
  }
}
