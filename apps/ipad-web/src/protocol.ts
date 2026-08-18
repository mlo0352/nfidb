export const PROTOCOL_VERSION = 1;
export const POINTER_BATCH_MESSAGE = 1;
export const WHEEL_MESSAGE = 2;
export const KEYBOARD_MESSAGE = 3;
export const TEXT_MESSAGE = 4;
export const COMMAND_MESSAGE = 5;
export const BATCH_HEADER_BYTES = 16;
export const SAMPLE_BYTES = 44;
export const MAX_SAMPLES_PER_BATCH = 512;

export const DeviceType = {
  Pen: 1,
  Touch: 2,
  Mouse: 3,
} as const;

export type DeviceTypeValue = (typeof DeviceType)[keyof typeof DeviceType];

export const PointerAction = {
  Down: 1,
  Move: 2,
  Up: 3,
  Cancel: 4,
  Hover: 5,
} as const;

export type PointerActionValue = (typeof PointerAction)[keyof typeof PointerAction];

export interface PointerSample {
  deviceType: DeviceTypeValue;
  action: PointerActionValue;
  flags: number;
  pointerId: number;
  sampleSequence: number;
  xNorm: number;
  yNorm: number;
  pressure: number;
  tiltXDeg: number;
  tiltYDeg: number;
  twistDeg: number;
  clientTimeMs: number;
}

export interface PointerBatch {
  batchSequence: number;
  clientSendTimeMs: number;
  samples: readonly PointerSample[];
}

export const KeyAction = {
  Down: 1,
  Up: 2,
} as const;

export type KeyActionValue = (typeof KeyAction)[keyof typeof KeyAction];

export const RemoteCommand = {
  AppNext: 1,
  AppPrevious: 2,
  MinimizeForeground: 3,
  TaskView: 4,
  ResetInput: 5,
} as const;

export type RemoteCommandValue = (typeof RemoteCommand)[keyof typeof RemoteCommand];

export interface WheelInput {
  modifiers: number;
  sequence: number;
  xNorm: number;
  yNorm: number;
  deltaX: number;
  deltaY: number;
  clientTimeMs: number;
}

export interface KeyboardInput {
  action: KeyActionValue;
  location: number;
  repeat: boolean;
  modifiers: number;
  sequence: number;
  clientTimeMs: number;
  code: string;
  key: string;
}

export function encodePointerBatch(batch: PointerBatch): ArrayBuffer {
  if (batch.samples.length > MAX_SAMPLES_PER_BATCH) {
    throw new RangeError(`A pointer batch may contain at most ${MAX_SAMPLES_PER_BATCH} samples`);
  }
  const buffer = new ArrayBuffer(BATCH_HEADER_BYTES + batch.samples.length * SAMPLE_BYTES);
  const view = new DataView(buffer);
  let offset = 0;
  view.setUint8(offset, PROTOCOL_VERSION);
  offset += 1;
  view.setUint8(offset, POINTER_BATCH_MESSAGE);
  offset += 1;
  view.setUint16(offset, batch.samples.length, true);
  offset += 2;
  view.setUint32(offset, batch.batchSequence >>> 0, true);
  offset += 4;
  view.setFloat64(offset, finiteOr(batch.clientSendTimeMs, 0), true);
  offset += 8;

  for (const rawSample of batch.samples) {
    const sample = sanitizeSample(rawSample);
    view.setUint8(offset, sample.deviceType);
    offset += 1;
    view.setUint8(offset, sample.action);
    offset += 1;
    view.setUint16(offset, sample.flags & 0xffff, true);
    offset += 2;
    view.setUint32(offset, sample.pointerId >>> 0, true);
    offset += 4;
    view.setUint32(offset, sample.sampleSequence >>> 0, true);
    offset += 4;
    view.setFloat32(offset, sample.xNorm, true);
    offset += 4;
    view.setFloat32(offset, sample.yNorm, true);
    offset += 4;
    view.setFloat32(offset, sample.pressure, true);
    offset += 4;
    view.setFloat32(offset, sample.tiltXDeg, true);
    offset += 4;
    view.setFloat32(offset, sample.tiltYDeg, true);
    offset += 4;
    view.setFloat32(offset, sample.twistDeg, true);
    offset += 4;
    view.setFloat64(offset, sample.clientTimeMs, true);
    offset += 8;
  }
  return buffer;
}

