import { calculateContentRect, normalizePoint, type FitMode } from "./geometry";
import {
  KeyAction,
  RemoteCommand,
  encodeCommandInput,
  encodeKeyboardInput,
  encodeTextInput,
  encodeWheelInput,
  type RemoteCommandValue,
} from "./protocol";

const MODIFIER_SHIFT = 1 << 0;
const MODIFIER_CONTROL = 1 << 1;
const MODIFIER_ALT = 1 << 2;
const MODIFIER_META = 1 << 3;
const GESTURE_TIMEOUT_MS = 1500;

interface TouchPoint {
  startX: number;
  startY: number;
  x: number;
  y: number;
}

export interface RemoteInputEngineOptions {
  overlay: HTMLCanvasElement;
  video: HTMLVideoElement;
  send: (packet: ArrayBuffer) => void;
  getFitMode: () => FitMode;
  getTouchEnabled: () => boolean;
  getGesturesEnabled: () => boolean;
  inputEnabled?: boolean;
  onNotice?: (message: string) => void;
}

export class RemoteInputEngine {
  private readonly overlay: HTMLCanvasElement;
  private readonly video: HTMLVideoElement;
  private readonly sendPacket: (packet: ArrayBuffer) => void;
  private readonly getFitMode: () => FitMode;
  private readonly getTouchEnabled: () => boolean;
  private readonly getGesturesEnabled: () => boolean;
  private readonly onNotice?: (message: string) => void;
  private readonly forwardedCodes = new Map<string, { code: string; key: string }>();
  private readonly touches = new Map<number, TouchPoint>();
  private readonly activePens = new Set<number>();
  private textInput: HTMLTextAreaElement | null = null;
  private messageSequence = 0;
  private composing = false;
  private gestureStartedAt = 0;
  private gestureTriggered = false;
  private disposed = false;

  constructor(options: RemoteInputEngineOptions) {
    this.overlay = options.overlay;
    this.video = options.video;
    this.sendPacket = options.send;
    this.getFitMode = options.getFitMode;
    this.getTouchEnabled = options.getTouchEnabled;
    this.getGesturesEnabled = options.getGesturesEnabled;
    this.onNotice = options.onNotice;
    if (options.inputEnabled !== false) {
      this.bind();
    }
  }

  attachTextInput(input: HTMLTextAreaElement): void {
    this.detachTextInput();
    this.textInput = input;
    input.addEventListener("beforeinput", this.handleBeforeInput);
    input.addEventListener("input", this.handleTextInput);
    input.addEventListener("compositionstart", this.handleCompositionStart);
    input.addEventListener("compositionend", this.handleCompositionEnd);
  }

  focusTextInput(): void {
    this.textInput?.focus({ preventScroll: true });
  }

  tapKey(code: string, key = code): void {
    const now = Date.now();
    this.sendKeyboard(KeyAction.Down, code, key, false, 0, now);
    this.sendKeyboard(KeyAction.Up, code, key, false, 0, now);
  }

  sendChord(keys: ReadonlyArray<{ code: string; key: string }>): void {
    const now = Date.now();
    let modifiers = 0;
    for (const value of keys) {
      modifiers |= modifierForCode(value.code);
      this.sendKeyboard(KeyAction.Down, value.code, value.key, false, modifiers, now);
    }
    for (const value of [...keys].reverse()) {
      this.sendKeyboard(KeyAction.Up, value.code, value.key, false, modifiers, now);
      modifiers &= ~modifierForCode(value.code);
    }
  }

  sendCommand(command: RemoteCommandValue): void {
    this.sendPacket(encodeCommandInput(command, this.nextSequence(), Date.now()));
  }

  resetRemoteInput(): void {
    this.sendCommand(RemoteCommand.ResetInput);
    this.forwardedCodes.clear();
    this.touches.clear();
    this.activePens.clear();
    this.gestureTriggered = false;
  }

