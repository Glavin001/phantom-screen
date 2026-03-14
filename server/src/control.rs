use std::sync::Arc;

use crate::input::InputEvent;
use crate::pipeline::PipelineManager;

/// Handle control events from the client
pub fn handle_control_event(event: &InputEvent, manager: &Arc<PipelineManager>) {
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
            }
        }
        _ => {}
    }
}
