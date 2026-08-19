import { describe, expect, it, vi } from "vitest";
import { outgoingDownloadUrl, sha256Hex, uploadFile } from "./file-transfer";

describe("file transfer", () => {
  it("hashes chunks without requiring secure-context Web Crypto", () => {
    expect(sha256Hex(new TextEncoder().encode("").buffer)).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    expect(sha256Hex(new TextEncoder().encode("abc").buffer)).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });

  it("encodes queued identifiers in download URLs", () => {
    expect(outgoingDownloadUrl("id with spaces")).toBe("/api/files/outbox/id%20with%20spaces/download");
  });

  it("uploads sequential verified chunks and reports progress", async () => {
    const calls: Array<{ path: string; init?: RequestInit }> = [];
    const responses = [
      { upload_id: "upload-1", name: "sample.bin", size: 5, uploaded_bytes: 0, chunk_size_bytes: 3 },
      { upload_id: "upload-1", uploaded_bytes: 3, total_bytes: 5 },
      { upload_id: "upload-1", uploaded_bytes: 5, total_bytes: 5 },
      { upload_id: "upload-1", name: "sample.bin", size: 5, sha256: "done" },
    ];
    const fetcher = vi.fn(async (path: string | URL | Request, init?: RequestInit) => {
      calls.push({ path: String(path), init });
      return new Response(JSON.stringify(responses.shift()), {
        status: calls.length === 1 ? 201 : 200,
        headers: { "content-type": "application/json" },
      });
    }) as unknown as typeof fetch;
    const progress = vi.fn();
    const fileBytes = new Uint8Array([1, 2, 3, 4, 5]);
    const file = {
      name: "sample.bin",
      type: "application/octet-stream",
      size: fileBytes.length,
      slice: (start: number, end: number) => ({
        arrayBuffer: async () => fileBytes.slice(start, end).buffer,
      }),
    } as unknown as File;
    const result = await uploadFile(
      file,
      { onProgress: progress },
      undefined,
      fetcher,
    );
    expect(result.sha256).toBe("done");
    expect(calls.map((call) => call.init?.method)).toEqual(["POST", "PUT", "PUT", "POST"]);
    expect(calls[1]!.init?.headers).toMatchObject({ "x-nfidb-offset": "0" });
    expect(calls[2]!.init?.headers).toMatchObject({ "x-nfidb-offset": "3" });
    expect(progress).toHaveBeenLastCalledWith(5, 5);
  });

  it("cleans up the server staging file when canceled", async () => {
    const controller = new AbortController();
    const methods: Array<string | undefined> = [];
    const fetcher = vi.fn(async (_path: string | URL | Request, init?: RequestInit) => {
      methods.push(init?.method);
      if (methods.length === 1) {
        controller.abort();
        return new Response(
          JSON.stringify({ upload_id: "upload-2", name: "x", size: 1, uploaded_bytes: 0, chunk_size_bytes: 1 }),
          { status: 201 },
        );
      }
      return new Response(null, { status: 204 });
    }) as unknown as typeof fetch;
    await expect(
      uploadFile(new File(["x"], "x"), {}, controller.signal, fetcher),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(methods).toEqual(["POST", "DELETE"]);
  });

  it("reconciles the server offset when a chunk response is lost", async () => {
    const fileBytes = new Uint8Array([1, 2, 3, 4, 5]);
    const file = {
      name: "resume.bin",
      type: "application/octet-stream",
      size: fileBytes.length,
      slice: (start: number, end: number) => ({ arrayBuffer: async () => fileBytes.slice(start, end).buffer }),
    } as unknown as File;
    const calls: Array<{ path: string; method: string }> = [];
    let firstChunk = true;
    const fetcher = vi.fn(async (path: string | URL | Request, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      calls.push({ path: String(path), method });
      if (method === "POST" && String(path).endsWith("/uploads")) {
        return new Response(JSON.stringify({ upload_id: "resume-1", name: "resume.bin", size: 5, uploaded_bytes: 0, chunk_size_bytes: 3 }), { status: 201 });
      }
      if (method === "PUT" && firstChunk) {
        firstChunk = false;
        throw new TypeError("response lost");
      }
      if (method === "GET") {
        return new Response(JSON.stringify({ upload_id: "resume-1", uploaded_bytes: 3, total_bytes: 5 }));
      }
      if (method === "PUT") {
        return new Response(JSON.stringify({ upload_id: "resume-1", uploaded_bytes: 5, total_bytes: 5 }));
      }
      return new Response(JSON.stringify({ upload_id: "resume-1", name: "resume.bin", size: 5, sha256: "resumed" }));
    }) as unknown as typeof fetch;
    const retry = vi.fn();
    const complete = await uploadFile(file, { onRetry: retry }, undefined, fetcher);
    expect(complete.sha256).toBe("resumed");
    expect(retry).toHaveBeenCalledOnce();
    expect(calls.map(({ method }) => method)).toEqual(["POST", "PUT", "GET", "PUT", "POST"]);
  });

  it("retries idempotent creation and completion after lost responses", async () => {
    const methods: string[] = [];
    let creates = 0;
    let completes = 0;
    let requestedId = "";
    const fetcher = vi.fn(async (path: string | URL | Request, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      methods.push(method);
      if (method === "POST" && String(path).endsWith("/uploads")) {
        creates += 1;
        requestedId = JSON.parse(String(init?.body)).upload_id as string;
        if (creates === 1) {
          throw new TypeError("create response lost");
        }
        return new Response(JSON.stringify({
          upload_id: requestedId,
          name: "durable.bin",
          size: 1,
          uploaded_bytes: 0,
          chunk_size_bytes: 1,
        }), { status: 201 });
      }
      if (method === "PUT") {
        return new Response(JSON.stringify({ upload_id: requestedId, uploaded_bytes: 1, total_bytes: 1 }));
      }
      if (method === "POST" && String(path).endsWith("/complete")) {
        completes += 1;
        if (completes === 1) {
          throw new TypeError("complete response lost");
        }
        return new Response(JSON.stringify({ upload_id: requestedId, name: "durable.bin", size: 1, sha256: "ok" }));
      }
      return new Response(null, { status: 204 });
    }) as unknown as typeof fetch;
    const retry = vi.fn();
    const bytes = new Uint8Array([7]);
    const file = {
      name: "durable.bin",
      type: "application/octet-stream",
      size: bytes.length,
      slice: (start: number, end: number) => ({ arrayBuffer: async () => bytes.slice(start, end).buffer }),
    } as unknown as File;
    const result = await uploadFile(file, { onRetry: retry }, undefined, fetcher);
    expect(result.sha256).toBe("ok");
    expect(requestedId).toMatch(/^[0-9a-f-]{36}$/);
    expect(creates).toBe(2);
    expect(completes).toBe(2);
    expect(retry).toHaveBeenCalledTimes(2);
    expect(methods).toEqual(["POST", "POST", "PUT", "POST", "POST"]);
  });
});
