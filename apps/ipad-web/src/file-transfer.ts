export interface OutgoingFile {
  id: string;
  name: string;
  size: number;
  mime: string;
  queued_epoch_ms: number;
  sha256: string | null;
}

export interface ActiveUpload {
  id: string;
  name: string;
  size: number;
  received: number;
  started_epoch_ms: number;
}

export interface CompletedTransfer {
  direction: "ipad-to-windows" | "windows-to-ipad";
  name: string;
  bytes: number;
  duration_ms: number;
  average_mbps: number;
  sha256: string | null;
  completed_epoch_ms: number;
  status: string;
}

export interface TransferStats {
  upload_bytes: number;
  download_bytes: number;
  uploads_completed: number;
  downloads_completed: number;
  canceled_transfers: number;
  failed_transfers: number;
  active_uploads: number;
  active_downloads: number;
  upload_mbps: number;
  download_mbps: number;
}

export interface FileListing {
  enabled: boolean;
  max_file_size_bytes: number;
  chunk_size_bytes: number;
  rate_limit_mbps: number;
  pause_while_drawing: boolean;
  inbox_name: string;
  outbox: OutgoingFile[];
  active_uploads: ActiveUpload[];
  recent: CompletedTransfer[];
  stats: TransferStats;
}

interface UploadTicket {
  upload_id: string;
  name: string;
  size: number;
  uploaded_bytes: number;
  chunk_size_bytes: number;
}

interface UploadProgress {
  upload_id: string;
  uploaded_bytes: number;
  total_bytes: number;
}

export interface UploadComplete {
  upload_id: string;
  name: string;
  size: number;
  sha256: string;
}

export interface UploadCallbacks {
  onStarted?: (uploadId: string) => void;
  onProgress?: (uploaded: number, total: number) => void;
  onRetry?: (attempt: number) => void;
}

type FetchLike = typeof fetch;
type PreferenceStorage = Pick<Storage, "getItem" | "setItem">;

const AUTO_CLEAR_DOWNLOADS_KEY = "nfidb-auto-clear-downloads";

export class FileApiError extends Error {
  readonly status: number;
  readonly expectedOffset: number | null;

  constructor(message: string, status: number, expectedOffset: number | null = null) {
    super(message);
    this.name = "FileApiError";
    this.status = status;
    this.expectedOffset = expectedOffset;
  }
}

export async function getFileListing(fetcher: FetchLike = fetch): Promise<FileListing> {
  return requestJson<FileListing>(fetcher, "/api/files");
}

export async function removeOutgoingFile(id: string, fetcher: FetchLike = fetch): Promise<void> {
  const response = await fetcher(`/api/files/outbox/${encodeURIComponent(id)}`, {
    method: "DELETE",
    credentials: "same-origin",
  });
  if (!response.ok && response.status !== 404) {
    throw await responseError(response);
  }
}

export function outgoingDownloadUrl(id: string, removeAfterDownload = false): string {
  const path = `/api/files/outbox/${encodeURIComponent(id)}/download`;
  return removeAfterDownload ? `${path}?remove=1` : path;
}

export function loadAutoClearDownloads(storage?: PreferenceStorage): boolean {
  try {
    const saved = (storage ?? window.localStorage).getItem(AUTO_CLEAR_DOWNLOADS_KEY);
    return saved === null ? true : saved !== "false";
  } catch {
    return true;
  }
}

export function saveAutoClearDownloads(enabled: boolean, storage?: PreferenceStorage): void {
  try {
    (storage ?? window.localStorage).setItem(AUTO_CLEAR_DOWNLOADS_KEY, String(enabled));
  } catch {
    // Private browsing and managed-device policies can disable local storage.
  }
}