  abandonAll(): void {
    this.forwardedCodes.clear();
    this.touches.clear();
    this.activePens.clear();
    this.gestureTriggered = false;
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.detachTextInput();
    this.overlay.removeEventListener("wheel", this.handleWheel);
    this.overlay.removeEventListener("contextmenu", this.handleContextMenu);
    this.overlay.removeEventListener("pointerdown", this.handleGesturePointer, true);
    this.overlay.removeEventListener("pointermove", this.handleGesturePointer, true);
    this.overlay.removeEventListener("pointerup", this.handleGesturePointer, true);
    this.overlay.removeEventListener("pointercancel", this.handleGesturePointer, true);
    window.removeEventListener("keydown", this.handleKeyboard, true);
    window.removeEventListener("keyup", this.handleKeyboard, true);
    window.removeEventListener("blur", this.handleBlur);
    document.removeEventListener("visibilitychange", this.handleVisibility);
  }

  private bind(): void {
    this.overlay.addEventListener("wheel", this.handleWheel, { passive: false });
    this.overlay.addEventListener("contextmenu", this.handleContextMenu);
    const pointerOptions: AddEventListenerOptions = { passive: false, capture: true };
    this.overlay.addEventListener("pointerdown", this.handleGesturePointer, pointerOptions);
    this.overlay.addEventListener("pointermove", this.handleGesturePointer, pointerOptions);
    this.overlay.addEventListener("pointerup", this.handleGesturePointer, pointerOptions);
    this.overlay.addEventListener("pointercancel", this.handleGesturePointer, pointerOptions);
    window.addEventListener("keydown", this.handleKeyboard, true);
    window.addEventListener("keyup", this.handleKeyboard, true);
    window.addEventListener("blur", this.handleBlur);
    document.addEventListener("visibilitychange", this.handleVisibility);
  }

  private detachTextInput(): void {
    if (!this.textInput) {
      return;
    }
    this.textInput.removeEventListener("beforeinput", this.handleBeforeInput);
    this.textInput.removeEventListener("input", this.handleTextInput);
    this.textInput.removeEventListener("compositionstart", this.handleCompositionStart);
    this.textInput.removeEventListener("compositionend", this.handleCompositionEnd);
    this.textInput = null;
  }

  private readonly handleWheel = (event: WheelEvent): void => {
    const point = this.normalizedPoint(event.clientX, event.clientY);
    if (!point) {
      return;
    }
    event.preventDefault();
    const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? 40
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
        ? Math.max(1, this.overlay.clientHeight)
        : 1;
    this.sendPacket(
      encodeWheelInput({
        modifiers: modifiersFromEvent(event),
        sequence: this.nextSequence(),
        xNorm: point.u,
        yNorm: point.v,
        deltaX: event.deltaX * unit,
        deltaY: event.deltaY * unit,
        clientTimeMs: eventEpochMs(event.timeStamp),
      }),
    );
  };

  private readonly handleContextMenu = (event: MouseEvent): void => {
    event.preventDefault();
  };

  private readonly handleKeyboard = (event: KeyboardEvent): void => {
    if (this.disposed || event.isComposing || event.key === "Process" || event.key === "Unidentified") {
      return;
    }
    const identity = normalizedKey(event);
    const textTarget = event.target === this.textInput;
    if (event.type === "keydown") {
      if (!textTarget || shouldForwardRaw(event)) {
        event.preventDefault();
        this.forwardedCodes.set(event.code || identity.code, identity);
        this.sendKeyboard(
          KeyAction.Down,
          identity.code,
          identity.key,
          event.repeat,
          modifiersFromEvent(event),
          eventEpochMs(event.timeStamp),
          event.location,
        );
      }
      return;
    }
    const forwarded = this.forwardedCodes.get(event.code || identity.code);
    if (forwarded) {
      this.forwardedCodes.delete(event.code || identity.code);
      event.preventDefault();
      this.sendKeyboard(
        KeyAction.Up,
        forwarded.code,
        forwarded.key,
        false,
        modifiersFromEvent(event),
        eventEpochMs(event.timeStamp),
        event.location,
      );
    }
  };