export function encodeWheelInput(input: WheelInput): ArrayBuffer {
  const buffer = new ArrayBuffer(32);
  const view = new DataView(buffer);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, WHEEL_MESSAGE);
  view.setUint16(2, input.modifiers & 0xffff, true);
  view.setUint32(4, input.sequence >>> 0, true);
  view.setFloat32(8, clamp(finiteOr(input.xNorm, 0), 0, 1), true);
  view.setFloat32(12, clamp(finiteOr(input.yNorm, 0), 0, 1), true);
  view.setFloat32(16, clamp(finiteOr(input.deltaX, 0), -10_000, 10_000), true);
  view.setFloat32(20, clamp(finiteOr(input.deltaY, 0), -10_000, 10_000), true);
  view.setFloat64(24, finiteOr(input.clientTimeMs, 0), true);
  return buffer;
}

export function encodeKeyboardInput(input: KeyboardInput): ArrayBuffer {
  const encoder = new TextEncoder();
  const code = encoder.encode(input.code);
  const key = encoder.encode(input.key);
  if (code.length === 0 || code.length > 64 || key.length > 64 || !isAscii(code)) {
    throw new RangeError("Keyboard code/key fields must be valid and at most 64 UTF-8 bytes");
  }
  const buffer = new ArrayBuffer(24 + code.length + key.length);
  const view = new DataView(buffer);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, KEYBOARD_MESSAGE);
  view.setUint8(2, input.action);
  view.setUint8(3, input.location & 0xff);
  view.setUint16(4, input.modifiers & 0xffff, true);
  view.setUint8(6, input.repeat ? 1 : 0);
  view.setUint8(7, 0);
  view.setUint32(8, input.sequence >>> 0, true);
  view.setBigUint64(12, BigInt(Math.max(0, Math.round(finiteOr(input.clientTimeMs, 0)))), true);
  view.setUint16(20, code.length, true);
  view.setUint16(22, key.length, true);
  new Uint8Array(buffer, 24, code.length).set(code);
  new Uint8Array(buffer, 24 + code.length, key.length).set(key);
  return buffer;
}

export function encodeTextInput(sequence: number, clientTimeMs: number, text: string): ArrayBuffer {
  const encoded = new TextEncoder().encode(text);
  if (encoded.length === 0 || encoded.length > 4096) {
    throw new RangeError("Text messages must contain 1 through 4096 UTF-8 bytes");
  }
  const buffer = new ArrayBuffer(20 + encoded.length);
  const view = new DataView(buffer);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, TEXT_MESSAGE);
  view.setUint16(2, 0, true);
  view.setUint32(4, sequence >>> 0, true);
  view.setBigUint64(8, BigInt(Math.max(0, Math.round(finiteOr(clientTimeMs, 0)))), true);
  view.setUint32(16, encoded.length, true);
  new Uint8Array(buffer, 20).set(encoded);
  return buffer;
}

export function encodeCommandInput(command: RemoteCommandValue, sequence: number, clientTimeMs: number): ArrayBuffer {
  const buffer = new ArrayBuffer(16);
  const view = new DataView(buffer);
  view.setUint8(0, PROTOCOL_VERSION);
  view.setUint8(1, COMMAND_MESSAGE);
  view.setUint8(2, command);
  view.setUint8(3, 0);
  view.setUint32(4, sequence >>> 0, true);
  view.setBigUint64(8, BigInt(Math.max(0, Math.round(finiteOr(clientTimeMs, 0)))), true);
  return buffer;
}

export function sanitizeSample(sample: PointerSample): PointerSample {
  return {
    ...sample,
    flags: sample.flags & 0xffff,
    pointerId: sample.pointerId >>> 0,
    sampleSequence: sample.sampleSequence >>> 0,
    xNorm: clamp(finiteOr(sample.xNorm, 0), 0, 1),
    yNorm: clamp(finiteOr(sample.yNorm, 0), 0, 1),
    pressure: clamp(finiteOr(sample.pressure, 0), 0, 1),
    tiltXDeg: clamp(finiteOr(sample.tiltXDeg, 0), -90, 90),
    tiltYDeg: clamp(finiteOr(sample.tiltYDeg, 0), -90, 90),
    twistDeg: positiveModulo(finiteOr(sample.twistDeg, 0), 360),
    clientTimeMs: finiteOr(sample.clientTimeMs, 0),
  };
}

function finiteOr(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

function positiveModulo(value: number, divisor: number): number {
  return ((value % divisor) + divisor) % divisor;
}

function isAscii(value: Uint8Array): boolean {
  return value.every((byte) => byte <= 0x7f);
}