export async function uploadFile(
  file: File,
  callbacks: UploadCallbacks = {},
  signal?: AbortSignal,
  fetcher: FetchLike = fetch,
): Promise<UploadComplete> {
  throwIfAborted(signal);
  const requestedUploadId = createUploadId();
  let ticket: UploadTicket | null = null;
  try {
    ticket = await retryJson(
      () => requestJson<UploadTicket>(fetcher, "/api/files/uploads", {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ upload_id: requestedUploadId, name: file.name, mime: file.type, size: file.size }),
        signal,
      }),
      callbacks,
      signal,
    );
    callbacks.onStarted?.(ticket.upload_id);
    let offset = ticket.uploaded_bytes;
    callbacks.onProgress?.(offset, file.size);
    while (offset < file.size) {
      throwIfAborted(signal);
      const end = Math.min(file.size, offset + ticket.chunk_size_bytes);
      const chunk = await file.slice(offset, end).arrayBuffer();
      const checksum = sha256Hex(chunk);
      let sent = false;
      for (let attempt = 1; attempt <= 4 && !sent; attempt += 1) {
        throwIfAborted(signal);
        try {
          const progress = await requestJson<UploadProgress>(
            fetcher,
            `/api/files/uploads/${encodeURIComponent(ticket.upload_id)}`,
            {
              method: "PUT",
              credentials: "same-origin",
              headers: {
                "content-type": "application/octet-stream",
                "x-nfidb-offset": String(offset),
                "x-nfidb-chunk-sha256": checksum,
              },
              body: chunk,
              signal,
            },
          );
          offset = progress.uploaded_bytes;
          sent = true;
        } catch (error) {
          if (signal?.aborted) {
            throw abortError();
          }
          if (error instanceof FileApiError && error.expectedOffset !== null && error.expectedOffset !== offset) {
            offset = error.expectedOffset;
            sent = true;
            continue;
          }
          if (attempt >= 4 || (error instanceof FileApiError && error.status >= 400 && error.status < 500)) {
            throw error;
          }
          callbacks.onRetry?.(attempt);
          await abortableDelay(150 * 2 ** (attempt - 1), signal);
          try {
            const progress = await requestJson<UploadProgress>(
              fetcher,
              `/api/files/uploads/${encodeURIComponent(ticket.upload_id)}`,
              { credentials: "same-origin", signal },
            );
            if (progress.uploaded_bytes !== offset) {
              offset = progress.uploaded_bytes;
              sent = true;
            }
          } catch (statusError) {
            if (signal?.aborted) {
              throw abortError();
            }
            if (attempt >= 4) {
              throw statusError;
            }
          }
        }
      }
      callbacks.onProgress?.(offset, file.size);
    }
    return await retryJson(
      () => requestJson<UploadComplete>(
        fetcher,
        `/api/files/uploads/${encodeURIComponent(ticket!.upload_id)}/complete`,
        { method: "POST", credentials: "same-origin", signal },
      ),
      callbacks,
      signal,
    );
  } catch (error) {
    const cleanupId = ticket?.upload_id ?? requestedUploadId;
    await fetcher(`/api/files/uploads/${encodeURIComponent(cleanupId)}`, {
      method: "DELETE",
      credentials: "same-origin",
    }).catch(() => undefined);
    throw error;
  }
}

async function retryJson<T>(
  operation: () => Promise<T>,
  callbacks: UploadCallbacks,
  signal?: AbortSignal,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    throwIfAborted(signal);
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (signal?.aborted) {
        throw abortError();
      }
      if (attempt >= 4 || (error instanceof FileApiError && error.status >= 400 && error.status < 500)) {
        throw error;
      }
      callbacks.onRetry?.(attempt);
      await abortableDelay(150 * 2 ** (attempt - 1), signal);
    }
  }
  throw lastError;
}

