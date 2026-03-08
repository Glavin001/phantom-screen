use anyhow::{Context, Result};
use std::collections::HashMap;
use x11rb::connection::Connection;
use x11rb::protocol::xproto;
use x11rb::protocol::xtest::ConnectionExt as XTestExt;

/// Manages X11 input injection via XTest extension
pub struct InputHandler {
    conn: x11rb::rust_connection::RustConnection,
    screen_num: usize,
    keycode_map: HashMap<String, u8>,
}

impl InputHandler {
    pub fn new(disp: &str) -> Result<Self> {
        // Set DISPLAY env var for x11rb
        // SAFETY: called before spawning threads, only modifies DISPLAY
        unsafe { std::env::set_var("DISPLAY", disp) };
        let (conn, screen_num) =
            x11rb::rust_connection::RustConnection::connect(Some(disp))
                .context("Failed to connect to X11 display")?;

        // Verify XTest extension is available
        conn.xtest_get_version(2, 1)
            .context("XTest extension not available")?
            .reply()
            .context("XTest version query failed")?;

        let keycode_map = build_keycode_map(&conn, screen_num)?;

        tracing::info!(
            "Input handler connected to display {} with {} keycodes mapped",
            disp,
            keycode_map.len()
        );

        Ok(Self {
            conn,
            screen_num,
            keycode_map,
        })
    }

    /// Inject a mouse move event
    pub fn mouse_move(&self, x: u16, y: u16) -> Result<()> {
        let root = self.root_window();
        self.conn
            .xtest_fake_input(6, 0, 0, root, x as i16, y as i16, 0)?
            .check()
            .context("Failed to inject mouse move")?;
        Ok(())
    }

    /// Inject a mouse button press/release
    pub fn mouse_button(&self, button: u8, pressed: bool) -> Result<()> {
        let event_type = if pressed { 4 } else { 5 }; // ButtonPress / ButtonRelease
        let root = self.root_window();
        self.conn
            .xtest_fake_input(event_type, button, 0, root, 0, 0, 0)?
            .check()
            .context("Failed to inject mouse button")?;
        Ok(())
    }

    /// Inject a mouse scroll event
    pub fn mouse_scroll(&self, dx: i16, dy: i16) -> Result<()> {
        let root = self.root_window();
        // Vertical scroll: button 4 (up) or 5 (down)
        if dy != 0 {
            let button = if dy < 0 { 4u8 } else { 5u8 };
            let clicks = dy.unsigned_abs();
            for _ in 0..clicks {
                // Press
                self.conn
                    .xtest_fake_input(4, button, 0, root, 0, 0, 0)?
                    .check()?;
                // Release
                self.conn
                    .xtest_fake_input(5, button, 0, root, 0, 0, 0)?
                    .check()?;
            }
        }
        // Horizontal scroll: button 6 (left) or 7 (right)
        if dx != 0 {
            let button = if dx < 0 { 6u8 } else { 7u8 };
            let clicks = dx.unsigned_abs();
            for _ in 0..clicks {
                self.conn
                    .xtest_fake_input(4, button, 0, root, 0, 0, 0)?
                    .check()?;
                self.conn
                    .xtest_fake_input(5, button, 0, root, 0, 0, 0)?
                    .check()?;
            }
        }
        Ok(())
    }

    /// Inject a key press/release from a DOM KeyboardEvent.code string
    pub fn key_event(&self, code: &str, pressed: bool) -> Result<()> {
        let keycode = self
            .keycode_map
            .get(code)
            .copied()
            .or_else(|| dom_code_to_keycode_fallback(code))
            .context(format!("Unknown key code: {}", code))?;

        let event_type = if pressed { 2 } else { 3 }; // KeyPress / KeyRelease
        let root = self.root_window();
        self.conn
            .xtest_fake_input(event_type, keycode, 0, root, 0, 0, 0)?
            .check()
            .context("Failed to inject key event")?;
        Ok(())
    }