  private readonly handleBeforeInput = (event: InputEvent): void => {
    if (event.inputType === "deleteContentBackward" || event.inputType === "deleteWordBackward") {
      event.preventDefault();
      this.tapKey("Backspace", "Backspace");
    } else if (event.inputType === "deleteContentForward" || event.inputType === "deleteWordForward") {
      event.preventDefault();
      this.tapKey("Delete", "Delete");
    } else if (event.inputType === "insertLineBreak" || event.inputType === "insertParagraph") {
      event.preventDefault();
      this.tapKey("Enter", "Enter");
    }
  };

  private readonly handleTextInput = (): void => {
    if (!this.composing) {
      this.flushTextInput();
    }
  };

  private readonly handleCompositionStart = (): void => {
    this.composing = true;
  };

  private readonly handleCompositionEnd = (): void => {
    this.composing = false;
    queueMicrotask(() => this.flushTextInput());
  };

  private flushTextInput(): void {
    if (!this.textInput || this.textInput.value.length === 0) {
      return;
    }
    const text = this.textInput.value;
    this.textInput.value = "";
    this.sendText(text, Date.now());
  }

  private sendText(text: string, clientTimeMs: number): void {
    for (const chunk of utf8Chunks(text, 4096)) {
      if (chunk.length > 0) {
        this.sendPacket(encodeTextInput(this.nextSequence(), clientTimeMs, chunk));
      }
    }
  }

  private sendKeyboard(
    action: 1 | 2,
    code: string,
    key: string,
    repeat: boolean,
    modifiers: number,
    clientTimeMs: number,
    location = 0,
  ): void {
    this.sendPacket(
      encodeKeyboardInput({
        action,
        location,
        repeat,
        modifiers,
        sequence: this.nextSequence(),
        clientTimeMs,
        code,
        key,
      }),
    );
  }

  private readonly handleGesturePointer = (event: PointerEvent): void => {
    if (event.pointerType === "pen") {
      if (event.type === "pointerdown") {
        this.activePens.add(event.pointerId);
        this.resetGesture();
      } else if (event.type === "pointerup" || event.type === "pointercancel") {
        this.activePens.delete(event.pointerId);
      }
      return;
    }
    if (event.pointerType !== "touch") {
      return;
    }
    if (this.getTouchEnabled() || !this.getGesturesEnabled() || this.activePens.size > 0) {
      this.resetGesture();
      return;
    }
    event.preventDefault();
    if (event.type === "pointerdown") {
      this.touches.set(event.pointerId, {
        startX: event.clientX,
        startY: event.clientY,
        x: event.clientX,
        y: event.clientY,
      });
      if (this.touches.size === 3) {
        for (const point of this.touches.values()) {
          point.startX = point.x;
          point.startY = point.y;
        }
        this.gestureStartedAt = performance.now();
        this.gestureTriggered = false;
      } else if (this.touches.size > 3) {
        this.gestureTriggered = true;
      }
      return;
    }
    const point = this.touches.get(event.pointerId);
    if (point) {
      point.x = event.clientX;
      point.y = event.clientY;
    }
    if (event.type === "pointermove") {
      this.evaluateGesture();
    } else if (event.type === "pointerup" || event.type === "pointercancel") {
      this.touches.delete(event.pointerId);
      if (this.touches.size === 0) {
        this.resetGesture();
      }
    }
  };

