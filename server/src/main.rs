#![allow(dead_code)]

mod auth;
mod config;
mod control;
mod input;
mod pipeline;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use config::Config;
use input::{InputEvent, InputHandler, estimate_event_length, parse_input_event};
use pipeline::{EncodedFrame, PipelineManager};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "phantom_screen_server=info".parse().unwrap()),
        )
        .init();

    let config = Config::parse();
    info!("Phantom Screen Server starting");
    info!(
        display = %config.display,
        resolution = %config.resolution,
        fps = config.fps,
        bitrate = config.bitrate,
        "Configuration"
    );

    let mut children: Vec<Child> = Vec::new();

    // Start virtual display if needed
    if !config.no_xvfb {
        children.push(start_xvfb(&config)?);
    }

    // Start window manager
    children.push(start_window_manager(&config)?);

    // Wait for display to be ready
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Launch post-start command (e.g. a demo app) if configured
    if let Some(ref cmd) = config.post_start_command {
        info!("Launching post-start command: {}", cmd);
        let child = Command::new("sh")
            .args(["-c", cmd])
            .spawn()
            .context("Failed to start post-start command")?;
        children.push(child);
    }

    // Start GStreamer pipeline
    let (_frame_rx, pipeline_manager) =
        pipeline::start_pipeline(&config).context("Failed to start pipeline")?;
    info!("GStreamer pipeline running");

    // Monitor the X display for external resolution changes (e.g. xrandr, apps)
    let pm_for_monitor = pipeline_manager.clone();
    let display_for_monitor = config.display.clone();
    pipeline::spawn_resolution_monitor(pm_for_monitor, display_for_monitor, Duration::from_secs(2));

    // Create input handler
    let input_handler =
        Arc::new(InputHandler::new(&config.display).context("Failed to create input handler")?);

    // Build WebTransport server config
    let identity = if let (Some(cert_path), Some(key_path)) = (&config.cert, &config.key) {
        wtransport::Identity::load_pemfiles(cert_path, key_path)
            .await
            .context("Failed to load TLS certificate/key")?
    } else {
        info!("No TLS cert/key provided, generating self-signed certificate");
        wtransport::Identity::self_signed(["localhost", "127.0.0.1", "::1"])
            .context("Failed to generate self-signed certificate")?
    };

    // Compute certificate hash for client connection URL
    let cert_hash_hex = {
        let chain = identity.certificate_chain();
        let certs = chain.as_slice();
        if let Some(leaf) = certs.first() {
            let digest = leaf.hash();
            digest
                .fmt(wtransport::tls::Sha256DigestFmt::DottedHex)
                .replace(':', "")
        } else {
            String::new()
        }
    };

    let server_config = wtransport::ServerConfig::builder()
        .with_bind_address(config.listen)
        .with_identity(identity)
        .keep_alive_interval(Some(Duration::from_secs(3)))
        .build();

    let server = wtransport::Endpoint::server(server_config)?;

    info!("WebTransport server listening on {}", config.listen);

    // Also start an HTTP server for serving static files and health checks
    let http_addr = SocketAddr::new(config.listen.ip(), config.listen.port() + 1);
    let client_dir = config.client_dir.clone();
    let pm_for_http = pipeline_manager.clone();
    tokio::spawn(async move {
        if let Err(e) = run_http_server(http_addr, client_dir, pm_for_http).await {
            error!("HTTP server error: {}", e);
        }
    });
    info!("HTTP server on port {}", http_addr.port());
    if !cert_hash_hex.is_empty() {
        info!("Certificate SHA-256: {}", cert_hash_hex);
        info!(
            "Connect at: http://127.0.0.1:{}/?serverUrl=https://127.0.0.1:{}&certHash={}&autoconnect=1",
            http_addr.port(),
            config.listen.port(),
            cert_hash_hex
        );
    } else {
        info!("Open http://127.0.0.1:{} in browser", http_addr.port());
    }

    // Accept WebTransport sessions until a shutdown signal is received
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("Shutdown signal received, cleaning up");
                break;
            }
            incoming = server.accept() => {
                let session_request = match incoming.await {
                    Ok(req) => req,
                    Err(e) => {
                        warn!("Failed to accept incoming session: {}", e);
                        continue;
                    }
                };

                let path = session_request.path().to_string();
                info!("WebTransport session request for path: {}", path);

                // JWT authentication (if configured)
                if let Some(ref secret) = config.jwt_secret {
                    match auth::extract_token_from_path(&path) {
                        Some(token) => match auth::validate_token(token, secret) {
                            Ok(data) => {
                                info!("Authenticated session for user: {}", data.claims.sub);
                            }
                            Err(e) => {
                                warn!("JWT validation failed: {}", e);
                                continue;
                            }
                        },
                        None => {
                            warn!("No JWT token in session path, rejecting");
                            continue;
                        }
                    }
                }

                let session = match session_request.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to accept session request: {}", e);
                        continue;
                    }
                };

                let pm = pipeline_manager.clone();
                let ih = input_handler.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_session(session, pm, ih).await {
                        warn!("Session ended: {}", e);
                    }
                });
            }
        }
    }

    // Graceful shutdown: stop pipeline then kill all child processes
    pipeline_manager.stop();
    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    info!("Shutdown complete");

    Ok(())
}