    /// Set clipboard text on the X11 server
    pub fn set_clipboard(&self, text: &str) -> Result<()> {
        // Use xclip via command since clipboard handling via X11 protocol is complex
        use std::process::{Command, Stdio};
        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to run xclip")?;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    /// Get clipboard text from the X11 server
    pub fn get_clipboard(&self) -> Result<String> {
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .context("Failed to run xclip")?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn root_window(&self) -> xproto::Window {
        self.conn.setup().roots[self.screen_num].root
    }
}

/// Parse an input event from the binary protocol
pub fn parse_input_event(data: &[u8]) -> Option<InputEvent> {
    if data.is_empty() {
        return None;
    }

    match data[0] {
        0x01 if data.len() >= 5 => {
            let x = u16::from_be_bytes([data[1], data[2]]);
            let y = u16::from_be_bytes([data[3], data[4]]);
            Some(InputEvent::MouseMove { x, y })
        }
        0x02 if data.len() >= 3 => Some(InputEvent::MouseButton {
            button: data[1],
            pressed: data[2] != 0,
        }),
        0x03 if data.len() >= 5 => {
            let dx = i16::from_be_bytes([data[1], data[2]]);
            let dy = i16::from_be_bytes([data[3], data[4]]);
            Some(InputEvent::MouseScroll { dx, dy })
        }
        0x10 if data.len() >= 3 => {
            let code_len = data[1] as usize;
            if data.len() < 2 + code_len + 1 {
                return None;
            }
            let code = std::str::from_utf8(&data[2..2 + code_len]).ok()?.to_string();
            let pressed = data[2 + code_len] != 0;
            Some(InputEvent::KeyEvent { code, pressed })
        }
        0x20 if data.len() >= 5 => {
            let length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
            if data.len() < 5 + length {
                return None;
            }
            let text = std::str::from_utf8(&data[5..5 + length]).ok()?.to_string();
            Some(InputEvent::Clipboard { text })
        }
        0x30 if data.len() >= 2 => parse_control_event(data),
        _ => None,
    }
}

fn parse_control_event(data: &[u8]) -> Option<InputEvent> {
    match data[1] {
        0x01 => Some(InputEvent::RequestKeyframe),
        0x02 if data.len() >= 6 => {
            let kbps = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
            Some(InputEvent::SetBitrate { kbps })
        }
        0x03 if data.len() >= 6 => {
            let w = u16::from_be_bytes([data[2], data[3]]);
            let h = u16::from_be_bytes([data[4], data[5]]);
            Some(InputEvent::SetResolution { width: w, height: h })
        }
        _ => None,
    }
}

#[derive(Debug)]
pub enum InputEvent {
    MouseMove { x: u16, y: u16 },
    MouseButton { button: u8, pressed: bool },
    MouseScroll { dx: i16, dy: i16 },
    KeyEvent { code: String, pressed: bool },
    Clipboard { text: String },
    RequestKeyframe,
    SetBitrate { kbps: u32 },
    SetResolution { width: u16, height: u16 },
}

/// Build a mapping from DOM KeyboardEvent.code to X11 keycode
fn build_keycode_map(
    _conn: &x11rb::rust_connection::RustConnection,
    _screen_num: usize,
) -> Result<HashMap<String, u8>> {
    // Static mapping from DOM KeyboardEvent.code to X11 keycode
    // Based on standard US keyboard layout; keycodes are hardware-level
    let mut map = HashMap::new();

    // Letters
    for (i, c) in ('a'..='z').enumerate() {
        let code = format!("Key{}", c.to_uppercase().next().unwrap());
        map.insert(code, 38 + i as u8); // 'a' starts at keycode 38 on standard X11
    }

    // Digits
    for i in 0..=9u8 {
        map.insert(format!("Digit{}", i), if i == 0 { 19 } else { 10 + i - 1 });
    }

    // Function keys
    for i in 1..=12u8 {
        map.insert(format!("F{}", i), 66 + i);
    }

    // Modifiers
    map.insert("ShiftLeft".into(), 50);
    map.insert("ShiftRight".into(), 62);
    map.insert("ControlLeft".into(), 37);
    map.insert("ControlRight".into(), 105);
    map.insert("AltLeft".into(), 64);
    map.insert("AltRight".into(), 108);
    map.insert("MetaLeft".into(), 133);
    map.insert("MetaRight".into(), 134);
    map.insert("CapsLock".into(), 66);

    // Special keys
    map.insert("Space".into(), 65);
    map.insert("Enter".into(), 36);
    map.insert("Tab".into(), 23);
    map.insert("Escape".into(), 9);
    map.insert("Backspace".into(), 22);
    map.insert("Delete".into(), 119);
    map.insert("Insert".into(), 118);
    map.insert("Home".into(), 110);
    map.insert("End".into(), 115);
    map.insert("PageUp".into(), 112);
    map.insert("PageDown".into(), 117);

    // Arrow keys
    map.insert("ArrowUp".into(), 111);
    map.insert("ArrowDown".into(), 116);
    map.insert("ArrowLeft".into(), 113);
    map.insert("ArrowRight".into(), 114);

    // Punctuation / symbols
    map.insert("Minus".into(), 20);
    map.insert("Equal".into(), 21);
    map.insert("BracketLeft".into(), 34);
    map.insert("BracketRight".into(), 35);
    map.insert("Backslash".into(), 51);
    map.insert("Semicolon".into(), 47);
    map.insert("Quote".into(), 48);
    map.insert("Comma".into(), 59);
    map.insert("Period".into(), 60);
    map.insert("Slash".into(), 61);
    map.insert("Backquote".into(), 49);

    // Numpad
    for i in 0..=9u8 {
        map.insert(format!("Numpad{}", i), 90 + i);
    }
    map.insert("NumpadEnter".into(), 104);
    map.insert("NumpadAdd".into(), 86);
    map.insert("NumpadSubtract".into(), 82);
    map.insert("NumpadMultiply".into(), 63);
    map.insert("NumpadDivide".into(), 106);
    map.insert("NumpadDecimal".into(), 91);
    map.insert("NumLock".into(), 77);

    // Print Screen, Scroll Lock, Pause
    map.insert("PrintScreen".into(), 107);
    map.insert("ScrollLock".into(), 78);
    map.insert("Pause".into(), 127);

    Ok(map)
}

/// Fallback keycode resolution for codes not in the static map
fn dom_code_to_keycode_fallback(code: &str) -> Option<u8> {
    match code {
        "ContextMenu" => Some(135),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mouse_move() {
        // [0x01] [x: u16 BE] [y: u16 BE]
        let data = [0x01, 0x03, 0xE8, 0x01, 0xF4]; // x=1000, y=500
        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::MouseMove { x, y } => {
                assert_eq!(x, 1000);
                assert_eq!(y, 500);
            }
            _ => panic!("Expected MouseMove"),
        }
    }

    #[test]
    fn test_parse_mouse_button_press() {
        let data = [0x02, 1, 1]; // button=1 (left), pressed=true
        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::MouseButton { button, pressed } => {
                assert_eq!(button, 1);
                assert!(pressed);
            }
            _ => panic!("Expected MouseButton"),
        }
    }

    #[test]
    fn test_parse_mouse_button_release() {
        let data = [0x02, 3, 0]; // button=3 (right), pressed=false
        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::MouseButton { button, pressed } => {
                assert_eq!(button, 3);
                assert!(!pressed);
            }
            _ => panic!("Expected MouseButton"),
        }
    }

