use anyhow::{Context, Result};
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use tokio::sync::broadcast;

use crate::config::{Config, EncoderType, detect_encoder};

/// Encoded H.264 frame data
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub pts: u64,
    pub is_keyframe: bool,
}

/// Controls for the running pipeline
pub struct PipelineController {
    pipeline: gstreamer::Pipeline,
    encoder: gstreamer::Element,
    bitrate: AtomicU32,
    running: AtomicBool,
}

impl PipelineController {
    /// Request a keyframe from the encoder
    pub fn force_keyframe(&self) {
        let event = gstreamer_video::UpstreamForceKeyUnitEvent::builder()
            .all_headers(true)
            .build();
        self.encoder.send_event(event);
        tracing::debug!("Forced keyframe");
    }

    /// Update encoder bitrate (in kbps)
    pub fn set_bitrate(&self, kbps: u32) {
        self.bitrate.store(kbps, Ordering::Relaxed);
        self.encoder.set_property("bitrate", kbps);
        tracing::info!("Bitrate set to {} kbps", kbps);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gstreamer::State::Null);
        tracing::info!("Pipeline stopped");
    }
}

/// Start the GStreamer capture/encode pipeline
///
/// Returns a broadcast channel of encoded frames and a pipeline controller.
pub fn start_pipeline(
    config: &Config,
) -> Result<(broadcast::Receiver<EncodedFrame>, Arc<PipelineController>)> {
    gstreamer::init().context("Failed to init GStreamer")?;

    let encoder_type = detect_encoder();
    let pipeline_str = build_pipeline_string(config, encoder_type);

    tracing::info!("Starting GStreamer pipeline: {}", pipeline_str);

    let pipeline = gstreamer::parse::launch(&pipeline_str)
        .context("Failed to parse GStreamer pipeline")?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("Pipeline is not a GstPipeline"))?;

    let appsink = pipeline
        .by_name("sink")
        .context("No element named 'sink' in pipeline")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("Element 'sink' is not an AppSink"))?;

    let encoder = pipeline
        .by_name("encoder")
        .context("No element named 'encoder' in pipeline")?;

    let (tx, rx) = broadcast::channel::<EncodedFrame>(120);

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

                let frame = EncodedFrame {
                    data: map.to_vec(),
                    pts,
                    is_keyframe,
                };

                // If no receivers, frames are dropped (that's fine)
                let _ = tx.send(frame);

                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    pipeline
        .set_state(gstreamer::State::Playing)
        .context("Failed to start pipeline")?;

    let controller = Arc::new(PipelineController {
        pipeline,
        encoder,
        bitrate: AtomicU32::new(config.bitrate),
        running: AtomicBool::new(true),
    });

    Ok((rx, controller))
}

fn build_pipeline_string(config: &Config, encoder_type: EncoderType) -> String {
    let display = &config.display;
    let fps = config.fps;
    let bitrate = config.bitrate;
    let ki = config.keyframe_interval;

    let encoder_part = match encoder_type {
        EncoderType::X264 => {
            format!(
                "videoconvert ! video/x-raw,format=I420 ! \
                 x264enc name=encoder tune=zerolatency speed-preset=ultrafast \
                 bitrate={bitrate} key-int-max={ki} bframes=0"
            )
        }
        EncoderType::Nvenc => {
            format!(
                "nvh264enc name=encoder preset=low-latency-hq rc-mode=cbr \
                 bitrate={bitrate} gop-size={ki} bframes=0 zerolatency=true"
            )
        }
        EncoderType::Vaapi => {
            format!(
                "vaapih264enc name=encoder rate-control=cbr \
                 bitrate={bitrate} keyframe-period={ki}"
            )
        }
    };

    format!(
        "ximagesrc display-name={display} use-damage=0 show-pointer=true \
         ! video/x-raw,framerate={fps}/1 \
         ! {encoder_part} \
         ! video/x-h264,stream-format=byte-stream,alignment=au \
         ! appsink name=sink emit-signals=true sync=false"
    )
}
