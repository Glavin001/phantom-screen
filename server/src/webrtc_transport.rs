use anyhow::Result;
use async_trait::async_trait;
use gstreamer::glib;
use gstreamer::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::pipeline::EncodedFrame;
use crate::signaling::SignalingState;
use crate::transport::TransportSession;

/// WebRTC implementation of TransportSession.
///
/// Video is handled by GStreamer's webrtcbin element (media tracks), so
/// `send_video_frame` is a no-op. Input arrives via a WebRTC data channel
/// using the same binary protocol as WebTransport.
pub struct WebRtcSession {
    _signaling: Arc<SignalingState>,
    input_rx: tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
}

impl WebRtcSession {
    /// Create a new WebRTC session by wiring up the webrtcbin data channel.
    ///
    /// The `signaling_state` must already have its webrtcbin element configured
    /// in the pipeline.
    pub fn new(signaling_state: Arc<SignalingState>) -> Arc<Self> {
        let (input_tx, input_rx) = mpsc::channel(256);

        // Listen for incoming data channels on the webrtcbin
        let input_tx_clone = input_tx.clone();
        signaling_state
            .webrtcbin
            .connect("on-data-channel", false, move |args| {
                let channel = args[1]
                    .get::<glib::Object>()
                    .expect("on-data-channel arg should be an Object");

                let label = channel
                    .property::<String>("label");
                tracing::info!("WebRTC data channel opened: {}", label);

                if label == "input" {
                    let tx = input_tx_clone.clone();
                    channel.connect("on-message-data", false, move |args| {
                        if let Ok(bytes) = args[1].get::<glib::Bytes>() {
                            let data: Vec<u8> = bytes.as_ref().to_vec();
                            if !data.is_empty() {
                                let _ = tx.try_send(data);
                            }
                        }
                        None
                    });
                }

                None
            });

        Arc::new(Self {
            _signaling: signaling_state,
            input_rx: tokio::sync::Mutex::new(input_rx),
        })
    }
}

#[async_trait]
impl TransportSession for WebRtcSession {
    async fn send_video_frame(&self, _frame: &EncodedFrame) -> Result<()> {
        // No-op: video is pushed through GStreamer's webrtcbin media track
        // directly via the pipeline (tee → h264parse → rtph264pay → webrtcbin).
        Ok(())
    }

    async fn recv_input(&self) -> Result<Option<Vec<u8>>> {
        let mut rx = self.input_rx.lock().await;
        Ok(rx.recv().await)
    }
}
