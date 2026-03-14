use anyhow::{Context, Result};
use std::collections::HashMap;
use x11rb::connection::Connection;
use x11rb::protocol::xproto;
use x11rb::protocol::xtest::ConnectionExt as XTestExt;

/// Manages X11 input injection via XTest extension
pub struct InputHandler {
    display: String,
    conn: std::sync::Mutex<x11rb::rust_connection::RustConnection>,
    screen_num: usize,
    keycode_map: std::sync::Mutex<HashMap<String, u8>>,
}

impl InputHandler {
    pub fn new(disp: &str) -> Result<Self> {
        // Set DISPLAY env var for x11rb
        // SAFETY: called before spawning threads, only modifies DISPLAY
        unsafe { std::env::set_var("DISPLAY", disp) };
        let (conn, screen_num, keycode_map) = Self::connect(disp)?;

        Ok(Self {
            display: disp.to_string(),
            conn: std::sync::Mutex::new(conn),
            screen_num,
            keycode_map: std::sync::Mutex::new(keycode_map),
        })
    }

    /// Reconnect to the X11 display after an Xvfb restart.
    pub fn reconnect(&self) -> Result<()> {
        let (conn, _screen_num, keycode_map) = Self::connect(&self.display)?;
        *self.conn.lock().unwrap() = conn;
        *self.keycode_map.lock().unwrap() = keycode_map;
        tracing::info!("Input handler reconnected to display {}", self.display);
        Ok(())
    }

    fn connect(
        disp: &str,
    ) -> Result<(
        x11rb::rust_connection::RustConnection,
        usize,
        HashMap<String, u8>,
    )> {
        let (conn, screen_num) = x11rb::rust_connection::RustConnection::connect(Some(disp))
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

        Ok((conn, screen_num, keycode_map))
    }

