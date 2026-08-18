import { beforeEach, describe, expect, it } from "vitest";
import { PointerEngine, type PointerTelemetry } from "./pointer-engine";
import { PointerAction } from "./protocol";

const HEADER_BYTES = 16;
const SAMPLE_BYTES = 44;

interface DecodedPacket {
  batchSequence: number;
  sampleCount: number;
  actions: number[];
  sampleSequences: number[];
  flags: number[];
  pressures: number[];
  tiltX: number[];
  tiltY: number[];
}

describe("PointerEngine", () => {
  let overlay: HTMLCanvasElement;
  let video: HTMLVideoElement;
  let packets: ArrayBuffer[];

  beforeEach(() => {
    document.body.innerHTML = `<canvas id="overlay"></canvas><video id="video"></video>`;
    overlay = document.querySelector<HTMLCanvasElement>("#overlay")!;
    video = document.querySelector<HTMLVideoElement>("#video")!;
    packets = [];
    Object.defineProperty(video, "videoWidth", { value: 1920 });
    Object.defineProperty(video, "videoHeight", { value: 1080 });
    overlay.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 1000, bottom: 750, width: 1000, height: 750, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    overlay.getContext = () => null;
  });

  it("preserves a pressure/tilt ramp from coalesced events without duplicating the parent", () => {
    let telemetry: PointerTelemetry | undefined;
    const engine = new PointerEngine({
      overlay,
      video,
      send: (packet) => packets.push(packet),
      getFitMode: () => "fit",
      getTouchEnabled: () => false,
      onTelemetry: (value) => {
        telemetry = value;
      },
    });

    overlay.dispatchEvent(pointer("pointerdown", 0.1, -10, 15, 1, 200, 250));
    const coalesced = [
      pointer("pointermove", 0.2, -30, 30, 1, 220, 260),
      pointer("pointermove", 0.45, -10, 10, 1, 240, 270),
      pointer("pointermove", 0.75, 20, -20, 1, 260, 280),
      pointer("pointermove", 1.0, 60, -60, 1, 280, 290),
    ];
    overlay.dispatchEvent(pointer("pointermove", 1.0, 60, -60, 1, 280, 290, coalesced));
    overlay.dispatchEvent(pointer("pointerup", 0, 60, -60, 0, 280, 290));

    expect(packets).toHaveLength(3);
    expect(decode(packets[0]!)).toMatchObject({
      batchSequence: 0,
      sampleCount: 1,
      actions: [PointerAction.Down],
      sampleSequences: [0],
    });
    expect(decode(packets[1]!)).toEqual({
      batchSequence: 1,
      sampleCount: 4,
      actions: [PointerAction.Move, PointerAction.Move, PointerAction.Move, PointerAction.Move],
      sampleSequences: [1, 2, 3, 4],
      flags: [1, 1, 1, 1],
      pressures: expectCloseArray([0.2, 0.45, 0.75, 1.0]),
      tiltX: [-30, -10, 20, 60],
      tiltY: [30, 10, -20, -60],
    });
    expect(decode(packets[2]!)).toMatchObject({
      batchSequence: 2,
      sampleCount: 1,
      actions: [PointerAction.Up],
      sampleSequences: [5],
    });
    expect(telemetry?.coalesced).toBe(0);
    engine.dispose();
  });

  it("encodes the sample volume of a ten-minute 240 Hz stroke with continuous sequences", () => {
    const engine = new PointerEngine({
      overlay,
      video,
      send: (packet) => packets.push(packet),
      getFitMode: () => "fit",
      getTouchEnabled: () => false,
    });
    overlay.dispatchEvent(pointer("pointerdown", 0.1, 0, 60, 1, 200, 250));

    const moveSamples = 144_000;
    const samplesPerEvent = 64;
    for (let start = 0; start < moveSamples; start += samplesPerEvent) {
      const coalesced = Array.from({ length: samplesPerEvent }, (_, offset) => {
        const sequence = start + offset;
        const phase = (sequence / moveSamples) * Math.PI * 24;
        return pointer(
          "pointermove",
          0.05 + 0.95 * (0.5 + 0.5 * Math.sin(phase)),
          60 * Math.sin(phase),
          60 * Math.cos(phase),
          1,
          200 + (sequence % 600),
          250 + 80 * Math.sin(phase),
        );
      });
      overlay.dispatchEvent(pointer("pointermove", 0.5, 0, 0, 1, 400, 300, coalesced));
    }
    overlay.dispatchEvent(pointer("pointerup", 0, 0, 0, 0, 800, 300));

    const decoded = packets.map(decode);
    expect(decoded.reduce((total, packet) => total + packet.sampleCount, 0)).toBe(moveSamples + 2);
    expect(decoded.every((packet, index) => packet.batchSequence === index)).toBe(true);
    expect(decoded.at(-1)?.sampleSequences).toEqual([moveSamples + 1]);
    expect(decoded.at(-1)?.actions).toEqual([PointerAction.Up]);
    engine.dispose();
  });
});

function pointer(
  type: string,
  pressure: number,
  tiltX: number,
  tiltY: number,
  buttons: number,
  clientX: number,
  clientY: number,
  coalesced: Event[] = [],
): PointerEvent {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    pointerType: { value: "pen" },
    pointerId: { value: 17 },
    pressure: { value: pressure },
    tiltX: { value: tiltX },
    tiltY: { value: tiltY },
    twist: { value: 123 },
    buttons: { value: buttons },
    clientX: { value: clientX },
    clientY: { value: clientY },
    getCoalescedEvents: { value: () => coalesced },
    getPredictedEvents: { value: () => [] },
  });
  return event as PointerEvent;
}

function decode(packet: ArrayBuffer): DecodedPacket {
  const view = new DataView(packet);
  const sampleCount = view.getUint16(2, true);
  const decoded: DecodedPacket = {
    batchSequence: view.getUint32(4, true),
    sampleCount,
    actions: [],
    sampleSequences: [],
    flags: [],
    pressures: [],
    tiltX: [],
    tiltY: [],
  };
  for (let index = 0; index < sampleCount; index += 1) {
    const offset = HEADER_BYTES + index * SAMPLE_BYTES;
    decoded.actions.push(view.getUint8(offset + 1));
    decoded.flags.push(view.getUint16(offset + 2, true));
    decoded.sampleSequences.push(view.getUint32(offset + 8, true));
    decoded.pressures.push(view.getFloat32(offset + 20, true));
    decoded.tiltX.push(view.getFloat32(offset + 24, true));
    decoded.tiltY.push(view.getFloat32(offset + 28, true));
  }
  return decoded;
}

function expectCloseArray(values: number[]): unknown {
  return values.map((value) => expect.closeTo(value, 5));
}
