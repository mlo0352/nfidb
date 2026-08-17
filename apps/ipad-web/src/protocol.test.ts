import { describe, expect, it } from "vitest";
import { DeviceType, PointerAction, encodePointerBatch, SAMPLE_BYTES, BATCH_HEADER_BYTES } from "./protocol";

describe("pointer protocol", () => {
  it("writes the documented little-endian layout", () => {
    const buffer = encodePointerBatch({
      batchSequence: 0x12345678,
      clientSendTimeMs: 100.5,
      samples: [
        {
          deviceType: DeviceType.Pen,
          action: PointerAction.Move,
          flags: 7,
          pointerId: 42,
          sampleSequence: 9,
          xNorm: 0.25,
          yNorm: 0.75,
          pressure: 0.5,
          tiltXDeg: -30,
          tiltYDeg: 31,
          twistDeg: 123,
          clientTimeMs: 500.25,
        },
      ],
    });
    expect(buffer.byteLength).toBe(BATCH_HEADER_BYTES + SAMPLE_BYTES);
    const view = new DataView(buffer);
    expect(view.getUint8(0)).toBe(1);
    expect(view.getUint16(2, true)).toBe(1);
    expect(view.getUint32(4, true)).toBe(0x12345678);
    expect(view.getFloat32(16 + 12, true)).toBeCloseTo(0.25);
    expect(view.getFloat32(16 + 20, true)).toBeCloseTo(0.5);
    expect(view.getFloat32(16 + 24, true)).toBeCloseTo(-30);
  });

  it("clamps invalid browser data before transport", () => {
    const buffer = encodePointerBatch({
      batchSequence: 1,
      clientSendTimeMs: 0,
      samples: [
        {
          deviceType: DeviceType.Pen,
          action: PointerAction.Move,
          flags: 0,
          pointerId: 1,
          sampleSequence: 1,
          xNorm: -4,
          yNorm: 5,
          pressure: Number.NaN,
          tiltXDeg: -500,
          tiltYDeg: 500,
          twistDeg: -15,
          clientTimeMs: 0,
        },
      ],
    });
    const view = new DataView(buffer);
    expect(view.getFloat32(28, true)).toBe(0);
    expect(view.getFloat32(32, true)).toBe(1);
    expect(view.getFloat32(36, true)).toBe(0);
    expect(view.getFloat32(40, true)).toBe(-90);
    expect(view.getFloat32(44, true)).toBe(90);
    expect(view.getFloat32(48, true)).toBe(345);
  });
});
