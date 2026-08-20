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
  mouse_enabled: boolean;
  keyboard_enabled: boolean;
  gestures_default: boolean;
  file_transfer_enabled: boolean;
}

export interface PairResult {
  access_token: string;
  session_id: string;
  method: "pin" | "qr";
}

export interface RemoteInputSettings {
  touch_enabled: boolean;
  gestures_enabled: boolean;
}

export interface InputControl {
  revision: number;
  settings: RemoteInputSettings;
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
  video_recovery_requests: number;
  encoded_bytes: number;
  input_batches: number;
  input_samples: number;
  injected_samples: number;
  input_errors: number;
  mouse_samples: number;
  wheel_events: number;
  keyboard_events: number;
  text_events: number;
  text_bytes: number;
  command_events: number;
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
  preprocess_p50_ms: number;
  preprocess_p95_ms: number;
  preprocess_p99_ms: number;
  recent_preprocess_mean_ms: number;
  average_encode_ms: number;
  max_encode_ms: number;
  encode_p50_ms: number;
  encode_p95_ms: number;
  encode_p99_ms: number;
  recent_encode_mean_ms: number;
  process_cpu_percent: number;
  working_set_mib: number;
  peak_working_set_mib: number;
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

export type VideoCodec = "h264" | "hevc" | "av1";
export type EncoderMode = "auto" | "h264-hardware" | "hevc-hardware" | "av1-hardware" | "h264-software";

export interface BrowserCodecCapability {
  reported: boolean;
  included_in_sdp: boolean;
  negotiated: boolean;
  first_keyframe_received: boolean;
  presented: boolean;
  mime_types: string[];
  failure_reason: string | null;
}

export interface BrowserVideoCapabilities {
  user_agent: string;
  set_codec_preferences: boolean;
  h264: BrowserCodecCapability;
  hevc: BrowserCodecCapability;
  av1: BrowserCodecCapability;
}

export interface CodecBitrates {
  h264_mbps: number;
  hevc_mbps: number | null;
  av1_mbps: number | null;
}

export interface VideoPreset {
  max_width: number;
  max_fps: number;
  bitrates: CodecBitrates;
}

export interface VideoConfig {
  profile: "fast" | "balanced" | "sharp";
  encoder: EncoderMode;
  cursor: boolean;
  presets: { fast: VideoPreset; balanced: VideoPreset; sharp: VideoPreset };
}

export interface EncoderCapability {
  id: string;
  codec: VideoCodec;
  backend: "media-foundation-hardware" | "open-h264-software";
  hardware: boolean;
  encoder_name: string;
  adapter_name: string | null;
  adapter_luid: string | null;
  vendor: string | null;
  driver_version: string | null;
  input_formats: string[];
  profiles: string[];
  low_latency: boolean | null;
  rate_control: string[];
  maximum_tested_width: number | null;
  maximum_tested_height: number | null;
  maximum_tested_fps: number | null;
  state: "detected" | "initializeable" | "functional" | "benchmark-tested" | "unavailable" | "failed";
  failure_reason: string | null;
}

export interface VideoControl {
  settings: { revision: number; settings: VideoConfig };
  host_capabilities: EncoderCapability[];
  browser_capabilities: BrowserVideoCapabilities;
  compatibility: Array<{
    mode: EncoderMode;
    codec: VideoCodec;
    host_detected: boolean;
    host_functional: boolean;
    browser_reported: boolean;
    negotiated: boolean;
    presentation_verified: boolean;
    availability: "available" | "provisional" | "experimental" | "unavailable";
    reason: string;
  }>;
  runtime: {
    requested_mode: EncoderMode;
    active_mode: EncoderMode;
    codec: VideoCodec;
    backend: "media-foundation-hardware" | "open-h264-software";
    encoder_name: string;
    hardware: boolean;
    pipeline_memory_mode: "gpu-zero-copy" | "gpu-assisted" | "cpu-copy" | "cpu-preprocessing";
    output_width: number;
    output_height: number;
    target_fps: number;
    target_bitrate_bps: number;
    restart_count: number;
    switching: boolean;
    auto_selection_reason: string;
    last_error: string | null;
  };
  learned_results: AutoBenchmarkObservation[];
}

export interface BenchmarkMetrics {
  requested_fps: number;
  encoded_fps: number;
  presented_fps: number | null;
  encode_mean_ms: number;
  encode_p95_ms: number;
  preprocess_mean_ms: number;
  preprocess_p95_ms: number;
  actual_mbps: number;
  cpu_percent: number | null;
  working_set_mib: number | null;
  drop_percent: number;
  freeze_count: number | null;
  pipeline_p95_ms: number | null;
  quality_score: number | null;
}

export interface AutoBenchmarkObservation {
  schema_version: number;
  nfidb_version: string;
  receiver_runtime: string;
  encoder_id: string;
  mode: EncoderMode;
  profile: VideoConfig["profile"];
  max_width: number;
  requested_fps: number;
  end_to_end_verified: boolean;
  recorded_unix_ms: number;
  metrics: BenchmarkMetrics;
  score: {
    mode: EncoderMode;
    passed_gates: boolean;
    score: number | null;
    components: Record<string, number>;
    reasons: string[];
  };
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

export async function getInputControl(): Promise<InputControl> {
  return requestJson<InputControl>("/api/input");
}

export async function setInputSettings(
  baseRevision: number,
  settings: RemoteInputSettings,
): Promise<InputControl> {
  return requestJson<InputControl>("/api/input", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ base_revision: baseRevision, settings }),
  });
}

export async function getVideoControl(): Promise<VideoControl> {
  return requestJson<VideoControl>("/api/video");
}

export async function sendBrowserVideoCapabilities(capabilities: BrowserVideoCapabilities): Promise<VideoControl> {
  return requestJson<VideoControl>("/api/video/capabilities", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(capabilities),
  });
}

export async function setVideoSettings(baseRevision: number, settings: VideoConfig): Promise<VideoControl> {
  return requestJson<VideoControl>("/api/video", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ base_revision: baseRevision, settings }),
  });
}

export async function reportVideoPresented(
  codec: VideoCodec,
  firstKeyframeReceived: boolean,
  presented: boolean,
  failureReason: string | null = null,
): Promise<VideoControl> {
  return requestJson<VideoControl>("/api/video/presented", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      codec,
      first_keyframe_received: firstKeyframeReceived,
      presented,
      failure_reason: failureReason,
    }),
  });
}

export async function recordAutoBenchmark(observation: AutoBenchmarkObservation): Promise<VideoControl> {
  return requestJson<VideoControl>("/api/video/benchmark-result", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(observation),
  });
}

export async function clearAutoBenchmarks(): Promise<VideoControl> {
  return requestJson<VideoControl>("/api/video/benchmark-results", { method: "DELETE" });
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
