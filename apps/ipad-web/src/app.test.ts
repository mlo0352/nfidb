import { afterEach, describe, expect, it, vi } from "vitest";
import { NfidbApp } from "./app";

describe("video startup recovery", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    localStorage.clear();
  });

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

describe("outbound file notifications", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("notifies for a multi-file delivery, badges the queue, and renders auto-clearing downloads", async () => {
    const root = document.createElement("div");
    root.innerHTML = `
      <div id="connectionNotice" hidden></div>
      <span id="filesBadge" hidden>0</span>
      <div id="filesContent"></div>`;
    const app = new NfidbApp(root);
    const internal = app as unknown as { refreshFileListing: () => Promise<void> };
    const listing = (outbox: Array<{ id: string; name: string }>) => ({
      enabled: true,
      max_file_size_bytes: 1024,
      chunk_size_bytes: 1024,
      rate_limit_mbps: 32,
      pause_while_drawing: true,
      inbox_name: "NFiDB Inbox",
      outbox: outbox.map((file) => ({
        ...file,
        size: 12,
        mime: "text/plain",
        queued_epoch_ms: 1,
        sha256: "1234567890abcdef",
      })),
      active_uploads: [],
      recent: outbox.length === 0 ? [] : [{
        direction: "windows-to-ipad",
        name: "first.txt",
        bytes: 12,
        duration_ms: 1,
        average_mbps: 1,
        sha256: null,
        completed_epoch_ms: 1,
        status: "completed",
      }],
      stats: {
        upload_bytes: 0,
        download_bytes: 0,
        uploads_completed: 0,
        downloads_completed: 0,
        canceled_transfers: 0,
        failed_transfers: 0,
        active_uploads: 0,
        active_downloads: 0,
        upload_mbps: 0,
        download_mbps: 0,
      },
    });
    const responses = [listing([]), listing([
      { id: "one", name: "first.txt" },
      { id: "two", name: "second.txt" },
    ])];
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(responses.shift()))));

    await internal.refreshFileListing();
    await internal.refreshFileListing();

    expect(root.querySelector("#connectionNotice")?.textContent).toBe(
      "2 files are ready from the host. Open Files to download.",
    );
    const badge = root.querySelector<HTMLElement>("#filesBadge");
    expect(badge?.textContent).toBe("2");
    expect(badge?.hidden).toBe(false);
    const links = Array.from(root.querySelectorAll<HTMLAnchorElement>("[data-download-id]"));
    expect(links.map((link) => link.getAttribute("href"))).toEqual([
      "/api/files/outbox/one/download?remove=1",
      "/api/files/outbox/two/download?remove=1",
    ]);
    expect(root.querySelector("#downloadAllButton")?.textContent).toContain("2");
    expect(root.querySelector("#filesContent")?.textContent).toContain("Windows to iPad");
    expect(root.querySelector("#filesContent")?.textContent).not.toContain("→");
  });
});

