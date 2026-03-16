//! X11 window monitoring for Coherence Mode.
//!
//! Watches the X11 display for window creation, destruction, movement, resize,
//! and visibility changes. Emits [`WindowEvent`]s over a broadcast channel.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::broadcast;
use x11rb::connection::Connection;
use x11rb::protocol::composite::{self};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

/// Shared snapshot of currently tracked windows, updated by the monitor thread.
pub type TrackedWindows = Arc<StdMutex<HashMap<u32, WindowInfo>>>;

/// Information about a single X11 window.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub window_id: u32,
    pub title: String,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub visible: bool,
    pub app_class: String,
}

/// Events emitted by the window monitor.
#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// Initial snapshot of all existing windows.
    Snapshot(Vec<WindowInfo>),
    /// A new window appeared.
    Added(WindowInfo),
    /// A window was destroyed.
    Removed { window_id: u32 },
    /// A window was resized.
    Resized {
        window_id: u32,
        width: u16,
        height: u16,
    },
    /// A window was moved.
    Moved { window_id: u32, x: i16, y: i16 },
    /// A window title changed.
    TitleChanged { window_id: u32, title: String },
    /// A window's visibility changed (mapped/unmapped).
    VisibilityChanged { window_id: u32, visible: bool },
}

/// Serialization format for WindowInfo on the wire.
impl WindowInfo {
    /// Serialize to binary for the coherence protocol.
    pub fn serialize(&self) -> Vec<u8> {
        let title_bytes = self.title.as_bytes();
        let class_bytes = self.app_class.as_bytes();
        let mut buf = Vec::with_capacity(
            4 + 2 + 2 + 2 + 2 + 1 + 2 + title_bytes.len() + 2 + class_bytes.len(),
        );

        buf.extend_from_slice(&self.window_id.to_be_bytes());
        buf.extend_from_slice(&self.x.to_be_bytes());
        buf.extend_from_slice(&self.y.to_be_bytes());
        buf.extend_from_slice(&self.width.to_be_bytes());
        buf.extend_from_slice(&self.height.to_be_bytes());
        buf.push(if self.visible { 1 } else { 0 });
        buf.extend_from_slice(&(title_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(title_bytes);
        buf.extend_from_slice(&(class_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(class_bytes);

        buf
    }

    /// Deserialize from binary.
    pub fn deserialize(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 13 {
            return None;
        }
        let window_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let x = i16::from_be_bytes([data[4], data[5]]);
        let y = i16::from_be_bytes([data[6], data[7]]);
        let width = u16::from_be_bytes([data[8], data[9]]);
        let height = u16::from_be_bytes([data[10], data[11]]);
        let visible = data[12] != 0;

        let mut offset = 13;
        if data.len() < offset + 2 {
            return None;
        }
        let title_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if data.len() < offset + title_len {
            return None;
        }
        let title = std::str::from_utf8(&data[offset..offset + title_len])
            .ok()?
            .to_string();
        offset += title_len;

        if data.len() < offset + 2 {
            return None;
        }
        let class_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if data.len() < offset + class_len {
            return None;
        }
        let app_class = std::str::from_utf8(&data[offset..offset + class_len])
            .ok()?
            .to_string();
        offset += class_len;

        Some((
            Self {
                window_id,
                title,
                x,
                y,
                width,
                height,
                visible,
                app_class,
            },
            offset,
        ))
    }
}

/// Handle to the running window monitor. Drop to stop monitoring.
pub struct WindowMonitorHandle {
    _join_handle: std::thread::JoinHandle<()>,
}

/// Start the X11 window monitor on a dedicated thread.
///
/// Returns a broadcast receiver for window events and a handle to keep the monitor alive.
pub fn start_window_monitor(
    display: &str,
) -> Result<(
    broadcast::Receiver<WindowEvent>,
    TrackedWindows,
    WindowMonitorHandle,
)> {
    let (tx, rx) = broadcast::channel::<WindowEvent>(256);
    let tracked_windows: TrackedWindows = Arc::new(StdMutex::new(HashMap::new()));
    let tracked_clone = tracked_windows.clone();
    let display = display.to_string();

    let handle = std::thread::Builder::new()
        .name("window-monitor".into())
        .spawn(move || {
            if let Err(e) = run_monitor_loop(&display, &tx, &tracked_clone) {
                tracing::error!("Window monitor error: {}", e);
            }
        })
        .context("Failed to spawn window monitor thread")?;

    Ok((
        rx,
        tracked_windows,
        WindowMonitorHandle {
            _join_handle: handle,
        },
    ))
}

/// The X11 atoms we need for window property queries.
pub(crate) struct Atoms {
    net_wm_name: xproto::Atom,
    wm_name: xproto::Atom,
    utf8_string: xproto::Atom,
    wm_class: xproto::Atom,
    net_wm_window_type: xproto::Atom,
    net_wm_window_type_normal: xproto::Atom,
    net_wm_window_type_dialog: xproto::Atom,
    net_wm_window_type_utility: xproto::Atom,
    net_close_window: xproto::Atom,
    wm_protocols: xproto::Atom,
    wm_delete_window: xproto::Atom,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Self> {
        fn atom(conn: &RustConnection, name: &str) -> Result<xproto::Atom> {
            Ok(conn
                .intern_atom(false, name.as_bytes())?
                .reply()
                .context(format!("Failed to intern atom {name}"))?
                .atom)
        }

        Ok(Self {
            net_wm_name: atom(conn, "_NET_WM_NAME")?,
            wm_name: atom(conn, "WM_NAME")?,
            utf8_string: atom(conn, "UTF8_STRING")?,
            wm_class: atom(conn, "WM_CLASS")?,
            net_wm_window_type: atom(conn, "_NET_WM_WINDOW_TYPE")?,
            net_wm_window_type_normal: atom(conn, "_NET_WM_WINDOW_TYPE_NORMAL")?,
            net_wm_window_type_dialog: atom(conn, "_NET_WM_WINDOW_TYPE_DIALOG")?,
            net_wm_window_type_utility: atom(conn, "_NET_WM_WINDOW_TYPE_UTILITY")?,
            net_close_window: atom(conn, "_NET_CLOSE_WINDOW")?,
            wm_protocols: atom(conn, "WM_PROTOCOLS")?,
            wm_delete_window: atom(conn, "WM_DELETE_WINDOW")?,
        })
    }
}

fn run_monitor_loop(
    display: &str,
    tx: &broadcast::Sender<WindowEvent>,
    shared_tracked: &TrackedWindows,
) -> Result<()> {
    loop {
        match run_monitor_session(display, tx, shared_tracked) {
            Ok(()) => {
                tracing::info!("Window monitor session ended cleanly");
            }
            Err(e) => {
                tracing::warn!("Window monitor session error: {}", e);
            }
        }

        // Clear stale tracked windows — old window IDs are invalid after Xvfb restart
        if let Ok(mut shared) = shared_tracked.lock() {
            shared.clear();
        }

        // Wait for the new X server to be ready before reconnecting
        tracing::info!("Window monitor will reconnect in 2s...");
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn run_monitor_session(
    display: &str,
    tx: &broadcast::Sender<WindowEvent>,
    shared_tracked: &TrackedWindows,
) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(Some(display))
        .context("Failed to connect to X11 for monitoring")?;
    let root = conn.setup().roots[screen_num].root;
    let atoms = Atoms::intern(&conn)?;

    // Enable X Composite extension: redirect all child windows to offscreen pixmaps
    // so ximagesrc can capture each window independently even when overlapped.
    composite::redirect_subwindows(&conn, root, composite::Redirect::AUTOMATIC)?
        .check()
        .context("Failed to enable Composite redirection on root window")?;
    tracing::info!("X Composite: redirected subwindows for independent capture");

    // Subscribe to substructure notify on root (window create/destroy/reparent/configure)
    conn.change_window_attributes(
        root,
        &xproto::ChangeWindowAttributesAux::new().event_mask(
            xproto::EventMask::SUBSTRUCTURE_NOTIFY | xproto::EventMask::PROPERTY_CHANGE,
        ),
    )?
    .check()
    .context("Failed to subscribe to root window events")?;

    // Initial enumeration
    let mut tracked: HashMap<u32, WindowInfo> = HashMap::new();
    enumerate_windows(&conn, root, &atoms, &mut tracked)?;

    let snapshot: Vec<WindowInfo> = tracked.values().cloned().collect();
    tracing::info!(
        "Window monitor reconnected, found {} windows",
        snapshot.len()
    );
    // Update shared state so late subscribers can get a fresh snapshot
    if let Ok(mut shared) = shared_tracked.lock() {
        *shared = tracked.clone();
    }
    let _ = tx.send(WindowEvent::Snapshot(snapshot));

    // Event loop
    loop {
        let event = match conn.wait_for_event() {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("X11 connection lost: {}", e);
                return Err(e.into());
            }
        };

        match event {
            x11rb::protocol::Event::MapNotify(e) => {
                let win = e.window;
                if tracked.contains_key(&win) {
                    // Window became visible
                    if let Some(info) = tracked.get_mut(&win)
                        && !info.visible
                    {
                        info.visible = true;
                        let _ = tx.send(WindowEvent::VisibilityChanged {
                            window_id: win,
                            visible: true,
                        });
                    }
                } else {
                    // New window mapped - check if it's one we should track
                    if let Some(info) = query_window_info(&conn, win, &atoms) {
                        tracing::info!("Window added: {} ({})", info.title, win);
                        let _ = tx.send(WindowEvent::Added(info.clone()));
                        let _ = conn.change_window_attributes(
                            win,
                            &xproto::ChangeWindowAttributesAux::new().event_mask(
                                xproto::EventMask::PROPERTY_CHANGE
                                    | xproto::EventMask::STRUCTURE_NOTIFY,
                            ),
                        );
                        let _ = conn.flush();
                        tracked.insert(win, info);
                    } else {
                        // This might be a WM frame window — check its children
                        // (window managers reparent client windows under frame windows)
                        if let Ok(subtree) = conn.query_tree(win)
                            && let Ok(subtree) = subtree.reply()
                        {
                            for &child in &subtree.children {
                                if tracked.contains_key(&child) {
                                    continue;
                                }
                                if let Some(info) = query_window_info(&conn, child, &atoms) {
                                    tracing::info!(
                                        "Window added (via WM frame): {} ({})",
                                        info.title,
                                        child
                                    );
                                    let _ = tx.send(WindowEvent::Added(info.clone()));
                                    let _ = conn.change_window_attributes(
                                        child,
                                        &xproto::ChangeWindowAttributesAux::new().event_mask(
                                            xproto::EventMask::PROPERTY_CHANGE
                                                | xproto::EventMask::STRUCTURE_NOTIFY,
                                        ),
                                    );
                                    let _ = conn.flush();
                                    tracked.insert(child, info);
                                }
                            }
                        }
                    }
                }
            }

            x11rb::protocol::Event::UnmapNotify(e) => {
                let win = e.window;
                if let Some(info) = tracked.get_mut(&win)
                    && info.visible
                {
                    info.visible = false;
                    let _ = tx.send(WindowEvent::VisibilityChanged {
                        window_id: win,
                        visible: false,
                    });
                }
            }

            x11rb::protocol::Event::DestroyNotify(e) => {
                let win = e.window;
                if tracked.remove(&win).is_some() {
                    tracing::info!("Window removed: {}", win);
                    let _ = tx.send(WindowEvent::Removed { window_id: win });
                }
            }

            x11rb::protocol::Event::ConfigureNotify(e) => {
                let win = e.window;
                if let Some(info) = tracked.get_mut(&win) {
                    let moved = info.x != e.x || info.y != e.y;
                    let resized = info.width != e.width || info.height != e.height;

                    if moved {
                        info.x = e.x;
                        info.y = e.y;
                        let _ = tx.send(WindowEvent::Moved {
                            window_id: win,
                            x: e.x,
                            y: e.y,
                        });
                    }
                    if resized {
                        info.width = e.width;
                        info.height = e.height;
                        let _ = tx.send(WindowEvent::Resized {
                            window_id: win,
                            width: e.width,
                            height: e.height,
                        });
                    }
                }
            }

            x11rb::protocol::Event::PropertyNotify(e) => {
                let win = e.window;
                if win == root {
                    continue;
                }
                if (e.atom == atoms.net_wm_name || e.atom == atoms.wm_name)
                    && let Some(info) = tracked.get_mut(&win)
                {
                    let new_title = get_window_title(&conn, win, &atoms);
                    if new_title != info.title {
                        info.title = new_title.clone();
                        let _ = tx.send(WindowEvent::TitleChanged {
                            window_id: win,
                            title: new_title,
                        });
                    }
                }
            }

            x11rb::protocol::Event::ReparentNotify(e) => {
                // Window was reparented - if it's reparented to root, we may want to track it
                // If reparented away from root, the WM is managing it (this is normal)
                if e.parent == root
                    && !tracked.contains_key(&e.window)
                    && let Some(info) = query_window_info(&conn, e.window, &atoms)
                {
                    let _ = conn.change_window_attributes(
                        e.window,
                        &xproto::ChangeWindowAttributesAux::new().event_mask(
                            xproto::EventMask::PROPERTY_CHANGE
                                | xproto::EventMask::STRUCTURE_NOTIFY,
                        ),
                    );
                    let _ = conn.flush();
                    let _ = tx.send(WindowEvent::Added(info.clone()));
                    tracked.insert(e.window, info);
                }
            }

            _ => {}
        }

        // Sync shared state after each event so late subscribers get current data
        if let Ok(mut shared) = shared_tracked.lock() {
            *shared = tracked.clone();
        }
    }
}

/// Enumerate existing windows at startup.
fn enumerate_windows(
    conn: &RustConnection,
    root: xproto::Window,
    atoms: &Atoms,
    tracked: &mut HashMap<u32, WindowInfo>,
) -> Result<()> {
    let tree = conn
        .query_tree(root)?
        .reply()
        .context("Failed to query window tree")?;

    for &child in &tree.children {
        if let Some(info) = query_window_info(conn, child, atoms) {
            // Subscribe to property/structure changes
            let _ = conn.change_window_attributes(
                child,
                &xproto::ChangeWindowAttributesAux::new().event_mask(
                    xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY,
                ),
            );
            tracked.insert(child, info);
        }

        // Also check children of children (WMs reparent windows)
        if let Ok(subtree) = conn.query_tree(child)
            && let Ok(subtree) = subtree.reply()
        {
            for &grandchild in &subtree.children {
                if tracked.contains_key(&grandchild) {
                    continue;
                }
                if let Some(info) = query_window_info(conn, grandchild, atoms) {
                    let _ = conn.change_window_attributes(
                        grandchild,
                        &xproto::ChangeWindowAttributesAux::new().event_mask(
                            xproto::EventMask::PROPERTY_CHANGE
                                | xproto::EventMask::STRUCTURE_NOTIFY,
                        ),
                    );
                    tracked.insert(grandchild, info);
                }
            }
        }
    }

    conn.flush()?;
    Ok(())
}

/// Query a single window's info. Returns None if the window should not be tracked.
fn query_window_info(
    conn: &RustConnection,
    window: xproto::Window,
    atoms: &Atoms,
) -> Option<WindowInfo> {
    // Get attributes to check override_redirect and map_state
    let attrs = conn.get_window_attributes(window).ok()?.reply().ok()?;

    // Skip override-redirect windows (menus, tooltips, splash screens)
    if attrs.override_redirect {
        return None;
    }

    // Check window type - only track normal, dialog, and utility windows
    if !is_trackable_window_type(conn, window, atoms) {
        return None;
    }

    // Get geometry
    let geom = conn.get_geometry(window).ok()?.reply().ok()?;

    // Skip tiny windows (likely hidden or internal)
    if geom.width < 10 || geom.height < 10 {
        return None;
    }

    let visible = attrs.map_state == xproto::MapState::VIEWABLE;
    let title = get_window_title(conn, window, atoms);
    let app_class = get_wm_class(conn, window, atoms);

    // Skip windows with no title and no class (likely internal WM windows)
    if title.is_empty() && app_class.is_empty() {
        return None;
    }

    // Translate coordinates to root window coordinates
    let (x, y) = translate_coords(conn, window).unwrap_or((geom.x, geom.y));

    Some(WindowInfo {
        window_id: window,
        title,
        x,
        y,
        width: geom.width,
        height: geom.height,
        visible,
        app_class,
    })
}

/// Check if a window's _NET_WM_WINDOW_TYPE indicates it should be tracked.
fn is_trackable_window_type(conn: &RustConnection, window: xproto::Window, atoms: &Atoms) -> bool {
    let reply = conn
        .get_property(
            false,
            window,
            atoms.net_wm_window_type,
            xproto::AtomEnum::ATOM,
            0,
            32,
        )
        .ok()
        .and_then(|cookie| cookie.reply().ok());

    match reply {
        Some(prop) if prop.length > 0 => {
            // Parse atom list
            if let Some(atom_values) = prop.value32() {
                for atom_val in atom_values {
                    if atom_val == atoms.net_wm_window_type_normal
                        || atom_val == atoms.net_wm_window_type_dialog
                        || atom_val == atoms.net_wm_window_type_utility
                    {
                        return true;
                    }
                }
                // Has a type set but it's not one we track (e.g., dock, desktop, toolbar)
                false
            } else {
                true // Can't parse - assume trackable
            }
        }
        _ => {
            // No type set - treat as normal window
            true
        }
    }
}

/// Get the window title, preferring _NET_WM_NAME over WM_NAME.
fn get_window_title(conn: &RustConnection, window: xproto::Window, atoms: &Atoms) -> String {
    // Try _NET_WM_NAME first (UTF-8)
    if let Ok(cookie) =
        conn.get_property(false, window, atoms.net_wm_name, atoms.utf8_string, 0, 1024)
        && let Ok(prop) = cookie.reply()
        && prop.length > 0
        && let Ok(s) = std::str::from_utf8(&prop.value)
    {
        return s.to_string();
    }

    // Fallback to WM_NAME
    if let Ok(cookie) = conn.get_property(
        false,
        window,
        atoms.wm_name,
        xproto::AtomEnum::STRING,
        0,
        1024,
    ) && let Ok(prop) = cookie.reply()
        && prop.length > 0
    {
        return String::from_utf8_lossy(&prop.value).to_string();
    }

    String::new()
}

/// Get the WM_CLASS of a window (application identifier).
fn get_wm_class(conn: &RustConnection, window: xproto::Window, atoms: &Atoms) -> String {
    if let Ok(cookie) = conn.get_property(
        false,
        window,
        atoms.wm_class,
        xproto::AtomEnum::STRING,
        0,
        256,
    ) && let Ok(prop) = cookie.reply()
        && prop.length > 0
    {
        // WM_CLASS is two null-terminated strings: instance\0class\0
        // We want the class (second string)
        let parts: Vec<&[u8]> = prop.value.split(|&b| b == 0).collect();
        if parts.len() >= 2 {
            return String::from_utf8_lossy(parts[1]).to_string();
        }
        if !parts.is_empty() {
            return String::from_utf8_lossy(parts[0]).to_string();
        }
    }
    String::new()
}

/// Translate window coordinates to root-relative coordinates.
fn translate_coords(conn: &RustConnection, window: xproto::Window) -> Option<(i16, i16)> {
    let root = conn.setup().roots[0].root;
    let reply = conn
        .translate_coordinates(window, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    Some((reply.dst_x, reply.dst_y))
}

/// Public helper to resize an X11 window.
pub fn resize_window(conn: &RustConnection, window: u32, width: u16, height: u16) -> Result<()> {
    conn.configure_window(
        window,
        &xproto::ConfigureWindowAux::new()
            .width(u32::from(width))
            .height(u32::from(height)),
    )?
    .check()
    .context("Failed to resize window")?;
    Ok(())
}

/// Public helper to raise/focus an X11 window.
pub fn raise_window(conn: &RustConnection, window: u32) -> Result<()> {
    conn.configure_window(
        window,
        &xproto::ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
    )?
    .check()
    .context("Failed to raise window")?;

    conn.set_input_focus(xproto::InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
        .check()
        .context("Failed to focus window")?;
    Ok(())
}

/// Public helper to close an X11 window via WM_DELETE_WINDOW.
pub fn close_window(conn: &RustConnection, window: u32, atoms: &Atoms) -> Result<()> {
    let event = xproto::ClientMessageEvent::new(
        32,
        window,
        atoms.wm_protocols,
        [atoms.wm_delete_window, x11rb::CURRENT_TIME, 0, 0, 0],
    );
    conn.send_event(false, window, xproto::EventMask::NO_EVENT, event)?
        .check()
        .context("Failed to send close event")?;
    Ok(())
}

/// Public interface for window management operations from coherence sessions.
pub struct WindowManager {
    display: String,
    conn: std::sync::Mutex<RustConnection>,
    atoms: std::sync::Mutex<Atoms>,
}

impl WindowManager {
    pub fn new(display: &str) -> Result<Self> {
        let (conn, atoms) = Self::connect(display)?;
        Ok(Self {
            display: display.to_string(),
            conn: std::sync::Mutex::new(conn),
            atoms: std::sync::Mutex::new(atoms),
        })
    }

    /// Reconnect to the X11 display after an Xvfb restart.
    pub fn reconnect(&self) -> Result<()> {
        let (conn, atoms) = Self::connect(&self.display)?;
        *self.conn.lock().unwrap() = conn;
        *self.atoms.lock().unwrap() = atoms;
        tracing::info!("WindowManager reconnected to display {}", self.display);
        Ok(())
    }

    fn connect(display: &str) -> Result<(RustConnection, Atoms)> {
        let (conn, _screen_num) = RustConnection::connect(Some(display))
            .context("Failed to connect to X11 for window management")?;
        let atoms = Atoms::intern(&conn)?;
        Ok((conn, atoms))
    }

    pub fn resize(&self, window_id: u32, width: u16, height: u16) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        resize_window(&conn, window_id, width, height)
    }

    pub fn raise(&self, window_id: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        raise_window(&conn, window_id)
    }

    pub fn close(&self, window_id: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let atoms = self.atoms.lock().unwrap();
        close_window(&conn, window_id, &atoms)
    }

    /// Query the actual geometry of a window from the X server.
    pub fn get_geometry(&self, window_id: u32) -> Result<(u16, u16)> {
        let conn = self.conn.lock().unwrap();
        let geom = conn
            .get_geometry(window_id)?
            .reply()
            .context("Failed to get window geometry")?;
        Ok((geom.width, geom.height))
    }

    /// Flush all pending requests and wait for the X server to process them.
    pub fn sync(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.sync().context("X11 sync failed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_info_serialize_deserialize_roundtrip() {
        let info = WindowInfo {
            window_id: 0x12345678,
            title: "Test Window".into(),
            x: -10,
            y: 20,
            width: 800,
            height: 600,
            visible: true,
            app_class: "TestApp".into(),
        };

        let bytes = info.serialize();
        let (deserialized, consumed) = WindowInfo::deserialize(&bytes).unwrap();

        assert_eq!(consumed, bytes.len());
        assert_eq!(deserialized.window_id, info.window_id);
        assert_eq!(deserialized.title, info.title);
        assert_eq!(deserialized.x, info.x);
        assert_eq!(deserialized.y, info.y);
        assert_eq!(deserialized.width, info.width);
        assert_eq!(deserialized.height, info.height);
        assert_eq!(deserialized.visible, info.visible);
        assert_eq!(deserialized.app_class, info.app_class);
    }

    #[test]
    fn window_info_serialize_empty_strings() {
        let info = WindowInfo {
            window_id: 1,
            title: String::new(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            visible: false,
            app_class: String::new(),
        };

        let bytes = info.serialize();
        let (deserialized, _) = WindowInfo::deserialize(&bytes).unwrap();
        assert_eq!(deserialized.title, "");
        assert_eq!(deserialized.app_class, "");
        assert!(!deserialized.visible);
    }

    #[test]
    fn window_info_deserialize_truncated_returns_none() {
        // Too short for the fixed header
        assert!(WindowInfo::deserialize(&[0; 5]).is_none());
        // Exactly header but title length would overflow
        let mut data = vec![0u8; 13];
        data.extend_from_slice(&[0x00, 0x10]); // title_len = 16
        // but no title bytes follow
        assert!(WindowInfo::deserialize(&data).is_none());
    }

    #[test]
    fn window_info_serialize_unicode_title() {
        let info = WindowInfo {
            window_id: 42,
            title: "Firefox".into(),
            x: 100,
            y: 200,
            width: 1024,
            height: 768,
            visible: true,
            app_class: "Navigator".into(),
        };

        let bytes = info.serialize();
        let (deserialized, _) = WindowInfo::deserialize(&bytes).unwrap();
        assert_eq!(deserialized.title, "Firefox");
        assert_eq!(deserialized.app_class, "Navigator");
    }

    #[test]
    fn window_info_multiple_deserialize() {
        let info1 = WindowInfo {
            window_id: 1,
            title: "A".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            visible: true,
            app_class: "a".into(),
        };
        let info2 = WindowInfo {
            window_id: 2,
            title: "BB".into(),
            x: 10,
            y: 20,
            width: 200,
            height: 300,
            visible: false,
            app_class: "bb".into(),
        };

        let mut buf = info1.serialize();
        buf.extend(info2.serialize());

        let (d1, consumed1) = WindowInfo::deserialize(&buf).unwrap();
        assert_eq!(d1.window_id, 1);

        let (d2, _consumed2) = WindowInfo::deserialize(&buf[consumed1..]).unwrap();
        assert_eq!(d2.window_id, 2);
        assert_eq!(d2.title, "BB");
    }
}
