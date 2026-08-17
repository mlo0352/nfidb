use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use nfidb_core::{EncodedVideoFrame, InputSink, Metrics};
use nfidb_protocol::PointerBatch;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
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
    peer.on_data_channel(Box::new(move |channel: Arc<RTCDataChannel>| {
        let input = Arc::clone(&channel_input);
        let metrics = Arc::clone(&channel_metrics);
        Box::pin(async move {
            if channel.label() != "input" && channel.label() != "control" {
                return;
            }
            let message_input = Arc::clone(&input);
            let message_metrics = Arc::clone(&metrics);
            channel.on_message(Box::new(move |message: DataChannelMessage| {
                let input = Arc::clone(&message_input);
                let metrics = Arc::clone(&message_metrics);
                Box::pin(async move {
                    if message.is_string {
                        return;
                    }
                    match PointerBatch::decode(&message.data) {
                        Ok(batch) => {
                            if let Some(last) = batch.samples.last() {
                                metrics.input(
                                    batch.samples.len(),
                                    batch.samples.len().saturating_sub(1),
                                    last.pressure,
                                    last.tilt_x_deg,
                                    last.tilt_y_deg,
                                );
                            }
                            if let Err(error) = input.inject_batch(&batch) {
                                tracing::warn!(%error, "DataChannel pointer injection failed");
                            }
                        }
                        Err(error) => tracing::warn!(%error, "invalid DataChannel pointer packet"),
                    }
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

    let state_metrics = Arc::clone(&metrics);
    let state_input = Arc::clone(&input);
    peer.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
        let metrics = Arc::clone(&state_metrics);
        let input = Arc::clone(&state_input);
        Box::pin(async move {
            let connected = state == RTCPeerConnectionState::Connected;
            metrics.set_connected(connected);
            if matches!(
                state,
                RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
            ) {
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

    tokio::spawn(async move {
        while let Ok(frame) = video_rx.recv().await {
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
    });

    active.replace(Arc::clone(&peer)).await;
    Ok(WebRtcAnswer {
        sdp: local.sdp,
        kind: "answer".to_owned(),
    })
}
