import { calculateContentRect, normalizePoint, type FitMode } from "./geometry";
import {
  DeviceType,
  PointerAction,
  encodePointerBatch,
  type DeviceTypeValue,
  type PointerActionValue,
  type PointerSample,
} from "./protocol";

type ExtendedPointerEvent = PointerEvent & {
  getPredictedEvents?: () => PointerEvent[];
};

export interface PointerTelemetry {
  pointerType: string;
  pointerId: number;
  x: number;
  y: number;
  pressure: number;
  tiltX: number;
  tiltY: number;
  altitudeAngle: number;
  azimuthAngle: number;
  twist: number;
  buttons: number;
  coalesced: number;
  eventsPerSecond: number;
  samplesPerSecond: number;
}

export interface PointerEngineOptions {
  overlay: HTMLCanvasElement;
  video: HTMLVideoElement;
  send: (packet: ArrayBuffer) => void;
  getFitMode: () => FitMode;
  getTouchEnabled: () => boolean;
  inputEnabled?: boolean;
  onTelemetry?: (telemetry: PointerTelemetry) => void;
}

export class PointerEngine {
  private readonly overlay: HTMLCanvasElement;
  private readonly video: HTMLVideoElement;
  private readonly sendPacket: (packet: ArrayBuffer) => void;
  private readonly getFitMode: () => FitMode;
  private readonly getTouchEnabled: () => boolean;
  private readonly onTelemetry?: (telemetry: PointerTelemetry) => void;
  private readonly activePointers = new Map<number, DeviceTypeValue>();
  private readonly interruptedPointers = new Set<number>();
  private batchSequence = 0;
  private sampleSequence = 0;
  private eventCounter = 0;
  private sampleCounter = 0;
  private rateStarted = performance.now();
  private disposed = false;

  constructor(options: PointerEngineOptions) {
    this.overlay = options.overlay;
    this.video = options.video;
    this.sendPacket = options.send;
    this.getFitMode = options.getFitMode;
    this.getTouchEnabled = options.getTouchEnabled;
    this.onTelemetry = options.onTelemetry;
    if (options.inputEnabled !== false) {
      this.bind();
    }
  }

  dispose(): void {
    this.disposed = true;
    this.overlay.removeEventListener("pointerdown", this.handlePointer);
    this.overlay.removeEventListener("pointermove", this.handlePointer);
    this.overlay.removeEventListener("pointerup", this.handlePointer);
    this.overlay.removeEventListener("pointercancel", this.handlePointer);
    document.removeEventListener("visibilitychange", this.handleVisibility);
  }

  cancelAll(): void {
    for (const [pointerId, deviceType] of this.activePointers) {
      this.sendSamples([
        {
          deviceType,
          action: PointerAction.Cancel,
          flags: 0,
          pointerId,
          sampleSequence: this.nextSampleSequence(),
          xNorm: 0,
          yNorm: 0,
          pressure: 0,
          tiltXDeg: 0,
          tiltYDeg: 0,
          twistDeg: 0,
          clientTimeMs: Date.now(),
        },
      ]);
    }
    this.activePointers.clear();
  }

  abandonAll(): void {
    for (const pointerId of this.activePointers.keys()) {
      this.interruptedPointers.add(pointerId);
    }
    this.activePointers.clear();
  }

  private bind(): void {
    const listenerOptions: AddEventListenerOptions = { passive: false };
    this.overlay.addEventListener("pointerdown", this.handlePointer, listenerOptions);
    this.overlay.addEventListener("pointermove", this.handlePointer, listenerOptions);
    this.overlay.addEventListener("pointerup", this.handlePointer, listenerOptions);
    this.overlay.addEventListener("pointercancel", this.handlePointer, listenerOptions);
    document.addEventListener("visibilitychange", this.handleVisibility);
  }

  private readonly handleVisibility = (): void => {
    if (document.hidden) {
      this.cancelAll();
    }
  };

  private readonly handlePointer = (event: PointerEvent): void => {
    if (this.disposed || (event.pointerType !== "pen" && event.pointerType !== "touch")) {
      return;
    }
    if (this.interruptedPointers.has(event.pointerId)) {
      event.preventDefault();
      if (event.type === "pointerup" || event.type === "pointercancel") {
        this.interruptedPointers.delete(event.pointerId);
      }
      return;
    }
    if (event.pointerType === "touch" && !this.getTouchEnabled()) {
      event.preventDefault();
      return;
    }
    event.preventDefault();
    const extended = event as ExtendedPointerEvent;
    const coalesced = extended.getCoalescedEvents?.() ?? [];
    const actualEvents = coalesced.length > 0 ? coalesced : [event];
    const wasActive = this.activePointers.has(event.pointerId);
    const isContactMove =
      event.type === "pointermove" &&
      (event.buttons !== 0 || event.pressure > 0) &&
      !wasActive;
    const preserveActiveLifecycle =
      wasActive &&
      (event.type === "pointerdown" ||
        event.type === "pointermove" ||
        event.type === "pointerup" ||
        event.type === "pointercancel");
    const samples = actualEvents
      .map((sampleEvent) =>
        this.toSample(
          sampleEvent,
          event.type === "pointerdown" || event.type === "pointerup" || event.type === "pointercancel"
            ? "pointermove"
            : event.type,
          preserveActiveLifecycle,
        ),
      )
      .filter((sample): sample is PointerSample => sample !== null);

    // Pointer Events only defines coalesced samples for pointermove, but normalizing
    // lifecycle actions here also protects against browser-specific event lists. A
    // batch must never contain repeated Down or Up actions.
    if ((event.type === "pointerdown" && !wasActive) || isContactMove) {
      if (samples.length > 0) {
        samples[0]!.action = PointerAction.Down;
        for (const sample of samples.slice(1)) {
          sample.action = PointerAction.Move;
        }
        this.activePointers.set(event.pointerId, event.pointerType === "pen" ? DeviceType.Pen : DeviceType.Touch);
        try {
          this.overlay.setPointerCapture(event.pointerId);
        } catch {
          // Safari can reject capture if the pointer already ended; lifecycle packets still recover safely.
        }
      }
    } else if (event.type === "pointerdown") {
      for (const sample of samples) {
        sample.action = PointerAction.Move;
      }
    } else if (event.type === "pointerup" || event.type === "pointercancel") {
      if (wasActive && samples.length > 0) {
        for (const sample of samples) {
          sample.action = PointerAction.Move;
        }
        samples[samples.length - 1]!.action =
          event.type === "pointerup" ? PointerAction.Up : PointerAction.Cancel;
      } else {
        samples.length = 0;
      }
    }
    if (samples.length > 0) {
      this.sendSamples(samples);
      this.sampleCounter += samples.length;
    }
    this.eventCounter += 1;
    this.emitTelemetry(event, coalesced.length);
    this.drawPrediction(extended);

    if (event.type === "pointerup" || event.type === "pointercancel") {
      this.activePointers.delete(event.pointerId);
      try {
        this.overlay.releasePointerCapture(event.pointerId);
      } catch {
        // The browser may release automatically before this handler.
      }
    }
  };

