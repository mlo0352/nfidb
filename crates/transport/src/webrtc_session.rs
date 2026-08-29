use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bytes::Bytes;
use nfidb_core::{
    EncodedVideoFrame, EncoderBackend, InputSink, KeyframeRequest, Metrics, VideoCodec, VideoRuntimeStatus,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_HEVC, MediaEngine};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp::packet::Packet;
use webrtc::rtp::packetizer::{Packetizer, new_packetizer};
use webrtc::rtp::sequence::new_random_sequencer;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

use crate::process_input_packet;

#[derive(Debug, Deserialize)]
pub struct WebRtcOffer {
    pub token: String,
    pub sdp: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct WebRtcAnswer {
    pub sdp: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Default)]
pub struct ActivePeer(Mutex<Option<Arc<RTCPeerConnection>>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerVideoState {
    Waiting,
    Connected,
    Terminal,
}

#[derive(Debug)]
struct StartupKeyframeGate {
    awaiting_keyframe: bool,
}

impl StartupKeyframeGate {
    const fn new() -> Self {
        Self {
            awaiting_keyframe: true,
        }
    }

    fn admit(&mut self, keyframe: bool) -> bool {
        if self.awaiting_keyframe {
            if !keyframe {
                return false;
            }
            self.awaiting_keyframe = false;
        }
        true
    }

    fn require_keyframe(&mut self) {
        self.awaiting_keyframe = true;
    }
}

#[derive(Debug, Default)]
struct CaptureTimeline {
    origin: Option<Instant>,
    current_ticks: u64,
}

impl CaptureTimeline {
    fn advance_before(&mut self, captured_at: Instant, clock_rate: u32) -> u64 {
        let Some(origin) = self.origin else {
            self.origin = Some(captured_at);
            return 0;
        };
        let elapsed = captured_at.checked_duration_since(origin).unwrap_or_default();
        let absolute_ticks = duration_to_ticks(elapsed, clock_rate);
        let advance = absolute_ticks.saturating_sub(self.current_ticks).max(1);
        self.current_ticks = self.current_ticks.saturating_add(advance);
        advance
    }
}

#[derive(Debug)]
struct TimedRtpPacketizer {
    packetizer: Box<dyn Packetizer + Send + Sync>,
    timeline: CaptureTimeline,
    clock_rate: u32,
}

impl TimedRtpPacketizer {
    const OUTBOUND_MTU: usize = 1200;

    fn new(capability: &RTCRtpCodecCapability) -> Result<Self> {
        let packetizer = new_packetizer(
            Self::OUTBOUND_MTU,
            0,
            0,
            capability.payloader_for_codec()?,
            Box::new(new_random_sequencer()),
            capability.clock_rate,
        );
        Ok(Self {
            packetizer: Box::new(packetizer),
            timeline: CaptureTimeline::default(),
            clock_rate: capability.clock_rate,
        })
    }

    fn packetize(&mut self, data: Bytes, captured_at: Instant) -> std::result::Result<Vec<Packet>, webrtc::rtp::Error> {
        let mut advance = self.timeline.advance_before(captured_at, self.clock_rate);
        while advance != 0 {
            let chunk = advance.min(u64::from(u32::MAX)) as u32;
            self.packetizer.skip_samples(chunk);
            advance -= u64::from(chunk);
        }
        // The next frame advances the clock before it is packetized. Keeping
        // the packetizer's post-frame increment at zero makes an idle gap land
        // on the resumed frame instead of one frame late.
        self.packetizer.packetize(&data, 0)
    }
}

fn duration_to_ticks(duration: Duration, clock_rate: u32) -> u64 {
    duration
        .as_secs()
        .saturating_mul(u64::from(clock_rate))
        .saturating_add(u64::from(duration.subsec_nanos()).saturating_mul(u64::from(clock_rate)) / 1_000_000_000)
}

impl ActivePeer {
    pub async fn replace(&self, peer: Arc<RTCPeerConnection>) {
        let previous = self.0.lock().replace(peer);
        if let Some(previous) = previous {
            let _ = previous.close().await;
        }
    }

    pub async fn close(&self) {
        let previous = self.0.lock().take();
        if let Some(previous) = previous {
            let _ = previous.close().await;
        }
    }
}

pub async fn accept_offer(
    offer: WebRtcOffer,
    input: Arc<dyn InputSink>,
    metrics: Arc<Metrics>,
    mut video_rx: broadcast::Receiver<EncodedVideoFrame>,
    keyframe_request: KeyframeRequest,
    video: VideoRuntimeStatus,
    active: &ActivePeer,
) -> Result<WebRtcAnswer> {
    if offer.kind != "offer" {
        anyhow::bail!("expected SDP offer, received {}", offer.kind);
    }
    let codec = video.codec;
    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media)?;
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();
    let peer = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: Vec::<RTCIceServer>::new(),
            ..Default::default()
        })
        .await?,
    );

    let video_capability = codec_capability(codec, video.backend);
    let mut video_packetizer = TimedRtpPacketizer::new(&video_capability)?;
    let video_track = Arc::new(TrackLocalStaticRTP::new(
        video_capability,
        "screen".to_owned(),
        "nfidb".to_owned(),
    ));
    let rtp_sender = peer
        .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
        .await?;
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; 1500];
        while rtp_sender.read(&mut buffer).await.is_ok() {}
    });

    let channel_input = Arc::clone(&input);
    let channel_metrics = Arc::clone(&metrics);
    let channel_keyframe_request = keyframe_request.clone();
    peer.on_data_channel(Box::new(move |channel: Arc<RTCDataChannel>| {
        let input = Arc::clone(&channel_input);
        let metrics = Arc::clone(&channel_metrics);
        let keyframe_request = channel_keyframe_request.clone();
        Box::pin(async move {
            if channel.label() != "input" && channel.label() != "control" {
                return;
            }
            let message_input = Arc::clone(&input);
            let message_metrics = Arc::clone(&metrics);
            let message_keyframe_request = keyframe_request.clone();
            channel.on_message(Box::new(move |message: DataChannelMessage| {
                let input = Arc::clone(&message_input);
                let metrics = Arc::clone(&message_metrics);
                let keyframe_request = message_keyframe_request.clone();
                Box::pin(async move {
                    if message.is_string {
                        if message.data.as_ref() == b"request-keyframe" {
                            metrics.video_recovery_requested();
                            keyframe_request.request();
                            tracing::info!("browser requested a video recovery keyframe over DataChannel");
                        }
                        return;
                    }
                    process_input_packet(input.as_ref(), metrics.as_ref(), &message.data, "datachannel");
                })
            }));
            let close_input = Arc::clone(&input);
            let close_metrics = Arc::clone(&metrics);
            channel.on_close(Box::new(move || {
                let input = Arc::clone(&close_input);
                let metrics = Arc::clone(&close_metrics);
                Box::pin(async move {
                    let _ = input.reset_all();
                    metrics.reset_input_continuity();
                })
            }));
        })
    }));

    let (video_state_tx, mut video_state_rx) = watch::channel(PeerVideoState::Waiting);
    let state_metrics = Arc::clone(&metrics);
    let state_input = Arc::clone(&input);
    let connected_keyframe_request = keyframe_request.clone();
    peer.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
        let metrics = Arc::clone(&state_metrics);
        let input = Arc::clone(&state_input);
        let video_state_tx = video_state_tx.clone();
        let keyframe_request = connected_keyframe_request.clone();
        Box::pin(async move {
            let connected = state == RTCPeerConnectionState::Connected;
            metrics.set_connected(connected);
            let terminal = matches!(
                state,
                RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
            );
            if connected {
                keyframe_request.request();
                let _ = video_state_tx.send(PeerVideoState::Connected);
            } else if terminal {
                let _ = video_state_tx.send(PeerVideoState::Terminal);
                let _ = input.reset_all();
                metrics.reset_input_continuity();
            }
            tracing::info!(?state, "WebRTC peer state changed");
        })
    }));

    let remote = RTCSessionDescription::offer(offer.sdp).context("invalid SDP offer")?;
    peer.set_remote_description(remote).await?;
    let answer = peer.create_answer(None).await?;
    let mut gathering = peer.gathering_complete_promise().await;
    peer.set_local_description(answer).await?;
    let _ = gathering.recv().await;
    let local = peer
        .local_description()
        .await
        .context("WebRTC local description unavailable")?;

    let video_metrics = Arc::clone(&metrics);
    tokio::spawn(async move {
        while *video_state_rx.borrow() == PeerVideoState::Waiting {
            if video_state_rx.changed().await.is_err() {
                return;
            }
        }
        if *video_state_rx.borrow() != PeerVideoState::Connected {
            return;
        }

        // Drop frames accumulated while SDP/ICE completed. Request a fresh IDR
        // only after the peer is connected, then make that IDR the first sample
        // ever handed to this receiver.
        video_rx = video_rx.resubscribe();
        keyframe_request.request();
        let startup_started = Instant::now();
        let mut startup_gate = StartupKeyframeGate::new();
        let mut video_started = false;
        'video: loop {
            match video_rx.recv().await {
                Ok(frame) => {
                    if frame.codec != codec {
                        video_metrics.video_transport_dropped(1);
                        continue;
                    }
                    if !startup_gate.admit(frame.keyframe) {
                        if video_started {
                            video_metrics.video_transport_dropped(1);
                        } else {
                            video_metrics.video_startup_delta_frame_skipped();
                        }
                        continue;
                    }
                    if !video_started {
                        video_metrics.video_started(startup_started.elapsed());
                        video_started = true;
                    }
                    let packets =
                        match video_packetizer.packetize(Bytes::copy_from_slice(&frame.data), frame.captured_at) {
                            Ok(packets) => packets,
                            Err(error) => {
                                tracing::warn!(%error, "video RTP packetization failed");
                                break;
                            }
                        };
                    for packet in packets {
                        if let Err(error) = video_track.write_rtp_with_extensions(&packet, &[]).await {
                            tracing::debug!(%error, "video track closed");
                            break 'video;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    video_metrics.video_transport_dropped(skipped);
                    startup_gate.require_keyframe();
                    keyframe_request.request();
                    tracing::debug!(
                        skipped,
                        "dropped stale video frames before WebRTC; requesting a recovery keyframe"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    active.replace(Arc::clone(&peer)).await;
    Ok(WebRtcAnswer {
        sdp: local.sdp,
        kind: "answer".to_owned(),
    })
}

fn codec_capability(codec: VideoCodec, backend: EncoderBackend) -> RTCRtpCodecCapability {
    match codec {
        VideoCodec::H264 => RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            // Apple VideoToolbox's Baseline profile emits an SPS beginning
            // 4200, while the existing Windows encoders use Constrained
            // Baseline. Profile constraints are symmetric in RFC 6184, even
            // when level asymmetry is permitted, and Safari enforces them.
            sdp_fmtp_line: match backend {
                EncoderBackend::VideoToolboxHardware => {
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                }
                EncoderBackend::MediaFoundationHardware | EncoderBackend::OpenH264Software => {
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                }
            }
            .to_owned(),
            ..Default::default()
        },
        VideoCodec::Hevc => RTCRtpCodecCapability {
            mime_type: MIME_TYPE_HEVC.to_owned(),
            clock_rate: 90_000,
            ..Default::default()
        },
        VideoCodec::Av1 => RTCRtpCodecCapability {
            mime_type: MIME_TYPE_AV1.to_owned(),
            clock_rate: 90_000,
            sdp_fmtp_line: "profile-id=0".to_owned(),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{CaptureTimeline, StartupKeyframeGate, TimedRtpPacketizer, codec_capability};
    use bytes::Bytes;
    use nfidb_core::{EncoderBackend, VideoCodec};
    use webrtc::api::media_engine::{MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_HEVC};

    #[test]
    fn startup_gate_rejects_delta_frames_until_first_keyframe() {
        let mut gate = StartupKeyframeGate::new();
        assert!(!gate.admit(false));
        assert!(!gate.admit(false));
        assert!(gate.admit(true));
        assert!(gate.admit(false));
        gate.require_keyframe();
        assert!(!gate.admit(false));
        assert!(gate.admit(true));
    }

    #[test]
    fn capture_timeline_does_not_accumulate_fractional_frame_drift() {
        let start = Instant::now();
        let mut timeline = CaptureTimeline::default();
        assert_eq!(timeline.advance_before(start, 90_000), 0);
        let mut advanced = 0;
        for frame in 1..=60_u32 {
            advanced += timeline.advance_before(start + Duration::from_nanos(u64::from(frame) * 16_666_667), 90_000);
        }
        assert_eq!(advanced, 90_000);
    }

    #[test]
    fn resumed_frame_carries_the_idle_gap_in_its_rtp_timestamp() {
        let capability = codec_capability(VideoCodec::H264, EncoderBackend::VideoToolboxHardware);
        let mut packetizer = TimedRtpPacketizer::new(&capability).unwrap();
        let frame = Bytes::from_static(&[0, 0, 0, 1, 0x65, 0x88, 0x84]);
        let start = Instant::now();

        let first = packetizer.packetize(frame.clone(), start).unwrap();
        let second = packetizer
            .packetize(frame.clone(), start + Duration::from_nanos(16_666_667))
            .unwrap();
        let resumed = packetizer.packetize(frame, start + Duration::from_secs(3)).unwrap();

        assert!(!first.is_empty());
        assert!(!second.is_empty());
        assert!(!resumed.is_empty());
        let initial_timestamp = first[0].header.timestamp;
        assert_eq!(second[0].header.timestamp.wrapping_sub(initial_timestamp), 1_500);
        assert_eq!(resumed[0].header.timestamp.wrapping_sub(initial_timestamp), 270_000);
    }

    #[test]
    fn every_encoder_codec_has_matching_rtp_identity() {
        assert_eq!(
            codec_capability(VideoCodec::H264, EncoderBackend::MediaFoundationHardware).mime_type,
            MIME_TYPE_H264
        );
        assert_eq!(
            codec_capability(VideoCodec::Hevc, EncoderBackend::VideoToolboxHardware).mime_type,
            MIME_TYPE_HEVC
        );
        assert_eq!(
            codec_capability(VideoCodec::Av1, EncoderBackend::MediaFoundationHardware).mime_type,
            MIME_TYPE_AV1
        );
    }

    #[test]
    fn h264_signaling_matches_the_encoder_profile_constraints() {
        assert!(
            codec_capability(VideoCodec::H264, EncoderBackend::VideoToolboxHardware)
                .sdp_fmtp_line
                .contains("profile-level-id=42001f")
        );
        assert!(
            codec_capability(VideoCodec::H264, EncoderBackend::MediaFoundationHardware)
                .sdp_fmtp_line
                .contains("profile-level-id=42e01f")
        );
    }
}
