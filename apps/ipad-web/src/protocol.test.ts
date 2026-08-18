import { describe, expect, it } from "vitest";
import {
  BATCH_HEADER_BYTES,
  COMMAND_MESSAGE,
  DeviceType,
  KEYBOARD_MESSAGE,
  KeyAction,
  PointerAction,
  RemoteCommand,
  SAMPLE_BYTES,
  TEXT_MESSAGE,
  WHEEL_MESSAGE,
  encodeCommandInput,
  encodeKeyboardInput,
  encodePointerBatch,
  encodeTextInput,
  encodeWheelInput,
} from "./protocol";

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

  it("encodes mouse, wheel, keyboard, Unicode text, and commands", () => {
    const mouse = encodePointerBatch({
      batchSequence: 2,
      clientSendTimeMs: 3,
      samples: [{
        deviceType: DeviceType.Mouse,
        action: PointerAction.Hover,
        flags: 0,
        pointerId: 4,
        sampleSequence: 5,
        xNorm: 0.2,
        yNorm: 0.8,
        pressure: 0,
        tiltXDeg: 0,
        tiltYDeg: 0,
        twistDeg: 0,
        clientTimeMs: 6,
      }],
    });
    expect(new DataView(mouse).getUint8(16)).toBe(DeviceType.Mouse);

    const wheel = encodeWheelInput({
      modifiers: 5,
      sequence: 6,
      xNorm: 0.25,
      yNorm: 0.75,
      deltaX: -4,
      deltaY: 12,
      clientTimeMs: 7,
    });
    const wheelView = new DataView(wheel);
    expect(wheel.byteLength).toBe(32);
    expect(wheelView.getUint8(1)).toBe(WHEEL_MESSAGE);
    expect(wheelView.getFloat32(20, true)).toBe(12);

    const keyboard = encodeKeyboardInput({
      action: KeyAction.Down,
      location: 1,
      repeat: false,
      modifiers: 4,
      sequence: 8,
      clientTimeMs: 9,
      code: "AltLeft",
      key: "Alt",
    });
    const keyboardView = new DataView(keyboard);
    expect(keyboardView.getUint8(1)).toBe(KEYBOARD_MESSAGE);
    expect(keyboardView.getUint16(20, true)).toBe(7);

    const text = encodeTextInput(10, 11, "A😀");
    expect(new DataView(text).getUint8(1)).toBe(TEXT_MESSAGE);
    expect(new DataView(text).getUint32(16, true)).toBe(5);

    const command = encodeCommandInput(RemoteCommand.AppNext, 12, 13);
    expect(command.byteLength).toBe(16);
    expect(new DataView(command).getUint8(1)).toBe(COMMAND_MESSAGE);
    expect(new DataView(command).getUint8(2)).toBe(RemoteCommand.AppNext);
  });

  it("rejects oversized keyboard and text fields", () => {
    expect(() => encodeKeyboardInput({
      action: KeyAction.Down,
      location: 0,
      repeat: false,
      modifiers: 0,
      sequence: 0,
      clientTimeMs: 0,
      code: "x".repeat(65),
      key: "x",
    })).toThrow(RangeError);
    expect(() => encodeTextInput(0, 0, "x".repeat(4097))).toThrow(RangeError);
  });
});