/// Resolves on SIGTERM or Ctrl-C.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn handle_session(
    session: wtransport::Connection,
    pipeline_manager: Arc<PipelineManager>,
    input_handler: Arc<InputHandler>,
) -> Result<()> {
    info!("Client connected");

    let session = Arc::new(session);

    // Spawn video sender that handles pipeline restarts via watch channel
    {
        let session_video = session.clone();
        let mut watch_rx = pipeline_manager.subscribe_watch();

        tokio::spawn(async move {
            loop {
                // Subscribe to the current pipeline's broadcast channel
                let mut frame_rx = watch_rx.borrow_and_update().subscribe();

                loop {
                    tokio::select! {
                        frame_result = frame_rx.recv() => {
                            match frame_result {
                                Ok(frame) => {
                                    if let Err(e) = send_video_frame(&session_video, &frame).await {
                                        warn!("Failed to send video frame: {}", e);
                                        return;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    warn!("Video receiver lagged by {} frames, skipping", n);
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    // Pipeline stopped, wait for restart
                                    break;
                                }
                            }
                        }
                        result = watch_rx.changed() => {
                            if result.is_err() {
                                // PipelineManager dropped, shut down
                                info!("Pipeline manager closed, stopping video sender");
                                return;
                            }
                            // New pipeline available, re-subscribe
                            info!("Pipeline restarted, re-subscribing to video frames");
                            break;
                        }
                    }
                }
            }
        });
    }

    // Handle bidirectional streams for input
    loop {
        match session.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let ih = input_handler.clone();
                let pm = pipeline_manager.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    loop {
                        match recv.read(&mut buf).await {
                            Ok(Some(n)) if n > 0 => {
                                process_input_data(&buf[..n], &ih, &pm);
                            }
                            Ok(_) => break,
                            Err(e) => {
                                warn!("Input stream error: {}", e);
                                break;
                            }
                        }
                    }
                    // Close our side
                    let _ = send.finish().await;
                });
            }
            Err(e) => {
                info!("Session closed: {}", e);
                return Ok(());
            }
        }
    }
}

