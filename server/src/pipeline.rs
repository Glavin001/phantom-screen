use anyhow::{Context, Result};
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use tokio::sync::{broadcast, watch};

use crate::config::{Config, EncoderType, detect_encoder};
use crate::input::InputHandler;

/// Query the current resolution of an X11 display using xdpyinfo.
///
/// Returns `(width, height)` or an error if the display cannot be queried.
pub fn query_display_resolution(display: &str) -> Result<(u16, u16)> {
    use std::process::Command;

    let output = Command::new("xdpyinfo")
        .env("DISPLAY", display)
        .output()
        .context("Failed to run xdpyinfo")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Look for "dimensions:    1920x1080 pixels"
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("dimensions:") {
            // e.g. "dimensions:    1920x1080 pixels"
            if let Some(dim_str) = trimmed
                .strip_prefix("dimensions:")
                .and_then(|s| s.split_whitespace().next())
            {
                let mut parts = dim_str.split('x');
                if let (Some(w), Some(h)) = (parts.next(), parts.next()) {
                    let width: u16 = w.parse().context("Invalid width from xdpyinfo")?;
                    let height: u16 = h.parse().context("Invalid height from xdpyinfo")?;
                    return Ok((width, height));
                }
            }
        }
    }

    anyhow::bail!("Could not parse display resolution from xdpyinfo output")
}

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
        // Wait for state change to complete so ximagesrc closes its X connection
        // before Xvfb is killed during resize
        let _ = self.pipeline.state(gstreamer::ClockTime::from_seconds(5));
        tracing::info!("Pipeline stopped");
    }
}

/// Manages pipeline lifecycle including dynamic restarts for resolution changes.
///
/// Sessions subscribe to `frame_watch` to receive new broadcast senders whenever the
/// pipeline restarts. On each restart the old broadcast sender is dropped (causing
/// receivers to see `Closed`), and a new sender is published through the watch channel.
/// Callback invoked before Xvfb is killed for resize, allowing other
/// subsystems to shut down their X connections gracefully.
pub type PreResizeHook = Box<dyn Fn() + Send + Sync>;

pub struct PipelineManager {
    config: Mutex<Config>,
    encoder_type: EncoderType,
    controller: Mutex<Arc<PipelineController>>,
    frame_watch_tx: watch::Sender<broadcast::Sender<EncodedFrame>>,
    input_handler: Option<Arc<InputHandler>>,
    post_start_command: Option<String>,
    pre_resize_hooks: Mutex<Vec<PreResizeHook>>,
    post_resize_hooks: Mutex<Vec<PreResizeHook>>,
}

impl PipelineManager {
    /// Register a hook that runs before Xvfb is killed for resize.
    /// Use this to stop per-window pipelines or other X clients.
    pub fn add_pre_resize_hook(&self, hook: PreResizeHook) {
        self.pre_resize_hooks.lock().unwrap().push(hook);
    }

    /// Register a hook that runs after resize completes (new pipeline is running).
    /// Use this to reconnect X clients to the new display.
    pub fn add_post_resize_hook(&self, hook: PreResizeHook) {
        self.post_resize_hooks.lock().unwrap().push(hook);
    }

    /// Resize the display and restart the pipeline.
    ///
    /// 1. Stops all GStreamer pipelines (main + per-window via hooks).
    /// 2. Kills and restarts Xvfb at the new resolution.
    /// 3. Starts a new main pipeline.
    /// 4. Publishes the new broadcast sender so sessions re-subscribe.
    pub fn resize(&self, width: u16, height: u16) -> Result<()> {
        // Round up to even dimensions — x264enc/I420 require even width and height
        // for 4:2:0 chroma subsampling.
        let width = (width + 1) & !1;
        let height = (height + 1) & !1;

        let mut config = self.config.lock().unwrap();
        let current_w = config.resolution_width() as u16;
        let current_h = config.resolution_height() as u16;

        if current_w == width && current_h == height {
            tracing::debug!(
                "Resize requested but resolution unchanged ({}x{})",
                width,
                height
            );
            return Ok(());
        }

        tracing::info!(
            "Resizing display from {}x{} to {}x{}",
            current_w,
            current_h,
            width,
            height
        );

        // Stop old pipeline first (before killing Xvfb to avoid GStreamer errors)
        {
            let controller = self.controller.lock().unwrap();
            controller.stop();
        }

        // Run pre-resize hooks (e.g., stop per-window pipelines)
        // This ensures all GStreamer X connections are closed before Xvfb is killed.
        {
            let hooks = self.pre_resize_hooks.lock().unwrap();
            for hook in hooks.iter() {
                hook();
            }
        }

        // Brief pause to let GStreamer elements fully release X connections
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Resize the virtual display (kills and restarts Xvfb)
        resize_display(&config.display, width, height)?;

        // Update config
        config.resolution = format!("{}x{}", width, height);
        // Clear stream_resolution so new pipeline captures at native size
        config.stream_resolution = None;

        // Brief pause for X server to settle after resize
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Reconnect the input handler to the new X display
        if let Some(ref ih) = self.input_handler
            && let Err(e) = ih.reconnect()
        {
            tracing::error!("Failed to reconnect input handler after resize: {}", e);
        }

        // Restart the post-start command (e.g. xterm) since it lost its X connection
        if let Some(ref cmd) = self.post_start_command {
            let display = &config.display;
            tracing::info!("Restarting post-start command after resize: {}", cmd);
            let _ = std::process::Command::new("sh")
                .args(["-c", cmd])
                .env("DISPLAY", display)
                .spawn()
                .map_err(|e| tracing::error!("Failed to restart post-start command: {}", e));
        }

        // Start new pipeline
        let (new_tx, new_controller) = start_pipeline_inner(&config, self.encoder_type)?;

        // Swap in new controller
        {
            let mut controller = self.controller.lock().unwrap();
            *controller = new_controller;
        }

        // Publish new broadcast sender - sessions will re-subscribe
        let _ = self.frame_watch_tx.send(new_tx);

        tracing::info!("Pipeline restarted at {}x{}", width, height);

        // Run post-resize hooks (e.g., reconnect WindowManager)
        {
            let hooks = self.post_resize_hooks.lock().unwrap();
            for hook in hooks.iter() {
                hook();
            }
        }

        Ok(())
    }

