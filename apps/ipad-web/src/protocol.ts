export const PROTOCOL_VERSION = 1;
export const POINTER_BATCH_MESSAGE = 1;
export const BATCH_HEADER_BYTES = 16;
export const SAMPLE_BYTES = 44;
export const MAX_SAMPLES_PER_BATCH = 512;

export const DeviceType = {
  Pen: 1,
  Touch: 2,
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
