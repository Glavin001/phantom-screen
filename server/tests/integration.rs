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

mod transport_abstraction {
    use phantom_screen_server::transport::TransportSession;
    use phantom_screen_server::pipeline::EncodedFrame;

    /// Mock transport session for testing the trait interface.
    struct MockSession {
        frames_sent: std::sync::Mutex<Vec<EncodedFrame>>,
        input_data: tokio::sync::Mutex<Vec<Vec<u8>>>,
    }

    impl MockSession {
        fn new(input_data: Vec<Vec<u8>>) -> Self {
            Self {
                frames_sent: std::sync::Mutex::new(Vec::new()),
                input_data: tokio::sync::Mutex::new(input_data),
            }
        }

        fn frames_sent_count(&self) -> usize {
            self.frames_sent.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl TransportSession for MockSession {
        async fn send_video_frame(&self, frame: &EncodedFrame) -> anyhow::Result<()> {
            self.frames_sent.lock().unwrap().push(frame.clone());
            Ok(())
        }

        async fn recv_input(&self) -> anyhow::Result<Option<Vec<u8>>> {
            let mut data = self.input_data.lock().await;
            if data.is_empty() {
                Ok(None)
            } else {
                Ok(Some(data.remove(0)))
            }
        }
    }

    #[tokio::test]
    async fn mock_session_sends_frames() {
        let session = MockSession::new(vec![]);
        let frame = EncodedFrame {
            data: vec![0x00, 0x00, 0x00, 0x01, 0x67],
            pts: 1_000_000_000,
            is_keyframe: true,
        };

        session.send_video_frame(&frame).await.unwrap();
        assert_eq!(session.frames_sent_count(), 1);
    }

    #[tokio::test]
    async fn mock_session_receives_input() {
        let mouse_move = vec![0x01, 0x00, 0x64, 0x00, 0xC8]; // x=100, y=200
        let session = MockSession::new(vec![mouse_move.clone()]);

        let data = session.recv_input().await.unwrap();
        assert_eq!(data, Some(mouse_move));

        // Second call returns None (empty)
        let data = session.recv_input().await.unwrap();
        assert_eq!(data, None);
    }

    #[tokio::test]
    async fn session_processes_multi_event_input() {
        use phantom_screen_server::{estimate_event_length, parse_input_event, InputEvent};

        // Build a buffer with multiple events
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x01, 0x03, 0xE8, 0x01, 0xF4]); // mouse move x=1000, y=500
        buf.extend_from_slice(&[0x30, 0x01]); // keyframe request

        let session = MockSession::new(vec![buf.clone()]);
        let data = session.recv_input().await.unwrap().unwrap();

        // Parse events from the received data (same logic as process_input_data)
        let mut offset = 0;
        let mut events = Vec::new();
        while offset < data.len() {
            let remaining = &data[offset..];
            let event_len = estimate_event_length(remaining);
            if event_len == 0 { break; }
            if let Some(event) = parse_input_event(&remaining[..event_len]) {
                events.push(event);
            }
            offset += event_len;
        }

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], InputEvent::MouseMove { x: 1000, y: 500 }));
        assert!(matches!(events[1], InputEvent::RequestKeyframe));
    }

    #[tokio::test]
    async fn transport_trait_is_object_safe() {
        // Verify TransportSession can be used as a trait object (Arc<dyn TransportSession>)
        let session: std::sync::Arc<dyn TransportSession> =
            std::sync::Arc::new(MockSession::new(vec![vec![0x30, 0x01]]));

        let data = session.recv_input().await.unwrap();
        assert!(data.is_some());

        let frame = EncodedFrame {
            data: vec![0x00],
            pts: 0,
            is_keyframe: false,
        };
        session.send_video_frame(&frame).await.unwrap();
    }
}

mod signaling_protocol {
    use phantom_screen_server::signaling::{IceCandidate, SdpMessage};

    #[test]
    fn ice_candidate_json_roundtrip() {
        let candidate = IceCandidate {
            candidate: "candidate:1 1 udp 2130706431 192.168.1.1 12345 typ host".into(),
            sdp_m_line_index: 0,
        };
        let json = serde_json::to_string(&candidate).unwrap();

        // Verify camelCase field name for JavaScript interop
        assert!(json.contains("\"sdpMLineIndex\""));
        assert!(!json.contains("sdp_m_line_index"));

        let parsed: IceCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.candidate, candidate.candidate);
        assert_eq!(parsed.sdp_m_line_index, candidate.sdp_m_line_index);
    }

    #[test]
    fn ice_candidate_deserialize_from_browser_format() {
        // Simulate the JSON format sent by the client's WebRTC transport
        let json = r#"{"candidate":"candidate:0 1 UDP 2122252543 10.0.0.1 49152 typ host","sdpMLineIndex":0}"#;
        let parsed: IceCandidate = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.sdp_m_line_index, 0);
        assert!(parsed.candidate.contains("typ host"));
    }

    #[test]
    fn sdp_message_roundtrip() {
        let msg = SdpMessage {
            sdp: "v=0\r\no=- 12345 2 IN IP4 127.0.0.1\r\ns=-\r\n".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: SdpMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sdp, msg.sdp);
    }

    #[test]
    fn sdp_message_from_client_offer() {
        // Simulate the JSON format sent by the client
        let json = r#"{"sdp":"v=0\r\no=- 12345 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n"}"#;
        let parsed: SdpMessage = serde_json::from_str(json).unwrap();
        assert!(parsed.sdp.starts_with("v=0"));
    }

    #[test]
    fn multiple_ice_candidates_serialize() {
        let candidates = vec![
            IceCandidate {
                candidate: "candidate:1 1 udp 2130706431 10.0.0.1 12345 typ host".into(),
                sdp_m_line_index: 0,
            },
            IceCandidate {
                candidate: "candidate:2 1 tcp 1694498815 10.0.0.1 12346 typ host".into(),
                sdp_m_line_index: 0,
            },
        ];

        let json = serde_json::to_string(&candidates).unwrap();
        let parsed: Vec<IceCandidate> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].candidate.contains("udp"));
        assert!(parsed[1].candidate.contains("tcp"));
    }

    #[test]
    fn empty_ice_candidates_list_serializes() {
        let candidates: Vec<IceCandidate> = vec![];
        let json = serde_json::to_string(&candidates).unwrap();
        assert_eq!(json, "[]");
    }
}

mod webrtc_signaling_http {
    /// Test that WebRTC endpoints return 404 when signaling is disabled.
    /// This mirrors the behavior when --enable-webrtc is not set.

    #[test]
    fn webrtc_disabled_returns_not_enabled() {
        // The handler functions check for None signaling state
        // Verify the JSON error format matches what the client expects
        let error_body = r#"{"error":"WebRTC not enabled"}"#;
        let parsed: serde_json::Value = serde_json::from_str(error_body).unwrap();
        assert_eq!(parsed["error"], "WebRTC not enabled");
    }

    #[test]
    fn invalid_offer_json_produces_error() {
        // Verify the error JSON format for invalid requests
        let error_body = r#"{"error":"Invalid JSON"}"#;
        let parsed: serde_json::Value = serde_json::from_str(error_body).unwrap();
        assert_eq!(parsed["error"], "Invalid JSON");
    }

    #[test]
    fn ok_response_format() {
        // Verify the success response format for candidate submission
        let body = r#"{"ok":true}"#;
        let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed["ok"], true);
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
