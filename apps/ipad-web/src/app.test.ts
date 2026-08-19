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
      "2 files are ready from Windows. Open Files to download.",
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