/// Send an encoded video frame over a unidirectional stream
///
/// Frame format:
/// [flags: u8] [pts: u64 BE] [length: u32 BE] [H.264 data...]
///   flags: bit 0 = keyframe
async fn send_video_frame(session: &wtransport::Connection, frame: &EncodedFrame) -> Result<()> {
    let mut stream = session.open_uni().await?.await?;

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

fn process_input_data(
    data: &[u8],
    input_handler: &InputHandler,
    pipeline_manager: &Arc<PipelineManager>,
) {
    let mut offset = 0;
    while offset < data.len() {
        let remaining = &data[offset..];
        let event_len = estimate_event_length(remaining);
        if event_len == 0 || offset + event_len > data.len() {
            break;
        }

        if let Some(event) = parse_input_event(&remaining[..event_len])
            && let Err(e) = dispatch_event(&event, input_handler, pipeline_manager)
        {
            warn!("Failed to dispatch input event: {}", e);
        }

        offset += event_len;
    }
}

fn dispatch_event(
    event: &InputEvent,
    input_handler: &InputHandler,
    pipeline_manager: &Arc<PipelineManager>,
) -> Result<()> {
    match event {
        InputEvent::MouseMove { x, y } => input_handler.mouse_move(*x, *y)?,
        InputEvent::MouseButton { button, pressed } => {
            input_handler.mouse_button(*button, *pressed)?
        }
        InputEvent::MouseScroll { dx, dy } => input_handler.mouse_scroll(*dx, *dy)?,
        InputEvent::KeyEvent { code, pressed } => input_handler.key_event(code, *pressed)?,
        InputEvent::Clipboard { text } => input_handler.set_clipboard(text)?,
        InputEvent::RequestKeyframe
        | InputEvent::SetBitrate { .. }
        | InputEvent::SetResolution { .. } => {
            control::handle_control_event(event, pipeline_manager);
        }
    }
    Ok(())
}

/// Remove a stale X lock file if it exists. Returns true if a file was removed.
fn cleanup_stale_display(display_num: u32) -> bool {
    let lock_file = format!("/tmp/.X{}-lock", display_num);
    if std::path::Path::new(&lock_file).exists() {
        let _ = std::fs::remove_file(&lock_file);
        info!("Removed stale X lock file: {}", lock_file);
        return true;
    }
    false
}

/// Start Xvfb, removing any stale lock file first. Returns the child process handle.
fn start_xvfb(config: &Config) -> Result<Child> {
    let disp = &config.display;
    let resolution = &config.resolution;

    cleanup_stale_display(config.display_num());

    info!("Starting Xvfb on display {}", disp);

    let child = Command::new("Xvfb")
        .args([
            disp,
            "-screen",
            "0",
            &format!("{}x24", resolution),
            "-ac",
            "+bs", // Enable BackingStore so obscured windows retain their pixels
            "+extension",
            "RANDR", // Enable RandR for dynamic resolution changes
        ])
        .spawn()
        .context("Failed to start Xvfb")?;

    // SAFETY: called during single-threaded init before any tasks are spawned
    unsafe { std::env::set_var("DISPLAY", disp) };
    std::thread::sleep(Duration::from_millis(300));
    info!("Xvfb started");
    Ok(child)
}

/// Start the window manager. Returns the child process handle.
fn start_window_manager(config: &Config) -> Result<Child> {
    let wm = &config.wm;
    info!("Starting window manager: {}", wm);

    let child = Command::new(wm).spawn().context(format!(
        "Failed to start window manager '{}'. Is it installed?",
        wm
    ))?;

    info!("Window manager started");
    Ok(child)
}

/// Simple HTTP server for serving static files and the /health endpoint
async fn run_http_server(
    addr: SocketAddr,
    client_dir: String,
    pipeline_manager: Arc<PipelineManager>,
) -> Result<()> {
    use hyper::body::Incoming;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;

    let client_dir = Arc::new(PathBuf::from(client_dir));

    let listener = tokio::net::TcpListener::bind(addr).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let client_dir = client_dir.clone();
        let pm = pipeline_manager.clone();

        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |req: Request<Incoming>| {
                let client_dir = client_dir.clone();
                let pm = pm.clone();
                async move {
                    let path = req.uri().path();

                    // Health/readiness endpoint
                    if path == "/health" {
                        let status = if pm.is_running() { "ready" } else { "starting" };
                        let body = format!(r#"{{"status":"{}"}}"#, status);
                        return Ok::<_, Infallible>(
                            Response::builder()
                                .status(200)
                                .header("Content-Type", "application/json")
                                .header("Access-Control-Allow-Origin", "*")
                                .body(http_body_util::Full::new(bytes::Bytes::from(body)))
                                .unwrap(),
                        );
                    }

                    let file_path = if path == "/" || path.is_empty() {
                        client_dir.join("index.html")
                    } else {
                        client_dir.join(path.trim_start_matches('/'))
                    };

                    match tokio::fs::read(&file_path).await {
                        Ok(contents) => {
                            let content_type = match file_path.extension().and_then(|e| e.to_str())
                            {
                                Some("html") => "text/html; charset=utf-8",
                                Some("js") => "application/javascript; charset=utf-8",
                                Some("css") => "text/css; charset=utf-8",
                                Some("json") => "application/json",
                                Some("wasm") => "application/wasm",
                                Some("png") => "image/png",
                                Some("svg") => "image/svg+xml",
                                _ => "application/octet-stream",
                            };

                            Ok(Response::builder()
                                .header("Content-Type", content_type)
                                .header("Access-Control-Allow-Origin", "*")
                                .body(http_body_util::Full::new(bytes::Bytes::from(contents)))
                                .unwrap())
                        }
                        Err(_) => Ok(Response::builder()
                            .status(404)
                            .body(http_body_util::Full::new(bytes::Bytes::from("Not Found")))
                            .unwrap()),
                    }
                }
            });

            if let Err(e) =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), service)
                    .await
            {
                warn!("HTTP connection error: {}", e);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod lock_file_cleanup {
        use super::*;

        #[test]
        fn removes_existing_lock_file() {
            let display_num = 12345;
            let lock_path = format!("/tmp/.X{}-lock", display_num);

            // Create a fake lock file
            std::fs::write(&lock_path, "12345\n").unwrap();
            assert!(std::path::Path::new(&lock_path).exists());

            let removed = cleanup_stale_display(display_num);
            assert!(removed);
            assert!(!std::path::Path::new(&lock_path).exists());
        }

        #[test]
        fn returns_false_when_no_lock_file() {
            let display_num = 12346;
            let lock_path = format!("/tmp/.X{}-lock", display_num);

            // Ensure it doesn't exist
            let _ = std::fs::remove_file(&lock_path);

            let removed = cleanup_stale_display(display_num);
            assert!(!removed);
        }
    }

    mod health_endpoint {
        use super::*;

        /// Unit-test the health response logic without a real HTTP server.
        /// This mirrors the `/health` branch inside run_http_server.
        fn health_response_body(is_running: bool) -> String {
            let status = if is_running { "ready" } else { "starting" };
            format!(r#"{{"status":"{}"}}"#, status)
        }

        #[test]
        fn health_body_ready_when_running() {
            let body = health_response_body(true);
            assert_eq!(body, r#"{"status":"ready"}"#);
        }

        #[test]
        fn health_body_starting_when_not_running() {
            let body = health_response_body(false);
            assert_eq!(body, r#"{"status":"starting"}"#);
        }

        #[test]
        fn health_body_is_valid_json_format() {
            for running in [true, false] {
                let body = health_response_body(running);
                // Verify it looks like valid JSON with expected structure
                assert!(body.starts_with(r#"{"status":""#));
                assert!(body.ends_with(r#""}"#));
                assert!(body.contains("ready") || body.contains("starting"));
            }
        }

        #[test]
        fn pipeline_controller_running_state() {
            let pc = pipeline::PipelineController::new_for_test(true);
            assert!(pc.is_running());

            let pc2 = pipeline::PipelineController::new_for_test(false);
            assert!(!pc2.is_running());
        }

        #[test]
        fn pipeline_controller_stop_changes_state() {
            let pc = pipeline::PipelineController::new_for_test(true);
            assert!(pc.is_running());
            pc.stop();
            assert!(!pc.is_running());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn http_server_health_endpoint() {
            let pm = pipeline::PipelineManager::new_for_test(true);
            let client_dir = "/nonexistent".to_string();
            let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

            // Use run_http_server directly
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            let bound_addr = listener.local_addr().unwrap();
            drop(listener); // Release so run_http_server can bind

            tokio::spawn(run_http_server(bound_addr, client_dir, pm));

            // Give it time to bind
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Use hyper client to make a proper HTTP/1.1 request
            use hyper_util::rt::TokioIo;
            let stream = tokio::net::TcpStream::connect(bound_addr).await.unwrap();
            let io = TokioIo::new(stream);

            let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
            tokio::spawn(async move {
                let _ = conn.await;
            });

            let req = hyper::Request::builder()
                .uri("/health")
                .header("Host", "localhost")
                .body(http_body_util::Empty::<bytes::Bytes>::new())
                .unwrap();

            let resp = sender.send_request(req).await.unwrap();
            assert_eq!(resp.status(), 200);

            let ct = resp
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap();
            assert_eq!(ct, "application/json");

            let cors = resp
                .headers()
                .get("access-control-allow-origin")
                .unwrap()
                .to_str()
                .unwrap();
            assert_eq!(cors, "*");

            use http_body_util::BodyExt;
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            let body_str = String::from_utf8(body.to_vec()).unwrap();
            assert_eq!(body_str, r#"{"status":"ready"}"#);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn http_server_returns_404_for_missing_files() {
            let pm = pipeline::PipelineManager::new_for_test(true);
            let client_dir = "/nonexistent".to_string();
            let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            let bound_addr = listener.local_addr().unwrap();
            drop(listener);

            tokio::spawn(run_http_server(bound_addr, client_dir, pm));
            tokio::time::sleep(Duration::from_millis(100)).await;

            use hyper_util::rt::TokioIo;
            let stream = tokio::net::TcpStream::connect(bound_addr).await.unwrap();
            let io = TokioIo::new(stream);

            let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
            tokio::spawn(async move {
                let _ = conn.await;
            });

            let req = hyper::Request::builder()
                .uri("/some/nonexistent/path")
                .header("Host", "localhost")
                .body(http_body_util::Empty::<bytes::Bytes>::new())
                .unwrap();

            let resp = sender.send_request(req).await.unwrap();
            assert_eq!(resp.status(), 404);
        }
    }
}