describe("surface controls", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("keeps the complete surface inside the browser visual viewport", () => {
    const root = document.createElement("div");
    const viewport = new EventTarget() as VisualViewport;
    Object.defineProperties(viewport, {
      offsetLeft: { configurable: true, value: 7 },
      offsetTop: { configurable: true, value: 116 },
      width: { configurable: true, value: 980 },
      height: { configurable: true, value: 620 },
    });
    const originalViewport = Object.getOwnPropertyDescriptor(window, "visualViewport");
    Object.defineProperty(window, "visualViewport", { configurable: true, value: viewport });
    try {
      const app = new NfidbApp(root);
      const internal = app as unknown as {
        status: { mode: string };
        renderSurface: () => void;
      };
      internal.status = { mode: "display-and-input" };
      internal.renderSurface();

      const surface = root.querySelector<HTMLElement>("#surface")!;
      expect(surface.style.getPropertyValue("--nfidb-viewport-left")).toBe("7px");
      expect(surface.style.getPropertyValue("--nfidb-viewport-top")).toBe("116px");
      expect(surface.style.getPropertyValue("--nfidb-viewport-width")).toBe("980px");
      expect(surface.style.getPropertyValue("--nfidb-viewport-height")).toBe("620px");
      expect(surface.style.getPropertyValue("--nfidb-video-top-inset")).toBe("48px");
    } finally {
      if (originalViewport) {
        Object.defineProperty(window, "visualViewport", originalViewport);
      } else {
        Reflect.deleteProperty(window, "visualViewport");
      }
    }
  });

  it("keeps controls closed during drawing and opens them only from the explicit button", () => {
    vi.useFakeTimers();
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const internal = app as unknown as {
      status: { mode: string };
      renderSurface: () => void;
      hideToolbar: () => void;
      scheduleToolbarHide: () => void;
    };
    internal.status = { mode: "display-and-input" };
    internal.renderSurface();
    internal.hideToolbar();

    root.querySelector("#interactionOverlay")?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    internal.scheduleToolbarHide();
    expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(false);

    root.querySelector<HTMLButtonElement>("#toolbarReveal")?.dispatchEvent(
      new Event("pointerdown", { bubbles: true }),
    );
    expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(true);
    expect(root.querySelector("#toolbarReveal")?.getAttribute("aria-expanded")).toBe("true");
    vi.advanceTimersByTime(10_000);
    expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(true);

    root.querySelector("#interactionOverlay")?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(false);
    expect(root.querySelector("#toolbarReveal")?.getAttribute("aria-expanded")).toBe("false");
  });

  it("opens controls from WebKit touch-end even when no click is synthesized", () => {
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const internal = app as unknown as {
      status: { mode: string };
      renderSurface: () => void;
      hideToolbar: () => void;
    };
    internal.status = { mode: "display-and-input" };
    internal.renderSurface();
    internal.hideToolbar();

    const event = new Event("touchend", { bubbles: true, cancelable: true });
    root.querySelector<HTMLButtonElement>("#toolbarReveal")!.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(true);
    expect(root.querySelector("#toolbarReveal")?.getAttribute("aria-expanded")).toBe("true");
  });

  it("closes the controls and toggles fullscreen in both directions", async () => {
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const internal = app as unknown as {
      status: { mode: string };
      renderSurface: () => void;
    };
    internal.status = { mode: "display-and-input" };
    internal.renderSurface();
    const surface = root.querySelector<HTMLElement>("#surface")!;
    let fullscreenElement: Element | null = null;
    const requestFullscreen = vi.fn(async () => {
      fullscreenElement = surface;
    });
    const exitFullscreen = vi.fn(async () => {
      fullscreenElement = null;
    });
    const originalFullscreenElement = Object.getOwnPropertyDescriptor(document, "fullscreenElement");
    const originalExitFullscreen = Object.getOwnPropertyDescriptor(document, "exitFullscreen");
    Object.defineProperty(document, "fullscreenElement", {
      configurable: true,
      get: () => fullscreenElement,
    });
    Object.defineProperty(document, "exitFullscreen", {
      configurable: true,
      value: exitFullscreen,
    });
    Object.defineProperty(surface, "requestFullscreen", {
      configurable: true,
      value: requestFullscreen,
    });

    try {
      root.querySelector<HTMLButtonElement>("#statsButton")!.click();
      expect(root.querySelector<HTMLElement>("#statsPanel")!.hidden).toBe(false);
      root.querySelector<HTMLButtonElement>("#fullscreenButton")!.click();
      await Promise.resolve();
      expect(requestFullscreen).toHaveBeenCalledOnce();
      expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(false);
      expect(root.querySelector<HTMLElement>("#statsPanel")!.hidden).toBe(true);
      expect(root.querySelector("#statsButton")?.getAttribute("aria-pressed")).toBe("false");

      root.querySelector<HTMLButtonElement>("#toolbarReveal")!.click();
      root.querySelector<HTMLButtonElement>("#statsButton")!.click();
      root.querySelector<HTMLButtonElement>("#statsClose")!.click();
      expect(root.querySelector<HTMLElement>("#statsPanel")!.hidden).toBe(true);

      root.querySelector<HTMLButtonElement>("#fullscreenButton")!.click();
      await Promise.resolve();
      expect(exitFullscreen).toHaveBeenCalledOnce();
      expect(root.querySelector("#toolbar")?.classList.contains("visible")).toBe(false);
    } finally {
      if (originalFullscreenElement) {
        Object.defineProperty(document, "fullscreenElement", originalFullscreenElement);
      } else {
        Reflect.deleteProperty(document, "fullscreenElement");
      }
      if (originalExitFullscreen) {
        Object.defineProperty(document, "exitFullscreen", originalExitFullscreen);
      } else {
        Reflect.deleteProperty(document, "exitFullscreen");
      }
    }
  });

  it("requests a keyframe for a live stall and rebuilds a persistently stuck peer", async () => {
    vi.useFakeTimers();
    const now = vi.spyOn(performance, "now").mockReturnValue(5_000);
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const send = vi.fn();
    const reconnect = vi.fn(async () => undefined);
    const peer = { connectionState: "connected" } as RTCPeerConnection;
    const internal = app as unknown as {
      status: { mode: string };
      peer: RTCPeerConnection;
      socket: { readyState: number; send: (message: string) => void };
      metrics: { capture_fps: number; encoded_fps: number };
      firstVideoFrameAtMs: number;
      lastVideoProgressAtMs: number;
      lastHostMetricsAtMs: number;
      videoStallRecoveryAttempts: number;
      renderSurface: () => void;
      connectVideo: () => Promise<void>;
      startVideoStallWatchdog: (peer: RTCPeerConnection, video: HTMLVideoElement) => void;
      stopVideoStallWatchdog: () => void;
    };
    internal.status = { mode: "display-and-input" };
    internal.renderSurface();
    internal.peer = peer;
    internal.socket = { readyState: WebSocket.OPEN, send };
    internal.metrics = { capture_fps: 60, encoded_fps: 60 };
    internal.firstVideoFrameAtMs = 500;
    internal.lastVideoProgressAtMs = 1_000;
    internal.lastHostMetricsAtMs = 4_500;
    internal.connectVideo = reconnect;
    const video = root.querySelector<HTMLVideoElement>("#remoteVideo")!;
    Object.defineProperty(video, "play", { configurable: true, value: vi.fn(async () => undefined) });

    internal.startVideoStallWatchdog(peer, video);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(send).toHaveBeenCalledWith(JSON.stringify({ type: "request-keyframe" }));
    expect(reconnect).not.toHaveBeenCalled();

    internal.videoStallRecoveryAttempts = 3;
    now.mockReturnValue(10_000);
    internal.lastHostMetricsAtMs = 9_500;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(reconnect).toHaveBeenCalledOnce();
    internal.stopVideoStallWatchdog();
  });

  it("rebuilds an active stalled stream even when neither control channel can request a keyframe", async () => {
    vi.useFakeTimers();
    vi.spyOn(performance, "now").mockReturnValue(12_000);
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const reconnect = vi.fn(async () => undefined);
    const peer = { connectionState: "connected" } as RTCPeerConnection;
    const internal = app as unknown as {
      status: { mode: string };
      peer: RTCPeerConnection;
      socket: { readyState: number; send: (message: string) => void };
      firstVideoFrameAtMs: number;
      lastVideoProgressAtMs: number;
      lastHostMetricsAtMs: number;
      metrics: { capture_fps: number; encoded_fps: number };
      renderSurface: () => void;
      connectVideo: () => Promise<void>;
      startVideoStallWatchdog: (peer: RTCPeerConnection, video: HTMLVideoElement) => void;
      stopVideoStallWatchdog: () => void;
    };
    internal.status = { mode: "display-and-input" };
    internal.renderSurface();
    internal.peer = peer;
    internal.socket = { readyState: WebSocket.CLOSED, send: vi.fn() };
    internal.firstVideoFrameAtMs = 500;
    internal.lastVideoProgressAtMs = 1_000;
    internal.lastHostMetricsAtMs = 11_500;
    internal.metrics = { capture_fps: 30, encoded_fps: 30 };
    internal.connectVideo = reconnect;
    const video = root.querySelector<HTMLVideoElement>("#remoteVideo")!;
    Object.defineProperty(video, "play", { configurable: true, value: vi.fn(async () => undefined) });

    internal.startVideoStallWatchdog(peer, video);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(reconnect).toHaveBeenCalledOnce();
    internal.stopVideoStallWatchdog();
  });

  it("does not rebuild a healthy static macOS screen", async () => {
    vi.useFakeTimers();
    vi.spyOn(performance, "now").mockReturnValue(12_000);
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const reconnect = vi.fn(async () => undefined);
    const peer = { connectionState: "connected" } as RTCPeerConnection;
    const internal = app as unknown as {
      status: { mode: string };
      peer: RTCPeerConnection;
      socket: { readyState: number; send: (message: string) => void };
      firstVideoFrameAtMs: number;
      lastVideoProgressAtMs: number;
      lastHostMetricsAtMs: number;
      metrics: { capture_fps: number; encoded_fps: number };
      renderSurface: () => void;
      connectVideo: () => Promise<void>;
      startVideoStallWatchdog: (peer: RTCPeerConnection, video: HTMLVideoElement) => void;
      stopVideoStallWatchdog: () => void;
    };
    internal.status = { mode: "display-and-input" };
    internal.renderSurface();
    internal.peer = peer;
    internal.socket = { readyState: WebSocket.OPEN, send: vi.fn() };
    internal.firstVideoFrameAtMs = 500;
    internal.lastVideoProgressAtMs = 1_000;
    internal.lastHostMetricsAtMs = 11_500;
    internal.metrics = { capture_fps: 0, encoded_fps: 0 };
    internal.connectVideo = reconnect;
    const video = root.querySelector<HTMLVideoElement>("#remoteVideo")!;
    Object.defineProperty(video, "play", { configurable: true, value: vi.fn(async () => undefined) });

    internal.startVideoStallWatchdog(peer, video);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(reconnect).not.toHaveBeenCalled();
    expect(internal.socket.send).not.toHaveBeenCalled();
    internal.stopVideoStallWatchdog();
  });

  it("accepts Safari frame callbacks even when mediaTime remains unchanged", () => {
    const now = vi.spyOn(performance, "now");
    const app = new NfidbApp(document.createElement("div"));
    const internal = app as unknown as {
      lastVideoProgressAtMs: number;
      markFirstVideoFrame: (mediaTimeSeconds?: number) => void;
    };

    now.mockReturnValue(1_000);
    internal.markFirstVideoFrame(4.25);
    expect(internal.lastVideoProgressAtMs).toBe(1_000);
    now.mockReturnValue(2_000);
    internal.markFirstVideoFrame(4.25);
    expect(internal.lastVideoProgressAtMs).toBe(2_000);
    now.mockReturnValue(3_000);
    internal.markFirstVideoFrame(4.5);
    expect(internal.lastVideoProgressAtMs).toBe(3_000);
  });

  it("updates the host-authoritative touch gate before claiming touch is enabled", async () => {
    const root = document.createElement("div");
    const app = new NfidbApp(root);
    const internal = app as unknown as {
      status: { mode: string };
      inputControl: { revision: number; settings: { touch_enabled: boolean; gestures_enabled: boolean } };
      renderSurface: () => void;
      updateInputSettings: (changes: { touch_enabled: boolean }) => Promise<void>;
    };
    internal.status = { mode: "display-and-input" };
    internal.inputControl = { revision: 4, settings: { touch_enabled: false, gestures_enabled: true } };
    internal.renderSurface();
    const fetchMock = vi.fn(async (_path: string, init?: RequestInit) => {
      expect(init?.method).toBe("PUT");
      expect(JSON.parse(String(init?.body))).toEqual({
        base_revision: 4,
        settings: { touch_enabled: true, gestures_enabled: true },
      });
      return new Response(JSON.stringify({
        revision: 5,
        settings: { touch_enabled: true, gestures_enabled: true },
      }));
    });
    vi.stubGlobal("fetch", fetchMock);

    await internal.updateInputSettings({ touch_enabled: true });

    expect(fetchMock).toHaveBeenCalledWith("/api/input", expect.objectContaining({ method: "PUT" }));
    expect(root.querySelector("#touchButton")?.getAttribute("aria-pressed")).toBe("true");
    expect(root.querySelector("#touchButton")?.textContent).toBe("Touch on");
    expect(root.querySelector<HTMLButtonElement>("#gestureButton")?.disabled).toBe(true);
  });
});
