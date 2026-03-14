//! Per-window GStreamer pipeline manager for Coherence Mode.
//!
//! Each tracked window gets its own GStreamer capture+encode pipeline using
//! `ximagesrc xid=<window_id>` for efficient per-window capture.

use anyhow::{Context, Result};
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;

use crate::config::{Config, EncoderType, detect_encoder};
use crate::pipeline::{EncodedFrame, build_window_pipeline_string};

/// A single per-window GStreamer pipeline.
struct WindowPipeline {
    pipeline: gstreamer::Pipeline,
    tx: broadcast::Sender<EncodedFrame>,
    running: AtomicBool,
}

impl WindowPipeline {
    fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }

    fn pause(&self) {
        let _ = self.pipeline.set_state(gstreamer::State::Paused);
    }

    fn resume(&self) {
        let _ = self.pipeline.set_state(gstreamer::State::Playing);
    }
}

/// Manages per-window GStreamer pipelines.
pub struct WindowPipelineManager {
    display: String,
    fps: u32,
    bitrate: u32,
    keyframe_interval: u32,
    max_pipelines: u32,
    encoder_type: EncoderType,
    pipelines: HashMap<u32, WindowPipeline>,
}

impl WindowPipelineManager {
    pub fn new(config: &Config) -> Self {
        let encoder_type = detect_encoder();
        Self {
            display: config.display.clone(),
            fps: config.fps,
            bitrate: config.window_bitrate,
            keyframe_interval: config.keyframe_interval,
            max_pipelines: config.max_window_pipelines,
            encoder_type,
            pipelines: HashMap::new(),
        }
    }

    /// Start streaming a window. Returns a receiver for encoded frames.
    pub fn start_window(&mut self, window_id: u32) -> Result<broadcast::Receiver<EncodedFrame>> {
        // If already running, force a keyframe and return a new receiver
        if let Some(wp) = self.pipelines.get(&window_id) {
            force_keyframe(&wp.pipeline, window_id);
            return Ok(wp.tx.subscribe());
        }

        // Validate the window still exists before creating a GStreamer pipeline.
        // GStreamer's ximagesrc uses Xlib internally and will trigger X errors
        // (potentially fatal without our custom error handler) if the window
        // ID is stale (e.g., from before an Xvfb restart).
        if !validate_window_exists(&self.display, window_id) {
            anyhow::bail!(
                "Window {} (0x{:x}) does not exist on display {} — refusing to start pipeline",
                window_id,
                window_id,
                self.display
            );
        }

        // Check pipeline limit
        if self.pipelines.len() as u32 >= self.max_pipelines {
            anyhow::bail!(
                "Maximum per-window pipelines ({}) reached",
                self.max_pipelines
            );
        }

        let pipeline_str = build_window_pipeline_string(
            &self.display,
            window_id,
            self.fps,
            self.bitrate,
            self.keyframe_interval,
            self.encoder_type,
        );

        tracing::info!(
            "Starting per-window pipeline for window {}: {}",
            window_id,
            pipeline_str
        );

        let pipeline = gstreamer::parse::launch(&pipeline_str)
            .context("Failed to parse per-window pipeline")?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow::anyhow!("Pipeline is not a GstPipeline"))?;

        let appsink = pipeline
            .by_name("sink")
            .context("No element named 'sink'")?
            .downcast::<AppSink>()
            .map_err(|_| anyhow::anyhow!("Element 'sink' is not an AppSink"))?;

        let (tx, rx) = broadcast::channel::<EncodedFrame>(60);
        let tx_clone = tx.clone();
        let wid_for_cb = window_id;
        let frame_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let frame_counter_cb = frame_counter.clone();

        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    let sample = appsink
                        .pull_sample()
                        .map_err(|_| gstreamer::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;
                    let map = buffer
                        .map_readable()
                        .map_err(|_| gstreamer::FlowError::Error)?;

                    let pts = buffer.pts().map(|p| p.nseconds()).unwrap_or(0);
                    let is_keyframe = !buffer.flags().contains(gstreamer::BufferFlags::DELTA_UNIT);

                    let count = frame_counter_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if count < 3 {
                        tracing::info!(
                            "Window {} frame #{}: {} bytes, keyframe={}, pts={}",
                            wid_for_cb,
                            count,
                            map.len(),
                            is_keyframe,
                            pts
                        );
                    }

                    let frame = EncodedFrame {
                        data: map.to_vec(),
                        pts,
                        is_keyframe,
                    };

                    let _ = tx_clone.send(frame);
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("Failed to start per-window pipeline")?;

        let wp = WindowPipeline {
            pipeline,
            tx,
            running: AtomicBool::new(true),
        };

        self.pipelines.insert(window_id, wp);
        Ok(rx)
    }