function createUploadId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function sha256Hex(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  const bitLength = bytes.length * 8;
  const paddedLength = Math.ceil((bytes.length + 9) / 64) * 64;
  const padded = new Uint8Array(paddedLength);
  padded.set(bytes);
  padded[bytes.length] = 0x80;
  const view = new DataView(padded.buffer);
  const high = Math.floor(bitLength / 0x1_0000_0000);
  const low = bitLength >>> 0;
  view.setUint32(paddedLength - 8, high, false);
  view.setUint32(paddedLength - 4, low, false);

  const hash = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const words = new Uint32Array(64);
  for (let block = 0; block < paddedLength; block += 64) {
    for (let index = 0; index < 16; index += 1) {
      words[index] = view.getUint32(block + index * 4, false);
    }
    for (let index = 16; index < 64; index += 1) {
      const earlier15 = words[index - 15] ?? 0;
      const earlier2 = words[index - 2] ?? 0;
      const s0 = rotateRight(earlier15, 7) ^ rotateRight(earlier15, 18) ^ (earlier15 >>> 3);
      const s1 = rotateRight(earlier2, 17) ^ rotateRight(earlier2, 19) ^ (earlier2 >>> 10);
      words[index] = ((words[index - 16] ?? 0) + s0 + (words[index - 7] ?? 0) + s1) >>> 0;
    }
    let a = hash[0] ?? 0;
    let b = hash[1] ?? 0;
    let c = hash[2] ?? 0;
    let d = hash[3] ?? 0;
    let e = hash[4] ?? 0;
    let f = hash[5] ?? 0;
    let g = hash[6] ?? 0;
    let h = hash[7] ?? 0;
    for (let index = 0; index < 64; index += 1) {
      const sigma1 = rotateRight(e, 6) ^ rotateRight(e, 11) ^ rotateRight(e, 25);
      const choice = (e & f) ^ (~e & g);
      const first = (h + sigma1 + choice + (SHA256_CONSTANTS[index] ?? 0) + (words[index] ?? 0)) >>> 0;
      const sigma0 = rotateRight(a, 2) ^ rotateRight(a, 13) ^ rotateRight(a, 22);
      const majority = (a & b) ^ (a & c) ^ (b & c);
      const second = (sigma0 + majority) >>> 0;
      h = g;
      g = f;
      f = e;
      e = (d + first) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (first + second) >>> 0;
    }
    hash[0] = ((hash[0] ?? 0) + a) >>> 0;
    hash[1] = ((hash[1] ?? 0) + b) >>> 0;
    hash[2] = ((hash[2] ?? 0) + c) >>> 0;
    hash[3] = ((hash[3] ?? 0) + d) >>> 0;
    hash[4] = ((hash[4] ?? 0) + e) >>> 0;
    hash[5] = ((hash[5] ?? 0) + f) >>> 0;
    hash[6] = ((hash[6] ?? 0) + g) >>> 0;
    hash[7] = ((hash[7] ?? 0) + h) >>> 0;
  }
  return Array.from(hash, (word) => word.toString(16).padStart(8, "0")).join("");
}

async function requestJson<T>(fetcher: FetchLike, path: string, init?: RequestInit): Promise<T> {
  const response = await fetcher(path, init);
  if (!response.ok) {
    throw await responseError(response);
  }
  return (await response.json()) as T;
}

async function responseError(response: Response): Promise<FileApiError> {
  try {
    const body = (await response.json()) as { error?: string; expected_offset?: number | null };
    return new FileApiError(
      body.error ?? `${response.status} ${response.statusText}`,
      response.status,
      typeof body.expected_offset === "number" ? body.expected_offset : null,
    );
  } catch {
    return new FileApiError(`${response.status} ${response.statusText}`, response.status);
  }
}

function rotateRight(value: number, shift: number): number {
  return (value >>> shift) | (value << (32 - shift));
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    throw abortError();
  }
}

function abortError(): DOMException {
  return new DOMException("Transfer canceled", "AbortError");
}

async function abortableDelay(milliseconds: number, signal?: AbortSignal): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const onAbort = () => {
      window.clearTimeout(timer);
      reject(abortError());
    };
    const timer = window.setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, milliseconds);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

const SHA256_CONSTANTS = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