    /// Inject a mouse move event
    pub fn mouse_move(&self, x: u16, y: u16) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let root = Self::root_window_from(&conn, self.screen_num);
        conn.xtest_fake_input(6, 0, 0, root, x as i16, y as i16, 0)?
            .check()
            .context("Failed to inject mouse move")?;
        Ok(())
    }

    /// Inject a mouse button press/release
    pub fn mouse_button(&self, button: u8, pressed: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let event_type = if pressed { 4 } else { 5 }; // ButtonPress / ButtonRelease
        let root = Self::root_window_from(&conn, self.screen_num);
        conn.xtest_fake_input(event_type, button, 0, root, 0, 0, 0)?
            .check()
            .context("Failed to inject mouse button")?;
        Ok(())
    }

    /// Inject a mouse scroll event
    pub fn mouse_scroll(&self, dx: i16, dy: i16) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let root = Self::root_window_from(&conn, self.screen_num);
        // Vertical scroll: button 4 (up) or 5 (down)
        if dy != 0 {
            let button = if dy < 0 { 4u8 } else { 5u8 };
            let clicks = dy.unsigned_abs();
            for _ in 0..clicks {
                // Press
                conn.xtest_fake_input(4, button, 0, root, 0, 0, 0)?
                    .check()?;
                // Release
                conn.xtest_fake_input(5, button, 0, root, 0, 0, 0)?
                    .check()?;
            }
        }
        // Horizontal scroll: button 6 (left) or 7 (right)
        if dx != 0 {
            let button = if dx < 0 { 6u8 } else { 7u8 };
            let clicks = dx.unsigned_abs();
            for _ in 0..clicks {
                conn.xtest_fake_input(4, button, 0, root, 0, 0, 0)?
                    .check()?;
                conn.xtest_fake_input(5, button, 0, root, 0, 0, 0)?
                    .check()?;
            }
        }
        Ok(())
    }

    /// Inject a key press/release from a DOM KeyboardEvent.code string
    pub fn key_event(&self, code: &str, pressed: bool) -> Result<()> {
        let keycode = self
            .keycode_map
            .lock()
            .unwrap()
            .get(code)
            .copied()
            .or_else(|| dom_code_to_keycode_fallback(code))
            .context(format!("Unknown key code: {}", code))?;

        let conn = self.conn.lock().unwrap();
        let event_type = if pressed { 2 } else { 3 }; // KeyPress / KeyRelease
        let root = Self::root_window_from(&conn, self.screen_num);
        conn.xtest_fake_input(event_type, keycode, 0, root, 0, 0, 0)?
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

    fn root_window_from(
        conn: &x11rb::rust_connection::RustConnection,
        screen_num: usize,
    ) -> xproto::Window {
        conn.setup().roots[screen_num].root
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
            let code = std::str::from_utf8(&data[2..2 + code_len])
                .ok()?
                .to_string();
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
            Some(InputEvent::SetResolution {
                width: w,
                height: h,
            })
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

/// Build a mapping from DOM KeyboardEvent.code to X11 keycode.
///
/// Uses [`build_dom_to_x11_keycode_map`] for the actual mapping data.
fn build_keycode_map(
    _conn: &x11rb::rust_connection::RustConnection,
    _screen_num: usize,
) -> Result<HashMap<String, u8>> {
    Ok(build_dom_to_x11_keycode_map())
}

/// Build the static mapping from DOM `KeyboardEvent.code` strings to X11 keycodes.
///
/// Keycodes follow the standard US QWERTY keyboard layout as reported by X11.
/// Letter keycodes are assigned by physical row position (not alphabetical order):
///   Row 1 (QWERTY):  Q=24  W=25  E=26  R=27  T=28  Y=29  U=30  I=31  O=32  P=33
///   Row 2 (ASDF):    A=38  S=39  D=40  F=41  G=42  H=43  J=44  K=45  L=46
///   Row 3 (ZXCV):    Z=52  X=53  C=54  V=55  B=56  N=57  M=58
fn build_dom_to_x11_keycode_map() -> HashMap<String, u8> {
    let mut map = HashMap::new();

    // Letters — X11 keycodes follow physical QWERTY layout, not alphabetical order
    let letter_keycodes: &[(&str, u8)] = &[
        ("KeyQ", 24),
        ("KeyW", 25),
        ("KeyE", 26),
        ("KeyR", 27),
        ("KeyT", 28),
        ("KeyY", 29),
        ("KeyU", 30),
        ("KeyI", 31),
        ("KeyO", 32),
        ("KeyP", 33),
        ("KeyA", 38),
        ("KeyS", 39),
        ("KeyD", 40),
        ("KeyF", 41),
        ("KeyG", 42),
        ("KeyH", 43),
        ("KeyJ", 44),
        ("KeyK", 45),
        ("KeyL", 46),
        ("KeyZ", 52),
        ("KeyX", 53),
        ("KeyC", 54),
        ("KeyV", 55),
        ("KeyB", 56),
        ("KeyN", 57),
        ("KeyM", 58),
    ];
    for &(code, keycode) in letter_keycodes {
        map.insert(code.into(), keycode);
    }

    // Digits
    for i in 0..=9u8 {
        map.insert(format!("Digit{}", i), if i == 0 { 19 } else { 10 + i - 1 });
    }

    // Function keys (F1–F10 are 67–76, F11=95, F12=96 on standard X11)
    for i in 1..=10u8 {
        map.insert(format!("F{}", i), 66 + i);
    }
    map.insert("F11".into(), 95);
    map.insert("F12".into(), 96);

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

    // Numpad — keycodes follow physical layout, not numeric order
    map.insert("Numpad0".into(), 90);
    map.insert("Numpad1".into(), 87);
    map.insert("Numpad2".into(), 88);
    map.insert("Numpad3".into(), 89);
    map.insert("Numpad4".into(), 83);
    map.insert("Numpad5".into(), 84);
    map.insert("Numpad6".into(), 85);
    map.insert("Numpad7".into(), 79);
    map.insert("Numpad8".into(), 80);
    map.insert("Numpad9".into(), 81);
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

    map
}

/// Estimate the byte length of a binary-encoded input event.
/// Returns 0 if the event type is unknown or the buffer is too short to determine length.
pub fn estimate_event_length(data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }
    match data[0] {
        0x01 => 5, // Mouse Move
        0x02 => 3, // Mouse Button
        0x03 => 5, // Mouse Scroll
        0x10 => {
            if data.len() < 2 {
                return 0;
            }
            let code_len = data[1] as usize;
            2 + code_len + 1
        }
        0x20 => {
            if data.len() < 5 {
                return 0;
            }
            let length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
            5 + length
        }
        0x30 => {
            if data.len() < 2 {
                return 0;
            }
            match data[1] {
                0x01 => 2,
                0x02 => 6,
                0x03 => 6,
                _ => 0,
            }
        }
        _ => 0,
    }
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

    // ── Keycode mapping tests ─────────────────────────────────────────

    #[test]
    fn keycode_map_contains_all_26_letters() {
        let map = build_dom_to_x11_keycode_map();
        for c in 'A'..='Z' {
            let code = format!("Key{}", c);
            assert!(map.contains_key(&code), "Missing mapping for {}", code);
        }
    }

    #[test]
    fn keycode_map_letter_keycodes_are_unique() {
        let map = build_dom_to_x11_keycode_map();
        let mut seen: HashMap<u8, String> = HashMap::new();
        for c in 'A'..='Z' {
            let code = format!("Key{}", c);
            let keycode = map[&code];
            if let Some(prev) = seen.get(&keycode) {
                panic!(
                    "Duplicate keycode {}: both {} and {} map to it",
                    keycode, prev, code
                );
            }
            seen.insert(keycode, code);
        }
    }

    /// Verify letter keycodes match the standard X11 QWERTY layout.
    /// These values come from `xmodmap -pke` on a standard US keyboard.
    #[test]
    fn keycode_map_qwerty_row1() {
        let map = build_dom_to_x11_keycode_map();
        // Top row: Q W E R T Y U I O P
        assert_eq!(map["KeyQ"], 24);
        assert_eq!(map["KeyW"], 25);
        assert_eq!(map["KeyE"], 26);
        assert_eq!(map["KeyR"], 27);
        assert_eq!(map["KeyT"], 28);
        assert_eq!(map["KeyY"], 29);
        assert_eq!(map["KeyU"], 30);
        assert_eq!(map["KeyI"], 31);
        assert_eq!(map["KeyO"], 32);
        assert_eq!(map["KeyP"], 33);
    }

    #[test]
    fn keycode_map_qwerty_row2() {
        let map = build_dom_to_x11_keycode_map();
        // Home row: A S D F G H J K L
        assert_eq!(map["KeyA"], 38);
        assert_eq!(map["KeyS"], 39);
        assert_eq!(map["KeyD"], 40);
        assert_eq!(map["KeyF"], 41);
        assert_eq!(map["KeyG"], 42);
        assert_eq!(map["KeyH"], 43);
        assert_eq!(map["KeyJ"], 44);
        assert_eq!(map["KeyK"], 45);
        assert_eq!(map["KeyL"], 46);
    }

    #[test]
    fn keycode_map_qwerty_row3() {
        let map = build_dom_to_x11_keycode_map();
        // Bottom row: Z X C V B N M
        assert_eq!(map["KeyZ"], 52);
        assert_eq!(map["KeyX"], 53);
        assert_eq!(map["KeyC"], 54);
        assert_eq!(map["KeyV"], 55);
        assert_eq!(map["KeyB"], 56);
        assert_eq!(map["KeyN"], 57);
        assert_eq!(map["KeyM"], 58);
    }

    /// Guard against the original bug: letters must NOT be mapped sequentially.
    /// If KeyB == KeyA + 1, the mapping is alphabetical (wrong) instead of QWERTY.
    #[test]
    fn keycode_map_letters_are_not_sequential_alphabetical() {
        let map = build_dom_to_x11_keycode_map();
        let a = map["KeyA"];
        let b = map["KeyB"];
        assert_ne!(
            b,
            a + 1,
            "KeyB should not be KeyA+1 (that's alphabetical, not QWERTY)"
        );
        let l = map["KeyL"];
        assert_ne!(
            l,
            a + 11,
            "KeyL should not be KeyA+11 (that's alphabetical, not QWERTY)"
        );
    }

    #[test]
    fn keycode_map_letter_keycodes_do_not_collide_with_special_keys() {
        let map = build_dom_to_x11_keycode_map();
        let letter_keycodes: Vec<u8> = ('A'..='Z').map(|c| map[&format!("Key{}", c)]).collect();

        let special_keys = [
            "Space",
            "Enter",
            "Tab",
            "Escape",
            "Backspace",
            "ShiftLeft",
            "ShiftRight",
            "ControlLeft",
            "ControlRight",
            "AltLeft",
            "AltRight",
        ];
        for key in special_keys {
            let kc = map[key];
            assert!(
                !letter_keycodes.contains(&kc),
                "{} (keycode {}) collides with a letter key",
                key,
                kc
            );
        }
    }

    #[test]
    fn keycode_map_digits() {
        let map = build_dom_to_x11_keycode_map();
        assert_eq!(map["Digit1"], 10);
        assert_eq!(map["Digit2"], 11);
        assert_eq!(map["Digit9"], 18);
        assert_eq!(map["Digit0"], 19);
    }

    #[test]
    fn keycode_map_special_keys() {
        let map = build_dom_to_x11_keycode_map();
        assert_eq!(map["Space"], 65);
        assert_eq!(map["Enter"], 36);
        assert_eq!(map["Tab"], 23);
        assert_eq!(map["Escape"], 9);
        assert_eq!(map["Backspace"], 22);
    }

    #[test]
    fn keycode_map_arrow_keys() {
        let map = build_dom_to_x11_keycode_map();
        assert_eq!(map["ArrowUp"], 111);
        assert_eq!(map["ArrowDown"], 116);
        assert_eq!(map["ArrowLeft"], 113);
        assert_eq!(map["ArrowRight"], 114);
    }

    #[test]
    fn keycode_map_no_duplicate_keycodes_across_entire_map() {
        let map = build_dom_to_x11_keycode_map();
        let mut keycode_to_codes: HashMap<u8, Vec<&String>> = HashMap::new();
        for (code, &keycode) in &map {
            keycode_to_codes.entry(keycode).or_default().push(code);
        }
        for (keycode, codes) in &keycode_to_codes {
            if codes.len() > 1 {
                panic!(
                    "Keycode {} is mapped by multiple DOM codes: {:?}",
                    keycode, codes
                );
            }
        }
    }

    #[test]
    fn keycode_map_function_keys() {
        let map = build_dom_to_x11_keycode_map();
        // F1–F10 are sequential 67–76
        for i in 1..=10u8 {
            assert_eq!(map[&format!("F{}", i)], 66 + i, "F{} keycode wrong", i);
        }
        // F11 and F12 are at 95 and 96 (not 77/78 which are NumLock/ScrollLock)
        assert_eq!(map["F11"], 95);
        assert_eq!(map["F12"], 96);
    }

    #[test]
    fn keycode_map_numpad() {
        let map = build_dom_to_x11_keycode_map();
        assert_eq!(map["Numpad7"], 79);
        assert_eq!(map["Numpad8"], 80);
        assert_eq!(map["Numpad9"], 81);
        assert_eq!(map["Numpad4"], 83);
        assert_eq!(map["Numpad5"], 84);
        assert_eq!(map["Numpad6"], 85);
        assert_eq!(map["Numpad1"], 87);
        assert_eq!(map["Numpad2"], 88);
        assert_eq!(map["Numpad3"], 89);
        assert_eq!(map["Numpad0"], 90);
        assert_eq!(map["NumpadDecimal"], 91);
        assert_eq!(map["NumpadEnter"], 104);
    }

    /// Guard against sequential numpad bug: Numpad1 must NOT be Numpad0+1.
    #[test]
    fn keycode_map_numpad_not_sequential() {
        let map = build_dom_to_x11_keycode_map();
        assert_ne!(
            map["Numpad1"],
            map["Numpad0"] + 1,
            "Numpad1 must not be Numpad0+1 (numpad layout is not sequential)"
        );
    }
}
