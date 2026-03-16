//! Coherence Mode session orchestration.
//!
//! Ties together the window monitor, per-window pipeline manager, and
//! WebTransport session to deliver per-window video streams and window events.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::pipeline::{EncodedFrame, clamp_to_display, round_to_even, unpack_display_size};
use crate::window_monitor::{TrackedWindows, WindowEvent, WindowManager};
use crate::window_pipeline::WindowPipelineManager;

/// Shared state for coherence mode, shared across all sessions.
pub struct CoherenceState {
    pub window_events: broadcast::Sender<WindowEvent>,
    pub tracked_windows: TrackedWindows,
    pub pipeline_manager: Arc<Mutex<WindowPipelineManager>>,
    pub window_manager: Arc<WindowManager>,
    /// Set to false during resize (before Xvfb kill), set to true when window
    /// monitor reconnects and re-enables Composite (on Snapshot event).
    pub composite_ready: Arc<AtomicBool>,
    /// Current Xvfb display dimensions (packed as width << 16 | height).
    /// Used to clamp per-window resize requests so windows never exceed
    /// the virtual display bounds (which causes Composite BadMatch errors).
    pub display_size: Arc<AtomicU32>,
}

impl CoherenceState {
    /// Get the current window list as a snapshot event.
    pub fn current_snapshot(&self) -> WindowEvent {
        let windows = self
            .tracked_windows
            .lock()
            .map(|tracked| tracked.values().cloned().collect())
            .unwrap_or_default();
        WindowEvent::Snapshot(windows)
    }
}

/// Per-session coherence handler.
pub struct CoherenceSession {
    window_event_rx: broadcast::Receiver<WindowEvent>,
    pipeline_manager: Arc<Mutex<WindowPipelineManager>>,
    window_manager: Arc<WindowManager>,
    subscriptions: HashMap<u32, JoinHandle<()>>,
    display_size: Arc<AtomicU32>,
}

impl CoherenceSession {
    pub fn new(state: &CoherenceState) -> Self {
        Self {
            window_event_rx: state.window_events.subscribe(),
            pipeline_manager: state.pipeline_manager.clone(),
            window_manager: state.window_manager.clone(),
            subscriptions: HashMap::new(),
            display_size: state.display_size.clone(),
        }
    }

    /// Subscribe to a window's video stream.
    /// Returns a receiver for that window's encoded frames.
    pub async fn subscribe_window(
        &mut self,
        window_id: u32,
    ) -> Result<broadcast::Receiver<EncodedFrame>> {
        let mut mgr = self.pipeline_manager.lock().unwrap();
        let rx = mgr.start_window(window_id)?;
        info!("Session subscribed to window {}", window_id);
        Ok(rx)
    }

    /// Unsubscribe from a window's video stream.
    pub async fn unsubscribe_window(&mut self, window_id: u32) {
        if let Some(handle) = self.subscriptions.remove(&window_id) {
            info!("Window {} unsubscribe: ABORTING sender task", window_id);
            handle.abort();
        } else {
            warn!(
                "Window {} unsubscribe: NO sender task found in subscriptions!",
                window_id
            );
        }
        info!("Session unsubscribed from window {}", window_id);
    }

    /// Track a frame sender task for a window.
    pub fn track_sender(&mut self, window_id: u32, handle: JoinHandle<()>) {
        if let Some(old) = self.subscriptions.insert(window_id, handle) {
            info!(
                "Window {} track_sender: ABORTING old sender task",
                window_id
            );
            old.abort();
        } else {
            info!(
                "Window {} track_sender: no previous sender (first subscribe)",
                window_id
            );
        }
    }

    /// Resize a remote X11 window and restart its per-window pipeline
    /// so ximagesrc captures at the new dimensions.
    pub async fn resize_window(
        &self,
        window_id: u32,
        width: u16,
        height: u16,
    ) -> Result<Option<broadcast::Receiver<EncodedFrame>>> {
        // Round up to even dimensions — x264enc / I420 (4:2:0 chroma subsampling)
        // requires even width and height; odd values cause silent cap negotiation
        // failure resulting in 0 fps (black screen).
        let mut width = round_to_even(width);
        let mut height = round_to_even(height);

        // Clamp to the current display dimensions — a window exceeding the Xvfb
        // display bounds causes Composite BadMatch errors every frame, resulting
        // in black/corrupt captures.
        let packed = self.display_size.load(std::sync::atomic::Ordering::Acquire);
        let (display_w, display_h) = unpack_display_size(packed);
        let (clamped_w, clamped_h) = clamp_to_display(width, height, display_w, display_h);
        if clamped_w != width || clamped_h != height {
            info!(
                "Clamping window {} resize from {}x{} to {}x{} (display is {}x{})",
                window_id, width, height, clamped_w, clamped_h, display_w, display_h
            );
            width = clamped_w;
            height = clamped_h;
        }

        info!(
            "Resizing coherence window {} to {}x{}",
            window_id, width, height
        );

        self.window_manager.resize(window_id, width, height)?;

        // Restart the per-window pipeline if it's currently streaming,
        // since ximagesrc negotiates caps once at startup and won't
        // pick up the new window size automatically.
        let mut mgr = self.pipeline_manager.lock().unwrap();
        if mgr.is_streaming(window_id) {
            let rx = mgr.restart_window(window_id)?;
            info!(
                "Restarted per-window pipeline for window {} at {}x{}",
                window_id, width, height
            );
            Ok(Some(rx))
        } else {
            info!(
                "Window {} not currently streaming, skipping pipeline restart",
                window_id
            );
            Ok(None)
        }
    }