  private toSample(event: PointerEvent, sourceType: string, preserveActiveLifecycle = false): PointerSample | null {
    const bounds = this.overlay.getBoundingClientRect();
    const sourceWidth = this.video.videoWidth || bounds.width;
    const sourceHeight = this.video.videoHeight || bounds.height;
    const rect = calculateContentRect(bounds.width, bounds.height, sourceWidth, sourceHeight, this.getFitMode());
    const point = normalizePoint(
      event.clientX - bounds.left,
      event.clientY - bounds.top,
      rect,
      preserveActiveLifecycle || this.getFitMode() !== "fit",
    );
    if (!point) {
      return null;
    }
    const deviceType: DeviceTypeValue = event.pointerType === "pen" ? DeviceType.Pen : DeviceType.Touch;
    return {
      deviceType,
      action: actionFor(sourceType, event),
      flags: event.buttons & 0xffff,
      pointerId: event.pointerId,
      sampleSequence: this.nextSampleSequence(),
      xNorm: point.u,
      yNorm: point.v,
      pressure: event.pressure,
      tiltXDeg: event.tiltX,
      tiltYDeg: event.tiltY,
      twistDeg: event.twist,
      clientTimeMs: eventEpochMs(event.timeStamp),
    };
  }

  private sendSamples(samples: readonly PointerSample[]): void {
    this.sendPacket(
      encodePointerBatch({
        batchSequence: this.batchSequence++ >>> 0,
        clientSendTimeMs: Date.now(),
        samples,
      }),
    );
  }

  private nextSampleSequence(): number {
    const sequence = this.sampleSequence;
    this.sampleSequence = (this.sampleSequence + 1) >>> 0;
    return sequence;
  }

  private emitTelemetry(event: PointerEvent, coalesced: number): void {
    if (!this.onTelemetry) {
      return;
    }
    const elapsed = Math.max(0.001, (performance.now() - this.rateStarted) / 1000);
    const extended = event as ExtendedPointerEvent;
    this.onTelemetry({
      pointerType: event.pointerType,
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      pressure: event.pressure,
      tiltX: event.tiltX,
      tiltY: event.tiltY,
      altitudeAngle: extended.altitudeAngle ?? 0,
      azimuthAngle: extended.azimuthAngle ?? 0,
      twist: event.twist,
      buttons: event.buttons,
      coalesced,
      eventsPerSecond: this.eventCounter / elapsed,
      samplesPerSecond: this.sampleCounter / elapsed,
    });
    if (elapsed >= 1) {
      this.eventCounter = 0;
      this.sampleCounter = 0;
      this.rateStarted = performance.now();
    }
  }

  private drawPrediction(event: ExtendedPointerEvent): void {
    const predictions = event.getPredictedEvents?.() ?? [];
    const prediction = predictions.at(-1);
    const context = this.overlay.getContext("2d");
    if (!context) {
      return;
    }
    context.clearRect(0, 0, this.overlay.width, this.overlay.height);
    if (!prediction) {
      return;
    }
    const bounds = this.overlay.getBoundingClientRect();
    const scaleX = this.overlay.width / Math.max(1, bounds.width);
    const scaleY = this.overlay.height / Math.max(1, bounds.height);
    context.beginPath();
    context.arc((prediction.clientX - bounds.left) * scaleX, (prediction.clientY - bounds.top) * scaleY, 3 * scaleX, 0, Math.PI * 2);
    context.fillStyle = "rgba(91, 224, 194, 0.62)";
    context.fill();
  }
}

function eventEpochMs(timeStamp: number): number {
  if (timeStamp > 1_000_000_000_000) {
    return timeStamp;
  }
  return performance.timeOrigin + timeStamp;
}

function actionFor(sourceType: string, event: PointerEvent): PointerActionValue {
  switch (sourceType) {
    case "pointerdown":
      return PointerAction.Down;
    case "pointerup":
      return PointerAction.Up;
    case "pointercancel":
      return PointerAction.Cancel;
    default:
      return event.pointerType === "pen" && event.pressure === 0 && event.buttons === 0
        ? PointerAction.Hover
        : PointerAction.Move;
  }
}
