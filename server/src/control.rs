use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::input::InputEvent;
use crate::pipeline::{PipelineManager, pack_display_size};

/// Handle control events from the client.
/// `display_size` is an optional packed (width<<16|height) atomic updated after
/// a successful display resize, used by coherence to clamp per-window sizes.
pub fn handle_control_event(
    event: &InputEvent,
    manager: &Arc<PipelineManager>,
    display_size: Option<&Arc<AtomicU32>>,
) {
    match event {
        InputEvent::RequestKeyframe => {
            manager.force_keyframe();
        }
        InputEvent::SetBitrate { kbps } => {
            manager.set_bitrate(*kbps);
        }
        InputEvent::SetResolution { width, height } => {
            tracing::info!("Resolution change requested: {}x{}", width, height);
            if let Err(e) = manager.resize(*width, *height) {
                tracing::error!("Failed to resize display: {}", e);
            } else if let Some(ds) = display_size {
                // Update the shared display size after a successful resize.
                // PipelineManager rounds to even, so read the actual value back.
                let (w, h) = manager.current_resolution();
                ds.store(
                    pack_display_size(w, h),
                    std::sync::atomic::Ordering::Release,
                );
                tracing::info!(
                    "Updated coherence display_size to {}x{} for window clamping",
                    w,
                    h
                );
            }
        }
        _ => {}
    }
}