    /// Focus/raise a remote X11 window.
    pub fn focus_window(&self, window_id: u32) -> Result<()> {
        self.window_manager.raise(window_id)
    }

    /// Close a remote X11 window.
    pub fn close_window(&self, window_id: u32) -> Result<()> {
        self.window_manager.close(window_id)
    }

    /// Get the next window event from the monitor.
    pub async fn next_window_event(&mut self) -> Option<WindowEvent> {
        match self.window_event_rx.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("Window event receiver lagged by {} events", n);
                // Try again
                self.window_event_rx.recv().await.ok()
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }

    /// Clean up all subscriptions.
    pub fn cleanup(&mut self) {
        for (_, handle) in self.subscriptions.drain() {
            handle.abort();
        }
    }
}

impl Drop for CoherenceSession {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Serialize a WindowEvent for sending to the client over the coherence event stream.
///
/// Format: [0x40][subtype][...payload]
pub fn serialize_window_event(event: &WindowEvent) -> Vec<u8> {
    match event {
        WindowEvent::Snapshot(windows) => {
            let mut buf = vec![0x40, 0x01];
            buf.extend_from_slice(&(windows.len() as u16).to_be_bytes());
            for w in windows {
                buf.extend(w.serialize());
            }
            buf
        }
        WindowEvent::Added(info) => {
            let mut buf = vec![0x40, 0x02];
            buf.extend(info.serialize());
            buf
        }
        WindowEvent::Removed { window_id } => {
            let mut buf = vec![0x40, 0x03];
            buf.extend_from_slice(&window_id.to_be_bytes());
            buf
        }
        WindowEvent::Resized {
            window_id,
            width,
            height,
        } => {
            let mut buf = vec![0x40, 0x04];
            buf.extend_from_slice(&window_id.to_be_bytes());
            // x and y are not changed in resize events, use 0
            buf.extend_from_slice(&0i16.to_be_bytes());
            buf.extend_from_slice(&0i16.to_be_bytes());
            buf.extend_from_slice(&width.to_be_bytes());
            buf.extend_from_slice(&height.to_be_bytes());
            buf
        }
        WindowEvent::Moved { window_id, x, y } => {
            let mut buf = vec![0x40, 0x04];
            buf.extend_from_slice(&window_id.to_be_bytes());
            buf.extend_from_slice(&x.to_be_bytes());
            buf.extend_from_slice(&y.to_be_bytes());
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf
        }
        WindowEvent::TitleChanged { window_id, title } => {
            let title_bytes = title.as_bytes();
            let mut buf = vec![0x40, 0x05];
            buf.extend_from_slice(&window_id.to_be_bytes());
            buf.extend_from_slice(&(title_bytes.len() as u16).to_be_bytes());
            buf.extend_from_slice(title_bytes);
            buf
        }
        WindowEvent::VisibilityChanged { window_id, visible } => {
            let mut buf = vec![0x40, 0x06];
            buf.extend_from_slice(&window_id.to_be_bytes());
            buf.push(if *visible { 1 } else { 0 });
            buf
        }
    }
}

/// Send an encoded video frame tagged with a window ID over a WebTransport unidirectional stream.
///
/// Extended frame format:
/// [flags: u8] [window_id: u32 BE] [pts: u64 BE] [length: u32 BE] [H.264 data...]
///   flags: bit 0 = keyframe, bit 1 = window frame (always set here)
pub async fn send_window_video_frame(
    session: &wtransport::Connection,
    window_id: u32,
    frame: &EncodedFrame,
) -> Result<()> {
    let mut stream = session.open_uni().await?.await?;

    let flags: u8 = (if frame.is_keyframe { 0x01 } else { 0x00 }) | 0x02; // bit 1 = window frame
    let mut header = [0u8; 17];
    header[0] = flags;
    header[1..5].copy_from_slice(&window_id.to_be_bytes());
    header[5..13].copy_from_slice(&frame.pts.to_be_bytes());
    header[13..17].copy_from_slice(&(frame.data.len() as u32).to_be_bytes());

    stream.write_all(&header).await?;
    stream.write_all(&frame.data).await?;
    stream.finish().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_monitor::WindowInfo;

    #[test]
    fn serialize_snapshot_event() {
        let event = WindowEvent::Snapshot(vec![WindowInfo {
            window_id: 1,
            title: "Test".into(),
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            visible: true,
            app_class: "test".into(),
        }]);

        let data = serialize_window_event(&event);
        assert_eq!(data[0], 0x40);
        assert_eq!(data[1], 0x01);
        let count = u16::from_be_bytes([data[2], data[3]]);
        assert_eq!(count, 1);
    }

    #[test]
    fn serialize_added_event() {
        let event = WindowEvent::Added(WindowInfo {
            window_id: 42,
            title: "Firefox".into(),
            x: 0,
            y: 0,
            width: 1024,
            height: 768,
            visible: true,
            app_class: "Navigator".into(),
        });

        let data = serialize_window_event(&event);
        assert_eq!(data[0], 0x40);
        assert_eq!(data[1], 0x02);
        let wid = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
        assert_eq!(wid, 42);
    }

    #[test]
    fn serialize_removed_event() {
        let event = WindowEvent::Removed { window_id: 99 };
        let data = serialize_window_event(&event);
        assert_eq!(data[0], 0x40);
        assert_eq!(data[1], 0x03);
        let wid = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
        assert_eq!(wid, 99);
        assert_eq!(data.len(), 6);
    }

    #[test]
    fn serialize_title_changed_event() {
        let event = WindowEvent::TitleChanged {
            window_id: 5,
            title: "New Title".into(),
        };
        let data = serialize_window_event(&event);
        assert_eq!(data[0], 0x40);
        assert_eq!(data[1], 0x05);
        let wid = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
        assert_eq!(wid, 5);
        let title_len = u16::from_be_bytes([data[6], data[7]]) as usize;
        let title = std::str::from_utf8(&data[8..8 + title_len]).unwrap();
        assert_eq!(title, "New Title");
    }

    #[test]
    fn serialize_visibility_changed_event() {
        let event = WindowEvent::VisibilityChanged {
            window_id: 7,
            visible: false,
        };
        let data = serialize_window_event(&event);
        assert_eq!(data[0], 0x40);
        assert_eq!(data[1], 0x06);
        assert_eq!(data[6], 0); // not visible
    }

    // --- Resize dimension processing tests ---
    // These test the full pipeline: round_to_even → clamp_to_display

    #[test]
    fn resize_dimensions_odd_rounded_then_clamped() {
        // Odd 1921x1081 → rounds to 1922x1082 → clamped to 1920x1080
        let w = round_to_even(1921);
        let h = round_to_even(1081);
        assert_eq!((w, h), (1922, 1082));
        let (cw, ch) = clamp_to_display(w, h, 1920, 1080);
        assert_eq!((cw, ch), (1920, 1080));
    }

    #[test]
    fn resize_dimensions_zero_gets_minimum() {
        let w = round_to_even(0);
        let h = round_to_even(0);
        assert_eq!((w, h), (2, 2));
        // Within any reasonable display
        let (cw, ch) = clamp_to_display(w, h, 1920, 1080);
        assert_eq!((cw, ch), (2, 2));
    }

    #[test]
    fn resize_dimensions_u16_max_rounded_and_clamped() {
        // u16::MAX (65535) → rounds to 65534 → clamped to display
        let w = round_to_even(u16::MAX);
        let h = round_to_even(u16::MAX);
        assert_eq!((w, h), (65534, 65534));
        let (cw, ch) = clamp_to_display(w, h, 1920, 1080);
        assert_eq!((cw, ch), (1920, 1080));
    }

    #[test]
    fn resize_dimensions_display_uninitialized_passthrough() {
        // When display_size hasn't been set (packed=0), don't clamp
        let (dw, dh) = crate::pipeline::unpack_display_size(0);
        assert_eq!((dw, dh), (0, 0));
        let (cw, ch) = clamp_to_display(800, 600, dw, dh);
        assert_eq!((cw, ch), (800, 600));
    }

    #[test]
    fn resize_dimensions_within_display_unchanged() {
        let w = round_to_even(800);
        let h = round_to_even(600);
        assert_eq!((w, h), (800, 600));
        let (cw, ch) = clamp_to_display(w, h, 1920, 1080);
        assert_eq!((cw, ch), (800, 600));
    }
}
