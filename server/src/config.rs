use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "phantom-screen-server",
    about = "Remote desktop streaming server"
)]
pub struct Config {
    /// X11 display to capture (e.g., ":99")
    #[arg(long, default_value = ":99")]
    pub display: String,

    /// Virtual display resolution
    #[arg(long, default_value = "1920x1080")]
    pub resolution: String,

    /// Listen address for HTTPS/WebTransport
    #[arg(long, default_value = "0.0.0.0:4443")]
    pub listen: SocketAddr,

    /// Video framerate
    #[arg(long, default_value_t = 60)]
    pub fps: u32,

    /// H.264 bitrate in kbps
    #[arg(long, default_value_t = 6000)]
    pub bitrate: u32,

    /// Keyframe interval in frames
    #[arg(long, default_value_t = 60)]
    pub keyframe_interval: u32,

    /// Path to TLS certificate PEM file (auto-generated if not provided)
    #[arg(long)]
    pub cert: Option<String>,

    /// Path to TLS private key PEM file (auto-generated if not provided)
    #[arg(long)]
    pub key: Option<String>,

    /// Path to web client static files directory
    #[arg(long, default_value = "../client/dist/standalone")]
    pub client_dir: String,

    /// Skip starting Xvfb (if already running)
    #[arg(long)]
    pub no_xvfb: bool,

    /// Window manager command to run
    #[arg(long, default_value = "openbox")]
    pub wm: String,

    /// JWT secret for session authentication (optional, disables auth if not set)
    #[arg(long)]
    pub jwt_secret: Option<String>,
}

impl Config {
    pub fn resolution_width(&self) -> u32 {
        self.resolution
            .split('x')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1920)
    }

    pub fn resolution_height(&self) -> u32 {
        self.resolution
            .split('x')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1080)
    }

    pub fn display_num(&self) -> u32 {
        self.display.trim_start_matches(':').parse().unwrap_or(99)
    }
}

/// Detect which H.264 encoder is available via GStreamer
pub fn detect_encoder() -> EncoderType {
    // Try to create each encoder element to see which is available
    if gstreamer::ElementFactory::make("nvh264enc").build().is_ok() {
        tracing::info!("Detected NVIDIA GPU encoder (nvh264enc)");
        return EncoderType::Nvenc;
    }
    if gstreamer::ElementFactory::make("vaapih264enc")
        .build()
        .is_ok()
    {
        tracing::info!("Detected VA-API encoder (vaapih264enc)");
        return EncoderType::Vaapi;
    }

    tracing::info!("Using software encoder (x264enc)");
    EncoderType::X264
}

#[derive(Debug, Clone, Copy)]
pub enum EncoderType {
    X264,
    Nvenc,
    Vaapi,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_parsing() {
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
        };

        assert_eq!(config.resolution_width(), 1920);
        assert_eq!(config.resolution_height(), 1080);
    }

    #[test]
    fn test_resolution_parsing_custom() {
        let config = Config {
            display: ":1".into(),
            resolution: "2560x1440".into(),
            listen: "127.0.0.1:9000".parse().unwrap(),
            fps: 30,
            bitrate: 4000,
            keyframe_interval: 30,
            cert: Some("/path/to/cert.pem".into()),
            key: Some("/path/to/key.pem".into()),
            client_dir: "/var/www".into(),
            no_xvfb: true,
            wm: "fluxbox".into(),
            jwt_secret: Some("secret".into()),
        };

        assert_eq!(config.resolution_width(), 2560);
        assert_eq!(config.resolution_height(), 1440);
        assert_eq!(config.display_num(), 1);
    }

    #[test]
    fn test_resolution_parsing_invalid_fallback() {
        let config = Config {
            display: ":abc".into(),
            resolution: "invalid".into(),
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
        };

        assert_eq!(config.resolution_width(), 1920); // fallback
        assert_eq!(config.resolution_height(), 1080); // fallback
        assert_eq!(config.display_num(), 99); // fallback
    }

    #[test]
    fn test_display_num() {
        let config = Config {
            display: ":42".into(),
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
        };
        assert_eq!(config.display_num(), 42);
    }
}
