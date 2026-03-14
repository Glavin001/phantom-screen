use std::sync::Arc;

use crate::input::InputEvent;
use crate::pipeline::PipelineController;

/// Handle control events from the client
pub fn handle_control_event(event: &InputEvent, controller: &Arc<PipelineController>) {
    match event {
        InputEvent::RequestKeyframe => {
            controller.force_keyframe();
        }
        InputEvent::SetBitrate { kbps } => {
            controller.set_bitrate(*kbps);
        }
        InputEvent::SetResolution { width, height } => {
            tracing::info!("Resolution change requested: {}x{}", width, height);
            tracing::warn!(
                "Dynamic resolution change not yet implemented (requested {}x{}); ignoring request",
                width,
                height
            );
            tracing::info!("Stream continues at current resolution; no pipeline change");
        }
        _ => {}
    }
}
