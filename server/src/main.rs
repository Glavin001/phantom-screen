#![allow(dead_code)]

mod auth;
mod coherence;
mod config;
mod control;
mod input;
mod pipeline;
mod window_monitor;
mod window_pipeline;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::{broadcast, watch};
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

    // Install a non-fatal Xlib error handler. GStreamer's ximagesrc uses Xlib
    // internally, and the default Xlib error handler calls exit(1) on any X
    // error (e.g., BadWindow when a window is destroyed between detection and
    // capture). Our handler logs the error instead of crashing the process.
    install_xlib_error_handler();

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

    // Create input handler (before pipeline so it can be shared with PipelineManager)
    let input_handler =
        Arc::new(InputHandler::new(&config.display).context("Failed to create input handler")?);

    // Launch post-start command (e.g. a demo app) if configured
    if let Some(ref cmd) = config.post_start_command {
        info!("Launching post-start command: {}", cmd);
        let child = Command::new("sh")
            .args(["-c", cmd])
            .spawn()
            .context("Failed to start post-start command")?;
        children.push(child);
    }

    // Start GStreamer pipeline (shares input_handler and post_start_command for resize coordination)
    let (_frame_rx, pipeline_manager) = pipeline::start_pipeline(
        &config,
        Some(input_handler.clone()),
        config.post_start_command.clone(),
    )
    .context("Failed to start pipeline")?;
    info!("GStreamer pipeline running");

    // Monitor the X display for external resolution changes (e.g. xrandr, apps)
    let pm_for_monitor = pipeline_manager.clone();
    let display_for_monitor = config.display.clone();
    pipeline::spawn_resolution_monitor(pm_for_monitor, display_for_monitor, Duration::from_secs(2));

    // Initialize coherence mode support (window monitor + pipeline manager)
    let coherence_state = {
        let (window_rx, tracked_windows, _monitor_handle) =
            window_monitor::start_window_monitor(&config.display)
                .context("Failed to start window monitor")?;
        // The monitor handle is moved into a leaked box so it lives for the process lifetime.
        // This is fine since we shut down via process exit.
        let _handle = Box::leak(Box::new(_monitor_handle));

        let (window_event_tx, _) = broadcast::channel::<window_monitor::WindowEvent>(256);

        // Forward window monitor events to the broadcast channel.
        // The monitor thread auto-reconnects after Xvfb restarts, so
        // this forwarder keeps running for the lifetime of the process.
        let composite_ready = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let tx_clone = window_event_tx.clone();
        let tracked_for_forwarder = tracked_windows.clone();
        let composite_ready_for_forwarder = composite_ready.clone();
        tokio::spawn(async move {
            let mut rx = window_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        // Snapshot means window monitor reconnected and Composite is re-enabled
                        if matches!(&event, window_monitor::WindowEvent::Snapshot(_)) {
                            composite_ready_for_forwarder
                                .store(true, std::sync::atomic::Ordering::Release);
                        }
                        let _ = tx_clone.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Window event forwarder lagged by {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Window monitor channel closed (process shutting down)");
                        // Clear tracked windows so clients don't use stale IDs
                        if let Ok(mut shared) = tracked_for_forwarder.lock() {
                            shared.clear();
                        }
                        break;
                    }
                }
            }
        });

        let pipeline_manager = Arc::new(tokio::sync::Mutex::new(
            window_pipeline::WindowPipelineManager::new(&config),
        ));
        let window_manager = Arc::new(
            window_monitor::WindowManager::new(&config.display)
                .context("Failed to create window manager")?,
        );

        Arc::new(coherence::CoherenceState {
            window_events: window_event_tx,
            tracked_windows,
            pipeline_manager,
            window_manager,
            composite_ready,
        })
    };
    info!("Coherence mode support initialized");

    // Register pre-resize hook: stop all per-window pipelines before Xvfb is killed.
    // This prevents GStreamer's ximagesrc from holding open Xlib connections that
    // would trigger fatal XIO errors when the X server goes away.
    {
        let wpm = coherence_state.pipeline_manager.clone();
        let composite_ready = coherence_state.composite_ready.clone();
        let window_events_tx = coherence_state.window_events.clone();
        pipeline_manager.add_pre_resize_hook(Box::new(move || {
            // Mark Composite as not ready — block new per-window pipeline starts
            composite_ready.store(false, std::sync::atomic::Ordering::Release);

            // Broadcast empty snapshot so clients know all window IDs are invalidated
            let _ = window_events_tx.send(window_monitor::WindowEvent::Snapshot(vec![]));

            if let Ok(mut mgr) = wpm.try_lock() {
                let count = mgr.active_count();
                if count > 0 {
                    tracing::info!("Pre-resize: stopping {} per-window pipeline(s)", count);
                    mgr.stop_all();
                }
            } else {
                tracing::warn!("Pre-resize: could not lock window pipeline manager");
            }
        }));
    }

    // Register post-resize hook: reconnect WindowManager to the new X server.
    {
        let wm = coherence_state.window_manager.clone();
        pipeline_manager.add_post_resize_hook(Box::new(move || {
            if let Err(e) = wm.reconnect() {
                tracing::error!("Post-resize: failed to reconnect WindowManager: {}", e);
            }
        }));
    }

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
    let launch_apps_json = {
        let apps = config.launch_app_list();
        format!(
            "[{}]",
            apps.iter()
                .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    tokio::spawn(async move {
        if let Err(e) = run_http_server(http_addr, client_dir, pm_for_http, launch_apps_json).await
        {
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
                let cs = coherence_state.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_session(session, pm, ih, cs).await {
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

/// Install non-fatal Xlib error handlers (both protocol errors and IO errors).
///
/// GStreamer elements like `ximagesrc` use Xlib (C library) internally. Xlib has
/// TWO error handlers that can call `exit()`:
///
/// 1. **XSetErrorHandler** — protocol errors (BadWindow, BadMatch, etc.)
///    Default handler prints error and calls `exit(1)`.
///
/// 2. **XSetIOErrorHandler** — fatal IO errors (connection lost/broken pipe)
///    Default handler prints "XIO: fatal IO error" and calls `_exit(1)`.
///    This fires when Xvfb is killed for resize while GStreamer still has
///    an open Xlib connection.
///
/// We replace both with handlers that log warnings instead of crashing.
fn install_xlib_error_handler() {
    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct XErrorEvent {
        _type: i32,
        _display: *mut std::ffi::c_void,
        resourceid: u64,
        _serial: u64,
        error_code: u8,
        request_code: u8,
        minor_code: u8,
    }

    type XErrorHandler =
        Option<unsafe extern "C" fn(*mut std::ffi::c_void, *mut XErrorEvent) -> i32>;
    type XIOErrorHandler = Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>;

    #[link(name = "X11")]
    unsafe extern "C" {
        fn XSetErrorHandler(handler: XErrorHandler) -> XErrorHandler;
        fn XSetIOErrorHandler(handler: XIOErrorHandler) -> XIOErrorHandler;
    }

    unsafe extern "C" {
        fn pthread_exit(retval: *mut std::ffi::c_void) -> !;
    }

    unsafe extern "C" fn non_fatal_error_handler(
        _display: *mut std::ffi::c_void,
        event: *mut XErrorEvent,
    ) -> i32 {
        let event = unsafe { &*event };
        tracing::warn!(
            error_code = event.error_code,
            request_code = event.request_code,
            resource_id = event.resourceid,
            "X11 error (non-fatal): error={} request={} resource=0x{:x}",
            event.error_code,
            event.request_code,
            event.resourceid,
        );
        0
    }

    unsafe extern "C" fn non_fatal_io_error_handler(_display: *mut std::ffi::c_void) -> i32 {
        // This fires when the X connection breaks (e.g., Xvfb killed for resize).
        // We MUST NOT return — Xlib calls _exit(1) after this handler returns,
        // regardless of the return value. This is by Xlib spec and cannot be
        // overridden via XSetIOErrorHandler alone.
        //
        // Instead, we terminate just this thread via pthread_exit(). The thread
        // hitting the IO error is a GStreamer streaming thread whose X connection
        // is already broken. Killing it prevents process-wide _exit(1) while
        // letting the rest of the server continue. The pipeline will detect the
        // thread loss and be cleaned up normally.
        tracing::warn!("X11 IO error: display connection lost, terminating thread");
        unsafe {
            pthread_exit(std::ptr::null_mut());
        }
    }

    unsafe {
        XSetErrorHandler(Some(non_fatal_error_handler));
        XSetIOErrorHandler(Some(non_fatal_io_error_handler));
    }
    info!("Installed non-fatal Xlib error and IO error handlers");
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

/// Core video sender loop: subscribes to pipeline broadcast channels via the
/// watch channel, and calls `send_frame` for each frame. Automatically
/// re-subscribes when the pipeline restarts (new broadcast sender published).
///
/// Extracted from `handle_session` so it can be unit-tested without WebTransport.
async fn video_sender_loop<F, Fut>(
    mut watch_rx: watch::Receiver<broadcast::Sender<EncodedFrame>>,
    send_frame: F,
) where
    F: Fn(EncodedFrame) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send,
{
    loop {
        // Subscribe to the current pipeline's broadcast channel
        let mut frame_rx = watch_rx.borrow_and_update().subscribe();

        // Stream frames until the pipeline stops or restarts
        let restart = loop {
            tokio::select! {
                frame_result = frame_rx.recv() => {
                    match frame_result {
                        Ok(frame) => {
                            if let Err(e) = send_frame(frame).await {
                                warn!("Failed to send video frame: {}", e);
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Video receiver lagged by {} frames, skipping", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // Pipeline stopped; wait for watch notification
                            // rather than immediately re-subscribing (the new
                            // pipeline may not be published yet).
                            break false;
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
                    break true;
                }
            }
        };

        // If we broke out due to Closed (not a watch notification),
        // wait for the new pipeline to be published before re-subscribing.
        if !restart {
            match watch_rx.changed().await {
                Ok(()) => {
                    info!("Pipeline restarted, re-subscribing to video frames");
                }
                Err(_) => {
                    info!("Pipeline manager closed, stopping video sender");
                    return;
                }
            }
        }
    }
}

async fn handle_session(
    session: wtransport::Connection,
    pipeline_manager: Arc<PipelineManager>,
    input_handler: Arc<InputHandler>,
    coherence_state: Arc<coherence::CoherenceState>,
) -> Result<()> {
    info!("Client connected");

    let session = Arc::new(session);

    // Spawn video sender that handles pipeline restarts via watch channel
    {
        let session_video = session.clone();
        let watch_rx = pipeline_manager.subscribe_watch();

        tokio::spawn(video_sender_loop(watch_rx, move |frame| {
            let session = session_video.clone();
            async move {
                send_video_frame(&session, &frame)
                    .await
                    .map_err(|e| e.to_string())
            }
        }));
    }

    // Per-session coherence state (lazily initialized on EnableCoherence)
    let coherence_session: Arc<tokio::sync::Mutex<Option<coherence::CoherenceSession>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // Handle bidirectional streams for input
    loop {
        match session.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let ih = input_handler.clone();
                let pm = pipeline_manager.clone();
                let cs = coherence_state.clone();
                let cs_session = coherence_session.clone();
                let session_for_input = session.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    loop {
                        match recv.read(&mut buf).await {
                            Ok(Some(n)) if n > 0 => {
                                process_input_data_with_coherence(
                                    &buf[..n],
                                    &ih,
                                    &pm,
                                    &cs,
                                    &cs_session,
                                    &session_for_input,
                                )
                                .await;
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
                // Clean up coherence session
                if let Some(mut cs) = coherence_session.lock().await.take() {
                    cs.cleanup();
                }
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

async fn process_input_data_with_coherence(
    data: &[u8],
    input_handler: &InputHandler,
    pipeline_manager: &Arc<PipelineManager>,
    coherence_state: &Arc<coherence::CoherenceState>,
    coherence_session: &Arc<tokio::sync::Mutex<Option<coherence::CoherenceSession>>>,
    session: &Arc<wtransport::Connection>,
) {
    let mut offset = 0;
    while offset < data.len() {
        let remaining = &data[offset..];
        let event_len = estimate_event_length(remaining);
        if event_len == 0 || offset + event_len > data.len() {
            break;
        }

        if let Some(event) = parse_input_event(&remaining[..event_len]) {
            match &event {
                InputEvent::EnableCoherence => {
                    let mut cs_lock = coherence_session.lock().await;
                    if cs_lock.is_none() {
                        let cs = coherence::CoherenceSession::new(coherence_state);
                        *cs_lock = Some(cs);
                        info!("Coherence mode enabled for session");

                        // Send a fresh snapshot of current windows immediately
                        let snapshot = coherence_state.current_snapshot();
                        let snapshot_data = coherence::serialize_window_event(&snapshot);
                        let session_for_snapshot = session.clone();
                        tokio::spawn(async move {
                            if let Ok(stream_future) = session_for_snapshot.open_uni().await
                                && let Ok(mut stream) = stream_future.await
                            {
                                let _ = stream.write_all(&snapshot_data).await;
                                let _ = stream.finish().await;
                            }
                        });

                        // Start forwarding future window events to the client
                        let events_rx = coherence_state.window_events.subscribe();
                        let session_clone = session.clone();
                        tokio::spawn(async move {
                            forward_window_events(events_rx, session_clone).await;
                        });
                    }
                }
                InputEvent::DisableCoherence => {
                    let mut cs_lock = coherence_session.lock().await;
                    if let Some(mut cs) = cs_lock.take() {
                        cs.cleanup();
                        info!("Coherence mode disabled for session");
                    }
                }
                InputEvent::SubscribeWindow { window_id } => {
                    if !coherence_state
                        .composite_ready
                        .load(std::sync::atomic::Ordering::Acquire)
                    {
                        warn!(
                            "Window {} subscribe rejected: Composite not ready (display resizing)",
                            window_id
                        );
                    } else {
                        let mut cs_lock = coherence_session.lock().await;
                        if let Some(ref mut cs) = *cs_lock {
                            let wid = *window_id;
                            match cs.subscribe_window(wid).await {
                                Ok(rx) => {
                                    spawn_window_sender(wid, rx, &session, cs);
                                }
                                Err(e) => {
                                    warn!("Failed to subscribe to window {}: {}", wid, e);
                                }
                            }
                        }
                    } // else composite_ready
                }
                InputEvent::UnsubscribeWindow { window_id } => {
                    let mut cs_lock = coherence_session.lock().await;
                    if let Some(ref mut cs) = *cs_lock {
                        cs.unsubscribe_window(*window_id).await;
                    }
                }
                InputEvent::ResizeWindow {
                    window_id,
                    width,
                    height,
                } => {
                    let mut cs_lock = coherence_session.lock().await;
                    if let Some(ref mut cs) = *cs_lock {
                        let wid = *window_id;
                        match cs.resize_window(wid, *width, *height).await {
                            Ok(Some(rx)) => {
                                // Pipeline was restarted at new size — spawn new sender
                                spawn_window_sender(wid, rx, &session, cs);
                            }
                            Ok(None) => {} // window wasn't being streamed
                            Err(e) => {
                                warn!("Failed to resize window {}: {}", wid, e);
                            }
                        }
                    }
                }
                InputEvent::FocusWindow { window_id } => {
                    let cs_lock = coherence_session.lock().await;
                    if let Some(ref cs) = *cs_lock
                        && let Err(e) = cs.focus_window(*window_id)
                    {
                        warn!("Failed to focus window {}: {}", window_id, e);
                    }
                }
                InputEvent::CloseWindow { window_id } => {
                    let cs_lock = coherence_session.lock().await;
                    if let Some(ref cs) = *cs_lock
                        && let Err(e) = cs.close_window(*window_id)
                    {
                        warn!("Failed to close window {}: {}", window_id, e);
                    }
                }
                InputEvent::LaunchApp { command } => {
                    info!("Launching app: {}", command);
                    let _ = std::process::Command::new("sh")
                        .args(["-c", command])
                        .spawn();
                }
                _ => {
                    // Regular input events - dispatch normally
                    if let Err(e) = dispatch_event(&event, input_handler, pipeline_manager) {
                        warn!("Failed to dispatch input event: {}", e);
                    }
                }
            }
        }

        offset += event_len;
    }
}

/// Forward window events from the monitor to the client via a unidirectional stream.
async fn forward_window_events(
    mut rx: broadcast::Receiver<window_monitor::WindowEvent>,
    session: Arc<wtransport::Connection>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let data = coherence::serialize_window_event(&event);
                match session.open_uni().await {
                    Ok(stream_future) => match stream_future.await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(&data).await {
                                warn!("Failed to send window event: {}", e);
                                break;
                            }
                            let _ = stream.finish().await;
                        }
                        Err(e) => {
                            warn!("Failed to open uni stream for window event: {}", e);
                            break;
                        }
                    },
                    Err(e) => {
                        warn!("Failed to open uni stream: {}", e);
                        break;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("Window event sender lagged by {} events", n);
            }
            Err(broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
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

/// Spawn a sender task that reads frames from a per-window broadcast receiver
/// and sends them to the client via WebTransport.
fn spawn_window_sender(
    wid: u32,
    mut rx: broadcast::Receiver<EncodedFrame>,
    session: &Arc<wtransport::Connection>,
    cs: &mut coherence::CoherenceSession,
) {
    let session_clone = session.clone();
    let handle = tokio::spawn(async move {
        info!("Window {} sender task STARTED", wid);
        let mut seen_keyframe = false;
        let mut frame_count: u64 = 0;
        let mut delta_skip_count: u64 = 0;
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    if !seen_keyframe {
                        if frame.is_keyframe {
                            seen_keyframe = true;
                            info!(
                                "Window {} sender: first keyframe ({} bytes), skipped {} deltas",
                                wid,
                                frame.data.len(),
                                delta_skip_count
                            );
                        } else {
                            delta_skip_count += 1;
                            if delta_skip_count <= 3 {
                                info!(
                                    "Window {} sender: skipping delta #{} ({} bytes)",
                                    wid,
                                    delta_skip_count,
                                    frame.data.len()
                                );
                            }
                            continue;
                        }
                    }
                    frame_count += 1;
                    if frame_count <= 5 {
                        info!(
                            "Window {} sender frame #{}: {} bytes, keyframe={}",
                            wid,
                            frame_count,
                            frame.data.len(),
                            frame.is_keyframe
                        );
                    }
                    if let Err(e) =
                        coherence::send_window_video_frame(&session_clone, wid, &frame).await
                    {
                        warn!("Window {} sender: SEND FAILED: {}", wid, e);
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        "Window {} sender: LAGGED by {}, resetting keyframe gate",
                        wid, n
                    );
                    seen_keyframe = false;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("Window {} sender: channel CLOSED", wid);
                    break;
                }
            }
        }
        info!(
            "Window {} sender task ENDED (sent {} frames)",
            wid, frame_count
        );
    });
    cs.track_sender(wid, handle);
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
        // Coherence events are handled in process_input_data_with_coherence
        InputEvent::EnableCoherence
        | InputEvent::DisableCoherence
        | InputEvent::SubscribeWindow { .. }
        | InputEvent::UnsubscribeWindow { .. }
        | InputEvent::ResizeWindow { .. }
        | InputEvent::FocusWindow { .. }
        | InputEvent::CloseWindow { .. }
        | InputEvent::LaunchApp { .. } => {}
    }
    Ok(())
}

/// Remove a stale X lock file if it exists. Returns true if a file was removed.
fn cleanup_stale_display(display_num: u32) -> bool {
    let lock_file = format!("/tmp/.X{display_num}-lock");
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
            &format!("{resolution}x24"),
            "-ac",
            "+bs", // Enable BackingStore so obscured windows retain their pixels
            "+extension",
            "RANDR", // Enable RandR for dynamic resolution changes
            "+extension",
            "Composite", // Enable Composite so per-window capture gets full contents
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
        "Failed to start window manager '{wm}'. Is it installed?"
    ))?;

    info!("Window manager started");
    Ok(child)
}

/// Simple HTTP server for serving static files and the /health endpoint
async fn run_http_server(
    addr: SocketAddr,
    client_dir: String,
    pipeline_manager: Arc<PipelineManager>,
    launch_apps_json: String,
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
        let apps_json = launch_apps_json.clone();

        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |req: Request<Incoming>| {
                let client_dir = client_dir.clone();
                let pm = pm.clone();
                let apps_json = apps_json.clone();
                async move {
                    let path = req.uri().path();

                    // Launch apps API endpoint
                    if path == "/api/launch-apps" {
                        return Ok::<_, Infallible>(
                            Response::builder()
                                .status(200)
                                .header("Content-Type", "application/json")
                                .header("Access-Control-Allow-Origin", "*")
                                .body(http_body_util::Full::new(bytes::Bytes::from(apps_json)))
                                .unwrap(),
                        );
                    }

                    // Health/readiness endpoint
                    if path == "/health" {
                        let status = if pm.is_running() { "ready" } else { "starting" };
                        let body = format!(r#"{{"status":"{status}"}}"#);
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

            tokio::spawn(run_http_server(
                bound_addr,
                client_dir,
                pm,
                r#"["xterm","firefox"]"#.into(),
            ));

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

            tokio::spawn(run_http_server(
                bound_addr,
                client_dir,
                pm,
                r#"["xterm","firefox"]"#.into(),
            ));
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

    /// Tests for the video_sender_loop: the async coordination between
    /// watch channels (pipeline restarts) and broadcast channels (frame delivery).
    ///
    /// These tests exercise the exact race conditions that caused streaming to
    /// break after resize — no GStreamer, no WebTransport, no X11 needed.
    mod video_sender {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::{broadcast, watch};

        fn test_frame(pts: u64) -> EncodedFrame {
            EncodedFrame {
                data: vec![0u8; 8],
                pts,
                is_keyframe: pts == 0,
            }
        }

        /// Basic test: frames sent on the initial pipeline are received.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn receives_frames_from_initial_pipeline() {
            let (tx1, _) = broadcast::channel::<EncodedFrame>(16);
            let (watch_tx, watch_rx) = watch::channel(tx1.clone());

            let received = Arc::new(AtomicUsize::new(0));
            let received_clone = received.clone();

            let handle = tokio::spawn(video_sender_loop(watch_rx, move |_frame| {
                received_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }));

            // Yield to let the spawned task subscribe to the broadcast channel
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Send some frames
            for i in 0..5 {
                tx1.send(test_frame(i)).unwrap();
            }

            // Give the loop time to process
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(received.load(Ordering::SeqCst), 5);

            // Drop the watch sender to shut down the loop
            drop(watch_tx);
            drop(tx1);
            let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
        }

        /// Simulates a pipeline restart where the watch channel notifies
        /// BEFORE the broadcast channel closes (the happy path).
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn resubscribes_when_watch_notifies_before_broadcast_closes() {
            let (tx1, _) = broadcast::channel::<EncodedFrame>(16);
            let (watch_tx, watch_rx) = watch::channel(tx1.clone());

            let received = Arc::new(AtomicUsize::new(0));
            let received_clone = received.clone();

            let handle = tokio::spawn(video_sender_loop(watch_rx, move |_frame| {
                received_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }));

            // Yield to let the spawned task subscribe
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Send frames on pipeline 1
            tx1.send(test_frame(0)).unwrap();
            tx1.send(test_frame(1)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            // Simulate pipeline restart: create new sender, publish via watch, THEN drop old
            let (tx2, _) = broadcast::channel::<EncodedFrame>(16);
            watch_tx.send(tx2.clone()).unwrap();
            drop(tx1); // old broadcast closes after watch already notified

            tokio::time::sleep(Duration::from_millis(20)).await;

            // Send frames on pipeline 2
            tx2.send(test_frame(2)).unwrap();
            tx2.send(test_frame(3)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            assert_eq!(received.load(Ordering::SeqCst), 4);

            drop(watch_tx);
            drop(tx2);
            let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
        }

        /// THE KEY BUG TEST: Simulates the race condition where the broadcast
        /// channel closes BEFORE the watch channel is updated (because resize
        /// runs synchronously: stop pipeline → sleep → restart → publish).
        ///
        /// Before the fix, the video sender would immediately re-subscribe to
        /// the stale broadcast channel and get Closed again, missing all frames.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn resubscribes_when_broadcast_closes_before_watch_updates() {
            let (tx1, _) = broadcast::channel::<EncodedFrame>(16);
            let (watch_tx, watch_rx) = watch::channel(tx1.clone());

            let received = Arc::new(AtomicUsize::new(0));
            let received_clone = received.clone();

            let handle = tokio::spawn(video_sender_loop(watch_rx, move |_frame| {
                received_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }));

            // Yield to let the spawned task subscribe
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Send frames on pipeline 1
            tx1.send(test_frame(0)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(received.load(Ordering::SeqCst), 1);

            // Simulate the race: drop old broadcast FIRST (pipeline.stop())
            drop(tx1);

            // Simulate the delay from Xvfb restart (the gap where the bug occurs)
            tokio::time::sleep(Duration::from_millis(50)).await;

            // NOW publish the new pipeline via watch (like resize() does after restart)
            let (tx2, _) = broadcast::channel::<EncodedFrame>(16);
            watch_tx.send(tx2.clone()).unwrap();

            tokio::time::sleep(Duration::from_millis(20)).await;

            // Send frames on pipeline 2 — these MUST be received
            tx2.send(test_frame(1)).unwrap();
            tx2.send(test_frame(2)).unwrap();
            tx2.send(test_frame(3)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            assert_eq!(
                received.load(Ordering::SeqCst),
                4,
                "Video sender must receive frames after pipeline restart even when \
                 broadcast closes before watch channel updates (the resize race condition)"
            );

            drop(watch_tx);
            drop(tx2);
            let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
        }

        /// Multiple sequential resizes (the scenario from the bug report:
        /// resize to 1350x1198, then to 1645x1198). Each resize drops the old
        /// broadcast before publishing the new one.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn survives_multiple_sequential_resizes() {
            let (tx1, _) = broadcast::channel::<EncodedFrame>(16);
            let (watch_tx, watch_rx) = watch::channel(tx1.clone());

            let received = Arc::new(AtomicUsize::new(0));
            let received_clone = received.clone();

            let handle = tokio::spawn(video_sender_loop(watch_rx, move |_frame| {
                received_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }));

            // Yield to let the spawned task subscribe
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Pipeline 1: send a frame
            tx1.send(test_frame(0)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            // Resize 1: drop old, delay, publish new (the race)
            drop(tx1);
            tokio::time::sleep(Duration::from_millis(30)).await;
            let (tx2, _) = broadcast::channel::<EncodedFrame>(16);
            watch_tx.send(tx2.clone()).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            tx2.send(test_frame(1)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            // Resize 2: drop old, delay, publish new (the race again)
            drop(tx2);
            tokio::time::sleep(Duration::from_millis(30)).await;
            let (tx3, _) = broadcast::channel::<EncodedFrame>(16);
            watch_tx.send(tx3.clone()).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            tx3.send(test_frame(2)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            // Resize 3: one more for good measure
            drop(tx3);
            tokio::time::sleep(Duration::from_millis(30)).await;
            let (tx4, _) = broadcast::channel::<EncodedFrame>(16);
            watch_tx.send(tx4.clone()).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            tx4.send(test_frame(3)).unwrap();
            tx4.send(test_frame(4)).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;

            assert_eq!(
                received.load(Ordering::SeqCst),
                5,
                "Video sender must survive multiple sequential resizes"
            );

            drop(watch_tx);
            drop(tx4);
            let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
        }

        /// When the PipelineManager is dropped (shutdown), the video sender
        /// should exit cleanly rather than hang or panic.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn exits_when_pipeline_manager_dropped() {
            let (tx1, _) = broadcast::channel::<EncodedFrame>(16);
            let (watch_tx, watch_rx) = watch::channel(tx1.clone());

            let handle = tokio::spawn(video_sender_loop(watch_rx, |_frame| async { Ok(()) }));

            // Drop everything — the loop should exit
            drop(tx1);
            drop(watch_tx);

            let result = tokio::time::timeout(Duration::from_millis(200), handle).await;
            assert!(
                result.is_ok(),
                "Video sender must exit promptly when pipeline manager is dropped"
            );
        }

        /// When send_frame returns an error, the loop should exit (connection lost).
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn exits_on_send_error() {
            let (tx1, _) = broadcast::channel::<EncodedFrame>(16);
            let (_watch_tx, watch_rx) = watch::channel(tx1.clone());

            let handle = tokio::spawn(video_sender_loop(watch_rx, |_frame| async {
                Err("connection lost".to_string())
            }));

            // Yield to let the spawned task subscribe
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;

            tx1.send(test_frame(0)).unwrap();

            let result = tokio::time::timeout(Duration::from_millis(200), handle).await;
            assert!(
                result.is_ok(),
                "Video sender must exit when send_frame fails"
            );
        }

        /// When broadcast channel lags (too many frames buffered), the sender
        /// should skip and continue rather than crash.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn handles_broadcast_lag_without_crashing() {
            // Small buffer to force lag
            let (tx1, _) = broadcast::channel::<EncodedFrame>(2);
            let (_watch_tx, watch_rx) = watch::channel(tx1.clone());

            let received = Arc::new(AtomicUsize::new(0));
            let received_clone = received.clone();

            let _handle = tokio::spawn(video_sender_loop(watch_rx, move |_frame| {
                received_clone.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }));

            // Yield to let the spawned task subscribe
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Flood the channel to cause lag
            for i in 0..10 {
                let _ = tx1.send(test_frame(i));
            }

            tokio::time::sleep(Duration::from_millis(50)).await;

            // Should have received at least some frames (not crashed)
            assert!(
                received.load(Ordering::SeqCst) > 0,
                "Video sender must handle lag gracefully"
            );
        }
    }
}
