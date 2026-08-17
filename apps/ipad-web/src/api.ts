export interface HostStatus {
  product: string;
  host_name: string;
  session_id: string;
  paired: boolean;
  expires_in_seconds: number;
  mode: string;
  protocol_version: number;
  webrtc: boolean;
  touch_default: boolean;
}

export interface PairResult {
  access_token: string;
  session_id: string;
  method: "pin" | "qr";
}

export interface HostMetrics {
  connected: boolean;
  capture_fps: number;
  encoded_fps: number;
  input_samples_per_sec: number;
  coalesced_samples_per_sec: number;
  dropped_frames: number;
  encoded_bytes: number;
  input_samples: number;
  pressure: number;
  tilt_x: number;
  tilt_y: number;
  rtt_ms: number;
  encode_ms: number;
  source_width: number;
  source_height: number;
}

export async function getStatus(): Promise<HostStatus> {
  return requestJson<HostStatus>("/api/status");
}

export async function pairWithPin(pin: string): Promise<PairResult> {
  return requestJson<PairResult>("/api/pair", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ pin: pin.replace(/\D/g, "") }),
  });
}

export async function pairWithQr(secret: string): Promise<PairResult> {
  return requestJson<PairResult>("/api/pair", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ qr_secret: secret }),
  });
}

export async function disconnect(token: string): Promise<void> {
  const response = await fetch("/api/disconnect", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (!response.ok && response.status !== 401) {
    throw new Error(await errorFrom(response));
  }
}

export async function sendOffer(token: string, description: RTCSessionDescriptionInit): Promise<RTCSessionDescriptionInit> {
  return requestJson<RTCSessionDescriptionInit>("/api/webrtc/offer", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token, type: description.type, sdp: description.sdp }),
  });
}

async function requestJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  if (!response.ok) {
    throw new Error(await errorFrom(response));
  }
  return (await response.json()) as T;
}

async function errorFrom(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: string };
    return body.error ?? `${response.status} ${response.statusText}`;
  } catch {
    return `${response.status} ${response.statusText}`;
  }
}
