use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::pipeline::EncodedFrame;

/// Abstraction over different transport mechanisms (WebTransport, WebRTC).
///
/// Video delivery and input reception are handled differently per transport:
/// - WebTransport: video via unidirectional streams, input via bidirectional streams
/// - WebRTC: video via media tracks (GStreamer webrtcbin), input via data channels
#[async_trait]
pub trait TransportSession: Send + Sync {
    /// Send an encoded video frame to the client.
    ///
    /// For WebRTC with media tracks, this is a no-op since GStreamer pushes
    /// frames directly through the pipeline to webrtcbin.
    async fn send_video_frame(&self, frame: &EncodedFrame) -> Result<()>;

    /// Receive input data from the client. Returns `None` when the session is closed.
    async fn recv_input(&self) -> Result<Option<Vec<u8>>>;
}

/// WebTransport implementation of TransportSession.
///
/// Uses unidirectional streams for video and bidirectional streams for input.
pub struct WebTransportSession {
    connection: Arc<wtransport::Connection>,
    input_rx: tokio::sync::Mutex<Option<InputReceiver>>,
}

/// Receives input data from WebTransport bidirectional streams.
struct InputReceiver {
    connection: Arc<wtransport::Connection>,
    current_stream: Option<wtransport::RecvStream>,
    buf: Vec<u8>,
}

impl WebTransportSession {
    pub fn new(connection: wtransport::Connection) -> Self {
        let connection = Arc::new(connection);
        Self {
            input_rx: tokio::sync::Mutex::new(Some(InputReceiver {
                connection: connection.clone(),
                current_stream: None,
                buf: vec![0u8; 4096],
            })),
            connection,
        }
    }

    /// Spawn a video sender task that forwards frames from a broadcast channel.
    pub fn spawn_video_sender(
        self: &Arc<Self>,
        mut rx: broadcast::Receiver<EncodedFrame>,
    ) {
        let session = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(frame) => {
                        if let Err(e) = session.send_video_frame(&frame).await {
                            tracing::warn!("Failed to send video frame: {}", e);
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Video receiver lagged by {} frames, skipping", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Pipeline closed, stopping video sender");
                        break;
                    }
                }
            }
        });
    }
}

#[async_trait]
impl TransportSession for WebTransportSession {
    async fn send_video_frame(&self, frame: &EncodedFrame) -> Result<()> {
        let mut stream = self.connection.open_uni().await?.await?;

        let flags: u8 = if frame.is_keyframe { 0x01 } else { 0x00 };
        let mut header = [0u8; 13];
        header[0] = flags;
        header[1..9].copy_from_slice(&frame.pts.to_be_bytes());
        header[9..13].copy_from_slice(&(frame.data.len() as u32).to_be_bytes());

        stream.write_all(&header).await?;
        stream.write_all(&frame.data).await?;
        stream.finish().await?;

        Ok(())
    }

    async fn recv_input(&self) -> Result<Option<Vec<u8>>> {
        let mut guard = self.input_rx.lock().await;
        let receiver = match guard.as_mut() {
            Some(r) => r,
            None => return Ok(None),
        };

        loop {
            // Try reading from current stream first
            if let Some(ref mut stream) = receiver.current_stream {
                match stream.read(&mut receiver.buf).await {
                    Ok(Some(n)) if n > 0 => {
                        return Ok(Some(receiver.buf[..n].to_vec()));
                    }
                    Ok(_) => {
                        // Stream ended, accept next one
                        receiver.current_stream = None;
                    }
                    Err(e) => {
                        tracing::warn!("Input stream error: {}", e);
                        receiver.current_stream = None;
                    }
                }
            }

            // Accept a new bidirectional stream
            match receiver.connection.accept_bi().await {
                Ok((send, recv)) => {
                    // We don't send data back on this stream (except to close it later)
                    // Store the send half so it stays open
                    let _send = send;
                    receiver.current_stream = Some(recv);
                }
                Err(_) => {
                    // Connection closed
                    *guard = None;
                    return Ok(None);
                }
            }
        }
    }
}
