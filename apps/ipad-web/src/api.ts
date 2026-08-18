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
  capture_frames: number;
  encoded_frames: number;
  encoded_keyframes: number;
  dropped_frames: number;
  video_transport_drops: number;
  video_startup_delta_frames: number;
  video_startup_wait_ms: number;
  encoded_bytes: number;
  input_batches: number;
  input_samples: number;
  injected_samples: number;
  input_errors: number;
  client_clock_offset_ms: number;
  input_arrival_ms: number;
  average_input_arrival_ms: number;
  max_input_arrival_ms: number;
  input_inject_ms: number;
  average_input_inject_ms: number;
  max_input_inject_ms: number;
  batch_sequence_gaps: number;
  sample_sequence_gaps: number;
  out_of_order_batches: number;
  out_of_order_samples: number;
  lifecycle_errors: number;
  active_pointers: number;
  last_batch_sequence: number;
  last_sample_sequence: number;
  pressure: number;
  pressure_min: number;
  pressure_max: number;
  tilt_x: number;
  tilt_y: number;
  tilt_x_min: number;
  tilt_x_max: number;
  tilt_y_min: number;
  tilt_y_max: number;
  rtt_ms: number;
  encode_ms: number;
  preprocess_ms: number;
  average_preprocess_ms: number;
  max_preprocess_ms: number;
  average_encode_ms: number;
  max_encode_ms: number;
  source_width: number;
  source_height: number;
  output_width: number;
  output_height: number;
}

export interface HostDiagnosticSummary {
  sample_count: number;
  retained_seconds: number;
  discarded_samples: number;
}

export async function getStatus(): Promise<HostStatus> {
  return requestJson<HostStatus>("/api/status");
}

export async function getMetrics(): Promise<HostMetrics> {
  return requestJson<HostMetrics>("/api/metrics");
}

export async function getDiagnosticSummary(): Promise<HostDiagnosticSummary> {
  return requestJson<HostDiagnosticSummary>("/api/diagnostics");
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