    /// Restart the pipeline without changing the display.
    ///
    /// Use this when the display resolution changed externally (e.g. xrandr)
    /// and the pipeline needs to re-capture at the new size.
    pub fn restart_pipeline(&self, new_width: u16, new_height: u16) -> Result<()> {
        // Round up to even dimensions — x264enc/I420 require even width and height
        let new_width = (new_width + 1) & !1;
        let new_height = (new_height + 1) & !1;

        let mut config = self.config.lock().unwrap();

        tracing::info!(
            "Restarting pipeline for external resolution change to {}x{}",
            new_width,
            new_height
        );

        // Update config to reflect the new display resolution
        config.resolution = format!("{}x{}", new_width, new_height);
        config.stream_resolution = None;

        // Stop old pipeline
        {
            let controller = self.controller.lock().unwrap();
            controller.stop();
        }

        // Brief pause for stability
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Start new pipeline at new resolution
        let (new_tx, new_controller) = start_pipeline_inner(&config, self.encoder_type)?;

        {
            let mut controller = self.controller.lock().unwrap();
            *controller = new_controller;
        }

        let _ = self.frame_watch_tx.send(new_tx);
        tracing::info!("Pipeline restarted at {}x{}", new_width, new_height);
        Ok(())
    }

    /// Get the current pipeline controller (for keyframe/bitrate requests).
    pub fn controller(&self) -> Arc<PipelineController> {
        self.controller.lock().unwrap().clone()
    }

    /// Subscribe to pipeline changes. Returns a watch receiver that yields
    /// a new broadcast::Sender each time the pipeline restarts.
    pub fn subscribe_watch(&self) -> watch::Receiver<broadcast::Sender<EncodedFrame>> {
        self.frame_watch_tx.subscribe()
    }

    pub fn is_running(&self) -> bool {
        self.controller.lock().unwrap().is_running()
    }

    pub fn stop(&self) {
        self.controller.lock().unwrap().stop();
    }

    pub fn force_keyframe(&self) {
        self.controller.lock().unwrap().force_keyframe();
    }

    pub fn set_bitrate(&self, kbps: u32) {
        self.controller.lock().unwrap().set_bitrate(kbps);
    }

    /// Returns the current display resolution as (width, height).
    pub fn current_resolution(&self) -> (u16, u16) {
        let config = self.config.lock().unwrap();
        (
            config.resolution_width() as u16,
            config.resolution_height() as u16,
        )
    }

    /// Create a dummy manager for testing (no real GStreamer pipeline or display).
    #[cfg(test)]
    pub(crate) fn new_for_test(running: bool) -> Arc<Self> {
        let pc = PipelineController::new_for_test(running);
        let (tx, _) = broadcast::channel::<EncodedFrame>(4);
        let (watch_tx, _) = watch::channel(tx);
        let config = Config {
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
            launch_apps: "xterm".into(),
            window_bitrate: 2000,
            max_window_pipelines: 8,
        };
        Arc::new(Self {
            config: Mutex::new(config),
            encoder_type: EncoderType::X264,
            controller: Mutex::new(pc),
            frame_watch_tx: watch_tx,
            input_handler: None,
            post_start_command: None,
            pre_resize_hooks: Mutex::new(Vec::new()),
            post_resize_hooks: Mutex::new(Vec::new()),
        })
    }
}

