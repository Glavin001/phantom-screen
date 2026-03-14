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

    /// Create a dummy controller for testing (no real GStreamer pipeline).
    #[cfg(test)]
    pub(crate) fn new_for_test(running: bool) -> Arc<Self> {
        // We need a minimal GStreamer init + a fake pipeline/element to satisfy the struct.
        gstreamer::init().expect("GStreamer init failed in test");
        let pipeline = gstreamer::Pipeline::new();
        let fakesink = gstreamer::ElementFactory::make("fakesink")
            .build()
            .expect("Failed to create fakesink");
        pipeline.add(&fakesink).unwrap();
        Arc::new(Self {
            pipeline,
            encoder: fakesink,
            bitrate: AtomicU32::new(6000),
            running: AtomicBool::new(running),
        })
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

/// Build the encoder portion of a GStreamer pipeline string.
/// Shared between full-desktop and per-window pipelines.
pub(crate) fn build_encoder_string(encoder_type: EncoderType, bitrate: u32, ki: u32) -> String {
    match encoder_type {
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
    }
}

pub(crate) fn build_pipeline_string(config: &Config, encoder_type: EncoderType) -> String {
    let display = &config.display;
    let fps = config.fps;

    // Insert a scale step if stream resolution differs from desktop resolution
    let scale_part = {
        let sw = config.stream_resolution_width();
        let sh = config.stream_resolution_height();
        if sw != config.resolution_width() || sh != config.resolution_height() {
            format!("! videoscale ! video/x-raw,width={sw},height={sh} ")
        } else {
            String::new()
        }
    };

    let encoder_part = build_encoder_string(encoder_type, config.bitrate, config.keyframe_interval);

    format!(
        "ximagesrc display-name={display} use-damage=0 show-pointer=true \
         ! video/x-raw,framerate={fps}/1 \
         {scale_part}\
         ! {encoder_part} \
         ! video/x-h264,stream-format=byte-stream,alignment=au \
         ! appsink name=sink emit-signals=true sync=false"
    )
}

/// Build a GStreamer pipeline string for capturing a specific X11 window by its xid.
pub(crate) fn build_window_pipeline_string(
    display: &str,
    window_id: u32,
    fps: u32,
    bitrate: u32,
    keyframe_interval: u32,
    encoder_type: EncoderType,
) -> String {
    let encoder_part = build_encoder_string(encoder_type, bitrate, keyframe_interval);
    format!(
        "ximagesrc display-name={display} xid={window_id} use-damage=0 show-pointer=true \
         ! video/x-raw,framerate={fps}/1 \
         ! {encoder_part} \
         ! video/x-h264,stream-format=byte-stream,alignment=au \
         ! appsink name=sink emit-signals=true sync=false"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config {
            display: ":99".into(),
            resolution: "1920x1080".into(),
            listen: "0.0.0.0:4443".parse().unwrap(),
            fps: 60,
            bitrate: 6000,
            keyframe_interval: 60,
            cert: None,
            key: None,
            client_dir: "../client/dist/standalone".into(),
            no_xvfb: false,
            wm: "openbox".into(),
            jwt_secret: None,
            post_start_command: None,
            stream_resolution: None,
            launch_apps: "xterm,firefox,chromium --no-sandbox --disable-gpu,l3afpad".into(),
            window_bitrate: 2000,
            max_window_pipelines: 8,
        }
    }

    #[test]
    fn pipeline_string_no_scale_when_resolutions_match() {
        let config = default_config();
        let result = build_pipeline_string(&config, EncoderType::X264);
        assert!(!result.contains("videoscale"));
        assert!(result.contains("ximagesrc display-name=:99"));
        assert!(result.contains("framerate=60/1"));
        assert!(result.contains("x264enc name=encoder"));
        assert!(result.contains("bitrate=6000"));
        assert!(result.contains("key-int-max=60"));
        assert!(result.contains("appsink name=sink"));
    }

    #[test]
    fn pipeline_string_includes_scale_when_stream_resolution_differs() {
        let mut config = default_config();
        config.stream_resolution = Some("1280x720".into());
        let result = build_pipeline_string(&config, EncoderType::X264);
        assert!(result.contains("videoscale"));
        assert!(result.contains("width=1280"));
        assert!(result.contains("height=720"));
    }

    #[test]
    fn pipeline_string_no_scale_when_stream_resolution_matches_desktop() {
        let mut config = default_config();
        config.stream_resolution = Some("1920x1080".into());
        let result = build_pipeline_string(&config, EncoderType::X264);
        assert!(!result.contains("videoscale"));
    }

    #[test]
    fn pipeline_string_nvenc_encoder() {
        let config = default_config();
        let result = build_pipeline_string(&config, EncoderType::Nvenc);
        assert!(result.contains("nvh264enc name=encoder"));
        assert!(result.contains("preset=low-latency-hq"));
        assert!(result.contains("rc-mode=cbr"));
        assert!(result.contains("zerolatency=true"));
        assert!(result.contains("gop-size=60"));
        assert!(!result.contains("x264enc"));
        assert!(!result.contains("vaapih264enc"));
    }

    #[test]
    fn pipeline_string_vaapi_encoder() {
        let config = default_config();
        let result = build_pipeline_string(&config, EncoderType::Vaapi);
        assert!(result.contains("vaapih264enc name=encoder"));
        assert!(result.contains("rate-control=cbr"));
        assert!(result.contains("keyframe-period=60"));
        assert!(!result.contains("x264enc"));
        assert!(!result.contains("nvh264enc"));
    }

    #[test]
    fn pipeline_string_x264_includes_videoconvert() {
        let config = default_config();
        let result = build_pipeline_string(&config, EncoderType::X264);
        assert!(result.contains("videoconvert"));
        assert!(result.contains("video/x-raw,format=I420"));
    }

    #[test]
    fn pipeline_string_custom_display_and_fps() {
        let mut config = default_config();
        config.display = ":42".into();
        config.fps = 30;
        config.bitrate = 3000;
        config.keyframe_interval = 120;
        let result = build_pipeline_string(&config, EncoderType::X264);
        assert!(result.contains("display-name=:42"));
        assert!(result.contains("framerate=30/1"));
        assert!(result.contains("bitrate=3000"));
        assert!(result.contains("key-int-max=120"));
    }

    #[test]
    fn pipeline_string_stream_resolution_with_nvenc() {
        let mut config = default_config();
        config.stream_resolution = Some("640x480".into());
        let result = build_pipeline_string(&config, EncoderType::Nvenc);
        assert!(result.contains("videoscale"));
        assert!(result.contains("width=640"));
        assert!(result.contains("height=480"));
        assert!(result.contains("nvh264enc name=encoder"));
    }

    #[test]
    fn pipeline_string_always_has_byte_stream_format() {
        for encoder in [EncoderType::X264, EncoderType::Nvenc, EncoderType::Vaapi] {
            let result = build_pipeline_string(&default_config(), encoder);
            assert!(
                result.contains("stream-format=byte-stream"),
                "Missing byte-stream for {encoder:?}"
            );
            assert!(
                result.contains("alignment=au"),
                "Missing alignment=au for {encoder:?}"
            );
        }
    }
}
