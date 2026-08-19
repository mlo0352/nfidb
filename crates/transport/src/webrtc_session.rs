use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use bytes::Bytes;
use nfidb_core::{EncodedVideoFrame, InputSink, KeyframeRequest, Metrics};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_H264, MediaEngine};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

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
    active: &ActivePeer,
) -> Result<WebRtcAnswer> {
    if offer.kind != "offer" {
        anyhow::bail!("expected SDP offer, received {}", offer.kind);
    }
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

    let video_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_owned(),
            ..Default::default()
        },
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
            channel.on_close(Box::new(move || {
                let input = Arc::clone(&close_input);
                Box::pin(async move {
                    let _ = input.reset_all();
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
        loop {
            match video_rx.recv().await {
                Ok(frame) => {
                    let first_decodable_frame = startup_gate.awaiting_keyframe;
                    if !startup_gate.admit(frame.keyframe) {
                        video_metrics.video_startup_delta_frame_skipped();
                        continue;
                    }
                    if first_decodable_frame {
                        video_metrics.video_started(startup_started.elapsed());
                    }
                    let sample = Sample {
                        data: Bytes::copy_from_slice(&frame.data),
                        duration: frame.duration,
                        ..Default::default()
                    };
                    if let Err(error) = video_track.write_sample(&sample).await {
                        tracing::debug!(%error, "video track closed");
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    video_metrics.video_transport_dropped(skipped);
                    tracing::debug!(skipped, "dropped stale video frames before WebRTC");
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

#[cfg(test)]
mod tests {
    use super::StartupKeyframeGate;

    #[test]
    fn startup_gate_rejects_delta_frames_until_first_keyframe() {
        let mut gate = StartupKeyframeGate::new();
        assert!(!gate.admit(false));
        assert!(!gate.admit(false));
        assert!(gate.admit(true));
        assert!(gate.admit(false));
    }
}