    #[test]
    fn test_parse_mouse_scroll() {
        // [0x03] [dx: i16 BE] [dy: i16 BE]
        let data = [0x03, 0xFF, 0xFE, 0x00, 0x03]; // dx=-2, dy=3
        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::MouseScroll { dx, dy } => {
                assert_eq!(dx, -2);
                assert_eq!(dy, 3);
            }
            _ => panic!("Expected MouseScroll"),
        }
    }

    #[test]
    fn test_parse_key_event() {
        // [0x10] [code_len: u8] [code: utf8] [pressed: u8]
        let code = b"KeyA";
        let mut data = vec![0x10, code.len() as u8];
        data.extend_from_slice(code);
        data.push(1); // pressed

        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::KeyEvent { code, pressed } => {
                assert_eq!(code, "KeyA");
                assert!(pressed);
            }
            _ => panic!("Expected KeyEvent"),
        }
    }

    #[test]
    fn test_parse_key_event_release() {
        let code = b"ShiftLeft";
        let mut data = vec![0x10, code.len() as u8];
        data.extend_from_slice(code);
        data.push(0); // released

        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::KeyEvent { code, pressed } => {
                assert_eq!(code, "ShiftLeft");
                assert!(!pressed);
            }
            _ => panic!("Expected KeyEvent"),
        }
    }

    #[test]
    fn test_parse_clipboard() {
        // [0x20] [length: u32 BE] [utf8 data...]
        let text = b"Hello, clipboard!";
        let len = text.len() as u32;
        let mut data = vec![0x20];
        data.extend_from_slice(&len.to_be_bytes());
        data.extend_from_slice(text);

        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::Clipboard { text } => {
                assert_eq!(text, "Hello, clipboard!");
            }
            _ => panic!("Expected Clipboard"),
        }
    }

    #[test]
    fn test_parse_keyframe_request() {
        let data = [0x30, 0x01];
        let event = parse_input_event(&data).unwrap();
        assert!(matches!(event, InputEvent::RequestKeyframe));
    }

    #[test]
    fn test_parse_set_bitrate() {
        // [0x30] [0x02] [kbps: u32 BE]
        let data = [0x30, 0x02, 0x00, 0x00, 0x17, 0x70]; // 6000 kbps
        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::SetBitrate { kbps } => assert_eq!(kbps, 6000),
            _ => panic!("Expected SetBitrate"),
        }
    }

    #[test]
    fn test_parse_set_resolution() {
        // [0x30] [0x03] [w: u16 BE] [h: u16 BE]
        let data = [0x30, 0x03, 0x07, 0x80, 0x04, 0x38]; // 1920x1080
        let event = parse_input_event(&data).unwrap();
        match event {
            InputEvent::SetResolution { width, height } => {
                assert_eq!(width, 1920);
                assert_eq!(height, 1080);
            }
            _ => panic!("Expected SetResolution"),
        }
    }

    #[test]
    fn test_parse_empty_data() {
        assert!(parse_input_event(&[]).is_none());
    }

    #[test]
    fn test_parse_truncated_mouse_move() {
        // Only 3 bytes, need 5
        assert!(parse_input_event(&[0x01, 0x00, 0x00]).is_none());
    }

    #[test]
    fn test_parse_truncated_key_event() {
        // Says code_len=10 but only has 3 bytes of code
        let data = [0x10, 10, b'A', b'B', b'C'];
        assert!(parse_input_event(&data).is_none());
    }

    #[test]
    fn test_parse_unknown_type() {
        assert!(parse_input_event(&[0xFF, 0x00]).is_none());
    }
}