    /// Stop streaming a window and tear down its pipeline.
    pub fn stop_window(&mut self, window_id: u32) {
        if let Some(wp) = self.pipelines.remove(&window_id) {
            wp.stop();
            tracing::info!("Stopped per-window pipeline for window {}", window_id);
        }
    }

    /// Pause a window's pipeline (e.g., when hidden).
    pub fn pause_window(&mut self, window_id: u32) {
        if let Some(wp) = self.pipelines.get(&window_id) {
            wp.pause();
            tracing::debug!("Paused pipeline for window {}", window_id);
        }
    }

    /// Resume a paused window's pipeline.
    pub fn resume_window(&mut self, window_id: u32) {
        if let Some(wp) = self.pipelines.get(&window_id) {
            wp.resume();
            tracing::debug!("Resumed pipeline for window {}", window_id);
        }
    }

    /// Get a new frame receiver for an already-running window pipeline.
    pub fn get_receiver(&self, window_id: u32) -> Option<broadcast::Receiver<EncodedFrame>> {
        self.pipelines.get(&window_id).map(|wp| wp.tx.subscribe())
    }

    /// Check if a window is currently being streamed.
    pub fn is_streaming(&self, window_id: u32) -> bool {
        self.pipelines.contains_key(&window_id)
    }

    /// Restart a window's pipeline (e.g., after resize).
    pub fn restart_window(&mut self, window_id: u32) -> Result<broadcast::Receiver<EncodedFrame>> {
        self.stop_window(window_id);
        self.start_window(window_id)
    }

    /// Stop all pipelines.
    pub fn stop_all(&mut self) {
        let ids: Vec<u32> = self.pipelines.keys().copied().collect();
        for id in ids {
            self.stop_window(id);
        }
    }

    /// Number of active pipelines.
    pub fn active_count(&self) -> usize {
        self.pipelines.len()
    }
}

/// Check if a window ID is valid on the given display using x11rb.
///
/// This uses the Rust x11rb library (not Xlib), which handles X errors
/// gracefully via Result types instead of calling exit().
fn validate_window_exists(x_display: &str, window_id: u32) -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;

    let conn = match x11rb::rust_connection::RustConnection::connect(Some(x_display)) {
        Ok((conn, _)) => conn,
        Err(e) => {
            tracing::warn!("Cannot connect to {} to validate window: {}", x_display, e);
            return false;
        }
    };

    match conn.get_window_attributes(window_id) {
        Ok(cookie) => match cookie.reply() {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("Window 0x{:x} does not exist on {}: {}", window_id, x_display, e);
                false
            }
        },
        Err(e) => {
            tracing::warn!("Failed to query window 0x{:x}: {}", window_id, e);
            false
        }
    }
}

/// Force the encoder to emit a keyframe by sending a custom upstream event.
fn force_keyframe(pipeline: &gstreamer::Pipeline, window_id: u32) {
    if let Some(encoder) = pipeline.by_name("encoder") {
        // Build a GstForceKeyUnit upstream event using the gstreamer-video crate
        let event = gstreamer_video::UpstreamForceKeyUnitEvent::builder()
            .all_headers(true)
            .build();
        if encoder.send_event(event) {
            tracing::info!("Forced keyframe for window {} pipeline", window_id);
        } else {
            tracing::warn!(
                "Failed to send force-keyframe event for window {}",
                window_id
            );
        }
    }
}

impl Drop for WindowPipelineManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_pipeline_string_contains_xid() {
        let s = build_window_pipeline_string(":99", 12345, 30, 2000, 60, EncoderType::X264);
        assert!(s.contains("xid=12345"));
        assert!(s.contains("display-name=:99"));
        assert!(s.contains("framerate=30/1"));
        assert!(s.contains("bitrate=2000"));
    }

    #[test]
    fn window_pipeline_string_nvenc() {
        let s = build_window_pipeline_string(":0", 999, 60, 3000, 30, EncoderType::Nvenc);
        assert!(s.contains("xid=999"));
        assert!(s.contains("nvh264enc"));
        assert!(s.contains("bitrate=3000"));
    }

    #[test]
    fn window_pipeline_string_vaapi() {
        let s = build_window_pipeline_string(":1", 42, 24, 1500, 48, EncoderType::Vaapi);
        assert!(s.contains("xid=42"));
        assert!(s.contains("vaapih264enc"));
        assert!(s.contains("bitrate=1500"));
    }
}
