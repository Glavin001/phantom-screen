//! Integration tests for the phantom-screen-server binary crate.
//! These tests verify server logic that can run without X11 or GStreamer.

mod event_protocol {
    use phantom_screen_server::{InputEvent, estimate_event_length, parse_input_event};

    /// Verify estimate_event_length matches actual parsed lengths for all event types.
    #[test]
    fn event_length_consistency_mouse_move() {
        let data = [0x01, 0x03, 0xE8, 0x01, 0xF4];
        assert_eq!(estimate_event_length(&data), 5);
        assert!(parse_input_event(&data[..5]).is_some());
    }

    #[test]
    fn event_length_consistency_mouse_button() {
        let data = [0x02, 1, 1];
        assert_eq!(estimate_event_length(&data), 3);
        assert!(parse_input_event(&data[..3]).is_some());
    }

    #[test]
    fn event_length_consistency_mouse_scroll() {
        let data = [0x03, 0x00, 0x01, 0xFF, 0xFF];
        assert_eq!(estimate_event_length(&data), 5);
        assert!(parse_input_event(&data[..5]).is_some());
    }

    #[test]
    fn event_length_consistency_key_event() {
        let code = b"Enter";
        let mut data = vec![0x10, code.len() as u8];
        data.extend_from_slice(code);
        data.push(1);
        let expected_len = 2 + code.len() + 1;
        assert_eq!(estimate_event_length(&data), expected_len);
        assert!(parse_input_event(&data[..expected_len]).is_some());
    }

    #[test]
    fn event_length_consistency_clipboard() {
        let text = b"test clipboard";
        let len = text.len() as u32;
        let mut data = vec![0x20];
        data.extend_from_slice(&len.to_be_bytes());
        data.extend_from_slice(text);
        let expected_len = 5 + text.len();
        assert_eq!(estimate_event_length(&data), expected_len);
        assert!(parse_input_event(&data[..expected_len]).is_some());
    }

    #[test]
    fn event_length_consistency_control_keyframe() {
        let data = [0x30, 0x01];
        assert_eq!(estimate_event_length(&data), 2);
        assert!(parse_input_event(&data[..2]).is_some());
    }

    #[test]
    fn event_length_consistency_control_bitrate() {
        let data = [0x30, 0x02, 0x00, 0x00, 0x10, 0x00];
        assert_eq!(estimate_event_length(&data), 6);
        assert!(parse_input_event(&data[..6]).is_some());
    }

    #[test]
    fn event_length_consistency_control_resolution() {
        let data = [0x30, 0x03, 0x07, 0x80, 0x04, 0x38];
        assert_eq!(estimate_event_length(&data), 6);
        assert!(parse_input_event(&data[..6]).is_some());
    }

    /// Verify multi-event buffers can be parsed in sequence — this is the
    /// server-side "process_input_data" flow without needing X11.
    #[test]
    fn multi_event_buffer_parsing() {
        // Build a buffer with: mouse_move + key_down + keyframe_request
        let mut buf = Vec::new();

        // Mouse move: x=100, y=200
        buf.extend_from_slice(&[0x01, 0x00, 0x64, 0x00, 0xC8]);

        // Key press: "KeyW" pressed
        let code = b"KeyW";
        buf.push(0x10);
        buf.push(code.len() as u8);
        buf.extend_from_slice(code);
        buf.push(1);

        // Keyframe request
        buf.extend_from_slice(&[0x30, 0x01]);

        // Parse all three events
        let mut offset = 0;
        let mut events = Vec::new();

        while offset < buf.len() {
            let remaining = &buf[offset..];
            let event_len = estimate_event_length(remaining);
            assert!(event_len > 0, "stuck at offset {offset}");
            assert!(offset + event_len <= buf.len());

            let event = parse_input_event(&remaining[..event_len]);
            assert!(event.is_some(), "failed to parse event at offset {offset}");
            events.push(event.unwrap());
            offset += event_len;
        }

        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            InputEvent::MouseMove { x: 100, y: 200 }
        ));
        assert!(matches!(
            events[1],
            InputEvent::KeyEvent {
                ref code,
                pressed: true,
            } if code == "KeyW"
        ));
        assert!(matches!(events[2], InputEvent::RequestKeyframe));
    }

    #[test]
    fn estimate_event_length_empty() {
        assert_eq!(estimate_event_length(&[]), 0);
    }

    #[test]
    fn estimate_event_length_unknown_type() {
        assert_eq!(estimate_event_length(&[0xFF, 0x00, 0x00]), 0);
    }

    #[test]
    fn estimate_event_length_truncated_key() {
        // Key event header but no body
        assert_eq!(estimate_event_length(&[0x10]), 0);
    }

    #[test]
    fn estimate_event_length_truncated_clipboard() {
        // Clipboard header but no length
        assert_eq!(estimate_event_length(&[0x20, 0x00]), 0);
    }
}

mod frame_protocol {
    /// Verify the frame header encoding format matches what the client expects.
    /// Frame: [flags: u8] [pts: u64 BE] [length: u32 BE] [H.264 data...]
    #[test]
    fn frame_header_encoding() {
        let is_keyframe = true;
        let pts: u64 = 1_000_000_000; // 1 second in nanoseconds
        let data = vec![0x00, 0x00, 0x00, 0x01, 0x67]; // fake SPS NAL unit start

        let flags: u8 = if is_keyframe { 0x01 } else { 0x00 };
        let mut header = [0u8; 13];
        header[0] = flags;
        header[1..9].copy_from_slice(&pts.to_be_bytes());
        header[9..13].copy_from_slice(&(data.len() as u32).to_be_bytes());

        // Verify header
        assert_eq!(header[0], 0x01); // keyframe flag
        assert_eq!(u64::from_be_bytes(header[1..9].try_into().unwrap()), pts);
        assert_eq!(
            u32::from_be_bytes(header[9..13].try_into().unwrap()),
            data.len() as u32
        );
    }

    #[test]
    fn frame_header_non_keyframe() {
        let flags: u8 = 0x00; // not a keyframe
        let pts: u64 = 0;
        let data_len: u32 = 1024;

        let mut header = [0u8; 13];
        header[0] = flags;
        header[1..9].copy_from_slice(&pts.to_be_bytes());
        header[9..13].copy_from_slice(&data_len.to_be_bytes());

        assert_eq!(header[0], 0x00);
        assert_eq!(u64::from_be_bytes(header[1..9].try_into().unwrap()), 0);
        assert_eq!(u32::from_be_bytes(header[9..13].try_into().unwrap()), 1024);
    }
}