  private evaluateGesture(): void {
    if (
      this.touches.size !== 3 ||
      this.gestureTriggered ||
      performance.now() - this.gestureStartedAt > GESTURE_TIMEOUT_MS
    ) {
      return;
    }
    const points = [...this.touches.values()];
    const dx = points.reduce((sum, point) => sum + point.x - point.startX, 0) / points.length;
    const dy = points.reduce((sum, point) => sum + point.y - point.startY, 0) / points.length;
    const threshold = Math.max(52, Math.min(this.overlay.clientWidth, this.overlay.clientHeight) * 0.065);
    let command: RemoteCommandValue | null = null;
    let label = "";
    if (Math.abs(dx) >= threshold && Math.abs(dx) > Math.abs(dy) * 1.2) {
      command = dx > 0 ? RemoteCommand.AppNext : RemoteCommand.AppPrevious;
      label = dx > 0 ? "Next Windows app" : "Previous Windows app";
    } else if (Math.abs(dy) >= threshold && Math.abs(dy) > Math.abs(dx) * 1.2) {
      command = dy > 0 ? RemoteCommand.MinimizeForeground : RemoteCommand.TaskView;
      label = dy > 0 ? "Minimize Windows app" : "Windows Task View";
    }
    if (command !== null) {
      this.gestureTriggered = true;
      this.sendCommand(command);
      this.onNotice?.(label);
    }
  }

  private resetGesture(): void {
    this.touches.clear();
    this.gestureStartedAt = 0;
    this.gestureTriggered = false;
  }

  private readonly handleBlur = (): void => {
    this.resetRemoteInput();
  };

  private readonly handleVisibility = (): void => {
    if (document.hidden) {
      this.resetRemoteInput();
    }
  };

  private normalizedPoint(clientX: number, clientY: number): { u: number; v: number } | null {
    const bounds = this.overlay.getBoundingClientRect();
    const sourceWidth = this.video.videoWidth || bounds.width;
    const sourceHeight = this.video.videoHeight || bounds.height;
    const rect = calculateContentRect(bounds.width, bounds.height, sourceWidth, sourceHeight, this.getFitMode());
    return normalizePoint(clientX - bounds.left, clientY - bounds.top, rect, this.getFitMode() !== "fit");
  }

  private nextSequence(): number {
    const value = this.messageSequence;
    this.messageSequence = (this.messageSequence + 1) >>> 0;
    return value;
  }
}

function shouldForwardRaw(event: KeyboardEvent): boolean {
  return event.ctrlKey || event.altKey || event.metaKey || event.key.length !== 1 || modifierForCode(event.code) !== 0;
}

function normalizedKey(event: KeyboardEvent): { code: string; key: string } {
  const controlAltDelete = event.ctrlKey && event.altKey && (event.code === "Backspace" || event.code === "Delete");
  if (controlAltDelete) {
    return { code: "Delete", key: "Delete" };
  }
  if (event.code === "Delete" && !event.ctrlKey && !event.altKey && !event.metaKey) {
    return { code: "Backspace", key: "Backspace" };
  }
  return { code: event.code || event.key, key: event.key };
}

function modifierForCode(code: string): number {
  if (code.startsWith("Shift")) {
    return MODIFIER_SHIFT;
  }
  if (code.startsWith("Control")) {
    return MODIFIER_CONTROL;
  }
  if (code.startsWith("Alt")) {
    return MODIFIER_ALT;
  }
  if (code.startsWith("Meta")) {
    return MODIFIER_META;
  }
  return 0;
}

function modifiersFromEvent(event: MouseEvent | KeyboardEvent | WheelEvent): number {
  return (event.shiftKey ? MODIFIER_SHIFT : 0) |
    (event.ctrlKey ? MODIFIER_CONTROL : 0) |
    (event.altKey ? MODIFIER_ALT : 0) |
    (event.metaKey ? MODIFIER_META : 0);
}

function eventEpochMs(timeStamp: number): number {
  return timeStamp > 1_000_000_000_000 ? timeStamp : performance.timeOrigin + timeStamp;
}

function utf8Chunks(text: string, maximumBytes: number): string[] {
  const encoder = new TextEncoder();
  const chunks: string[] = [];
  let current = "";
  let currentBytes = 0;
  for (const character of text) {
    const bytes = encoder.encode(character).length;
    if (currentBytes + bytes > maximumBytes && current.length > 0) {
      chunks.push(current);
      current = "";
      currentBytes = 0;
    }
    current += character;
    currentBytes += bytes;
  }
  if (current.length > 0) {
    chunks.push(current);
  }
  return chunks;
}
