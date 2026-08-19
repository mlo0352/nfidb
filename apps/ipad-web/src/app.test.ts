import { afterEach, describe, expect, it, vi } from "vitest";
import { NfidbApp } from "./app";

describe("video startup recovery", () => {
  afterEach(() => vi.useRealTimers());

  it("requests another keyframe until the first frame is actually presented", () => {
    vi.useFakeTimers();
    const app = new NfidbApp(document.createElement("div"));
    const send = vi.fn();
    const peer = { connectionState: "connected" } as RTCPeerConnection;
    const internal = app as unknown as {
      peer: RTCPeerConnection;
      socket: WebSocket;
      startVideoRecovery: (peer: RTCPeerConnection) => void;
      markFirstVideoFrame: () => void;
      stopVideoRecovery: () => void;
    };
    internal.peer = peer;
    internal.socket = { readyState: WebSocket.OPEN, send } as unknown as WebSocket;

    internal.startVideoRecovery(peer);
    vi.advanceTimersByTime(1499);
    expect(send).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(send).toHaveBeenCalledWith(JSON.stringify({ type: "request-keyframe" }));

    internal.markFirstVideoFrame();
    vi.advanceTimersByTime(10_000);
    expect(send).toHaveBeenCalledTimes(1);
    internal.stopVideoRecovery();
  });

  it("falls back to the WebRTC DataChannel when the control socket is unavailable", () => {
    vi.useFakeTimers();
    const app = new NfidbApp(document.createElement("div"));
    const send = vi.fn();
    const peer = { connectionState: "connected" } as RTCPeerConnection;
    const internal = app as unknown as {
      peer: RTCPeerConnection;
      channel: RTCDataChannel;
      startVideoRecovery: (peer: RTCPeerConnection) => void;
      stopVideoRecovery: () => void;
    };
    internal.peer = peer;
    internal.channel = { readyState: "open", send } as unknown as RTCDataChannel;

    internal.startVideoRecovery(peer);
    vi.advanceTimersByTime(1500);
    expect(send).toHaveBeenCalledWith("request-keyframe");
    internal.stopVideoRecovery();
  });
});
