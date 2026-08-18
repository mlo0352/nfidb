import { beforeEach, describe, expect, it } from "vitest";
import { RemoteInputEngine } from "./remote-input";
import { COMMAND_MESSAGE, KEYBOARD_MESSAGE, RemoteCommand, TEXT_MESSAGE, WHEEL_MESSAGE } from "./protocol";

describe("RemoteInputEngine", () => {
  let overlay: HTMLCanvasElement;
  let video: HTMLVideoElement;
  let input: HTMLTextAreaElement;
  let packets: ArrayBuffer[];

  beforeEach(() => {
    document.body.innerHTML = `<canvas id="overlay"></canvas><video id="video"></video><textarea id="input"></textarea>`;
    overlay = document.querySelector<HTMLCanvasElement>("#overlay")!;
    video = document.querySelector<HTMLVideoElement>("#video")!;
    input = document.querySelector<HTMLTextAreaElement>("#input")!;
    packets = [];
    Object.defineProperty(video, "videoWidth", { value: 1920 });
    Object.defineProperty(video, "videoHeight", { value: 1080 });
    Object.defineProperty(overlay, "clientWidth", { value: 1000 });
    Object.defineProperty(overlay, "clientHeight", { value: 750 });
    overlay.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 1000, bottom: 750, width: 1000, height: 750, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  });

  it("maps Option+Tab to ordered Alt and Tab key packets", () => {
    const engine = createEngine();
    dispatchKey("keydown", "AltLeft", "Alt", { altKey: true });
    dispatchKey("keydown", "Tab", "Tab", { altKey: true });
    dispatchKey("keyup", "Tab", "Tab", { altKey: true });
    dispatchKey("keyup", "AltLeft", "Alt");

    expect(packets.map(messageType)).toEqual([
      KEYBOARD_MESSAGE,
      KEYBOARD_MESSAGE,
      KEYBOARD_MESSAGE,
      KEYBOARD_MESSAGE,
    ]);
    expect(packets.map(decodeKeyboardCode)).toEqual(["AltLeft", "Tab", "Tab", "AltLeft"]);
    expect(packets.map((packet) => new DataView(packet).getUint8(2))).toEqual([1, 1, 2, 2]);
    engine.dispose();
  });

  it("maps Delete to Backspace except for Control+Option+Delete", () => {
    const engine = createEngine();
    dispatchKey("keydown", "Delete", "Delete");
    dispatchKey("keyup", "Delete", "Delete");
    dispatchKey("keydown", "Backspace", "Backspace", { ctrlKey: true, altKey: true });
    dispatchKey("keyup", "Backspace", "Backspace", { ctrlKey: true, altKey: true });

    expect(packets.map(decodeKeyboardCode)).toEqual(["Backspace", "Backspace", "Delete", "Delete"]);
    engine.dispose();
  });

  it("sends hardware characters as physical keys and textarea input as Unicode text", () => {
    const engine = createEngine();
    dispatchKey("keydown", "KeyA", "A", { shiftKey: true });
    dispatchKey("keyup", "KeyA", "A", { shiftKey: true });
    input.focus();
    input.value = "hello 😀";
    input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: "hello 😀" }));

    expect(packets.map(messageType)).toEqual([KEYBOARD_MESSAGE, KEYBOARD_MESSAGE, TEXT_MESSAGE]);
    expect(packets.slice(0, 2).map(decodeKeyboardCode)).toEqual(["KeyA", "KeyA"]);
    expect(decodeText(packets[2]!)).toBe("hello 😀");
    expect(input.value).toBe("");
    engine.dispose();
  });

  it("normalizes wheel input over the remote video", () => {
    const engine = createEngine();
    overlay.dispatchEvent(new WheelEvent("wheel", {
      bubbles: true,
      cancelable: true,
      clientX: 500,
      clientY: 375,
      deltaX: 2,
      deltaY: 10,
      deltaMode: WheelEvent.DOM_DELTA_PIXEL,
    }));

    expect(packets).toHaveLength(1);
    expect(messageType(packets[0]!)).toBe(WHEEL_MESSAGE);
    const view = new DataView(packets[0]!);
    expect(view.getFloat32(8, true)).toBeCloseTo(0.5);
    expect(view.getFloat32(12, true)).toBeCloseTo(0.5);
    expect(view.getFloat32(20, true)).toBe(10);
    engine.dispose();
  });

  it("maps three-finger swipes to semantic Windows commands", () => {
    const engine = createEngine();
    for (let id = 1; id <= 3; id += 1) {
      overlay.dispatchEvent(pointer("pointerdown", id, 300, 300));
    }
    for (let id = 1; id <= 3; id += 1) {
      overlay.dispatchEvent(pointer("pointermove", id, 400, 300));
    }

    const commands = packets.filter((packet) => messageType(packet) === COMMAND_MESSAGE);
    expect(commands).toHaveLength(1);
    expect(new DataView(commands[0]!).getUint8(2)).toBe(RemoteCommand.AppNext);
    engine.dispose();
  });

  function createEngine(): RemoteInputEngine {
    const engine = new RemoteInputEngine({
      overlay,
      video,
      send: (packet) => packets.push(packet),
      getFitMode: () => "fit",
      getTouchEnabled: () => false,
      getGesturesEnabled: () => true,
    });
    engine.attachTextInput(input);
    return engine;
  }
});

function dispatchKey(
  type: "keydown" | "keyup",
  code: string,
  key: string,
  modifiers: Pick<KeyboardEventInit, "altKey" | "ctrlKey" | "metaKey" | "shiftKey"> = {},
): void {
  window.dispatchEvent(new KeyboardEvent(type, {
    bubbles: true,
    cancelable: true,
    code,
    key,
    ...modifiers,
  }));
}

function pointer(type: string, pointerId: number, clientX: number, clientY: number): PointerEvent {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    pointerType: { value: "touch" },
    pointerId: { value: pointerId },
    clientX: { value: clientX },
    clientY: { value: clientY },
  });
  return event as PointerEvent;
}

function messageType(packet: ArrayBuffer): number {
  return new DataView(packet).getUint8(1);
}

function decodeKeyboardCode(packet: ArrayBuffer): string {
  const view = new DataView(packet);
  const length = view.getUint16(20, true);
  return new TextDecoder().decode(new Uint8Array(packet, 24, length));
}

function decodeText(packet: ArrayBuffer): string {
  const view = new DataView(packet);
  const length = view.getUint32(16, true);
  return new TextDecoder().decode(new Uint8Array(packet, 20, length));
}