/// Resize the Xvfb display by restarting it at the new resolution.
///
/// Xvfb has limited RANDR support and cannot add new modes dynamically.
/// The reliable approach is to kill the current Xvfb and start a fresh one
/// at the requested resolution with BackingStore and RANDR enabled.
pub fn resize_display(display: &str, width: u16, height: u16) -> Result<()> {
    use std::process::Command;

    let resolution = format!("{}x{}", width, height);

    // Kill existing Xvfb on this display
    // Find Xvfb processes matching our display
    let pgrep = Command::new("pgrep").args(["-a", "-x", "Xvfb"]).output();
    if let Ok(output) = pgrep {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains(display)
                && let Some(pid_str) = line.split_whitespace().next()
                && let Ok(pid) = pid_str.parse::<i32>()
            {
                tracing::info!("Killing existing Xvfb (pid {}) for resize", pid);
                let _ = Command::new("kill").args(["-TERM", pid_str]).output();
            }
        }
    }

    // Wait for old Xvfb to die and release the display
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Remove stale lock file
    let display_num = display.trim_start_matches(':');
    let lock_file = format!("/tmp/.X{}-lock", display_num);
    let _ = std::fs::remove_file(&lock_file);

    // Start new Xvfb at the requested resolution
    let child = Command::new("Xvfb")
        .args([
            display,
            "-screen",
            "0",
            &format!("{}x24", resolution),
            "-ac",
            "+bs",
            "+extension",
            "RANDR",
            "+extension",
            "Composite",
        ])
        .spawn()
        .context("Failed to start new Xvfb")?;

    {
        let d = display;
        tracing::info!(
            "Started new Xvfb (pid={}) at {} on {}",
            child.id(),
            resolution,
            d
        );
    }

    // Wait for new Xvfb to be ready
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Restart window manager on the display
    let _ = Command::new("sh")
        .args(["-c", &format!("DISPLAY={} openbox &", display)])
        .spawn();

    std::thread::sleep(std::time::Duration::from_millis(100));
    tracing::info!("Display resized to {} via Xvfb restart", resolution);
    Ok(())
}

/// Start the GStreamer capture/encode pipeline.
///
/// Returns a broadcast sender/receiver pair and a pipeline controller.
pub fn start_pipeline(
    config: &Config,
    input_handler: Option<Arc<InputHandler>>,
    post_start_command: Option<String>,
) -> Result<(broadcast::Receiver<EncodedFrame>, Arc<PipelineManager>)> {
    gstreamer::init().context("Failed to init GStreamer")?;

    let encoder_type = detect_encoder();
    let (frame_tx, new_controller) = start_pipeline_inner(config, encoder_type)?;

    let frame_rx = frame_tx.subscribe();
    let (watch_tx, _) = watch::channel(frame_tx);

    let manager = Arc::new(PipelineManager {
        config: Mutex::new(config.clone()),
        encoder_type,
        controller: Mutex::new(new_controller),
        frame_watch_tx: watch_tx,
        input_handler,
        post_start_command,
        pre_resize_hooks: Mutex::new(Vec::new()),
        post_resize_hooks: Mutex::new(Vec::new()),
    });

    Ok((frame_rx, manager))
}

/// Spawn a background task that monitors the X display for resolution changes.
///
/// Polls `xdpyinfo` at regular intervals and restarts the pipeline when the
/// display resolution differs from the pipeline's current config. This handles
/// the case where something external (xrandr, an app) changes the display
/// resolution without going through the client resize path.
pub fn spawn_resolution_monitor(
    pipeline_manager: Arc<PipelineManager>,
    display: String,
    poll_interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        // First tick completes immediately; skip it so we don't check on startup
        interval.tick().await;

        loop {
            interval.tick().await;

            let (current_w, current_h) = {
                let config = pipeline_manager.config.lock().unwrap();
                (
                    config.resolution_width() as u16,
                    config.resolution_height() as u16,
                )
            };

            match query_display_resolution(&display) {
                Ok((actual_w, actual_h)) => {
                    if actual_w != current_w || actual_h != current_h {
                        tracing::info!(
                            "Display resolution changed externally: {}x{} -> {}x{}",
                            current_w,
                            current_h,
                            actual_w,
                            actual_h
                        );
                        if let Err(e) = pipeline_manager.restart_pipeline(actual_w, actual_h) {
                            tracing::error!(
                                "Failed to restart pipeline after external resize: {}",
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("Resolution monitor: could not query display: {}", e);
                }
            }
        }
    })
}

/// Internal: create a new pipeline, returning the broadcast sender and controller.
fn start_pipeline_inner(
    config: &Config,
    encoder_type: EncoderType,
) -> Result<(broadcast::Sender<EncodedFrame>, Arc<PipelineController>)> {
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

    let (tx, _rx) = broadcast::channel::<EncodedFrame>(120);
    let tx_for_sink = tx.clone();

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
                let _ = tx_for_sink.send(frame);

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

    Ok((tx, controller))
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
