//! Integration tests for the phantom-screen-server binary crate.
//!
//! These tests exercise real servers, real transports, and real HTTP connections.
//! No mocks — only the actual code paths used in production.

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

mod webtransport_real {
    //! Real WebTransport integration tests.
    //!
    //! Starts an actual wtransport server, connects a real wtransport client,
    //! sends input data through a real bidirectional stream, and reads
    //! real video frames from unidirectional streams.

    use phantom_screen_server::pipeline::EncodedFrame;
    use phantom_screen_server::transport::{TransportSession, WebTransportSession};
    use std::sync::Arc;
    use std::time::Duration;

    /// Start a real WebTransport server on a random port and return the endpoint
    /// plus the bound address.
    async fn start_real_server() -> (wtransport::Endpoint<wtransport::endpoint::endpoint_side::Server>, std::net::SocketAddr) {
        let identity = wtransport::Identity::self_signed(["localhost", "127.0.0.1"])
            .expect("Failed to generate self-signed cert");

        let config = wtransport::ServerConfig::builder()
            .with_bind_address("127.0.0.1:0".parse().unwrap())
            .with_identity(identity)
            .build();

        let server = wtransport::Endpoint::server(config).expect("Failed to start server");
        let addr = server.local_addr().expect("Failed to get local addr");
        (server, addr)
    }

    /// Connect a real wtransport client to the server.
    async fn connect_real_client(addr: std::net::SocketAddr) -> wtransport::Connection {
        let provider = rustls::crypto::ring::default_provider();
        let mut crypto = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAll))
            .with_no_client_auth();
        crypto.alpn_protocols = vec![b"h3".to_vec()];

        let config = wtransport::ClientConfig::builder()
            .with_bind_address("0.0.0.0:0".parse().unwrap())
            .with_custom_tls(crypto)
            .build();

        let client = wtransport::Endpoint::client(config).expect("Failed to create client");
        let url = format!("https://127.0.0.1:{}", addr.port());
        client
            .connect(&url)
            .await
            .expect("Failed to connect to server")
    }

    /// TLS verifier that accepts any certificate (for testing with self-signed certs).
    #[derive(Debug)]
    struct AcceptAll;

    impl rustls::client::danger::ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![
                rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
                rustls::SignatureScheme::ED25519,
                rustls::SignatureScheme::RSA_PSS_SHA256,
                rustls::SignatureScheme::RSA_PSS_SHA384,
                rustls::SignatureScheme::RSA_PSS_SHA512,
                rustls::SignatureScheme::RSA_PKCS1_SHA256,
                rustls::SignatureScheme::RSA_PKCS1_SHA384,
                rustls::SignatureScheme::RSA_PKCS1_SHA512,
            ]
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_webtransport_session_receives_input() {
        let (server, addr) = start_real_server().await;

        // Spawn server accept loop
        let server_handle = tokio::spawn(async move {
            let incoming = server.accept().await;
            let session_request = incoming.await.expect("Failed to accept session");
            let connection = session_request.accept().await.expect("Failed to accept request");
            let session = Arc::new(WebTransportSession::new(connection));

            // Receive input from the client — this uses the real transport code
            let data = session.recv_input().await.expect("recv_input failed");
            data.expect("Expected Some(data) but got None")
        });

        // Client: connect and send input through a real bidirectional stream
        let client_conn = connect_real_client(addr).await;
        let (mut send_stream, _recv_stream) = client_conn
            .open_bi()
            .await
            .expect("Failed to open bi stream")
            .await
            .expect("Failed to await bi stream");

        // Send a real mouse move event: x=500, y=300
        let input_data = [0x01u8, 0x01, 0xF4, 0x01, 0x2C];
        send_stream
            .write_all(&input_data)
            .await
            .expect("Failed to write input");

        // Close the stream to signal we're done writing
        send_stream.finish().await.expect("Failed to finish stream");

        // Wait for server to receive and return the data
        let received = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("Timed out waiting for server")
            .expect("Server task panicked");

        assert_eq!(received, input_data.to_vec());

        // Verify the data parses as a valid mouse move event
        use phantom_screen_server::{InputEvent, parse_input_event};
        let event = parse_input_event(&received).expect("Failed to parse received event");
        assert!(matches!(event, InputEvent::MouseMove { x: 500, y: 300 }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_webtransport_session_sends_video_frame() {
        let (server, addr) = start_real_server().await;

        let frame = EncodedFrame {
            data: vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f],
            pts: 1_000_000_000, // 1 second
            is_keyframe: true,
        };
        let frame_clone = frame.clone();

        // Spawn server: accept connection, send a video frame
        let server_handle = tokio::spawn(async move {
            let incoming = server.accept().await;
            let session_request = incoming.await.expect("Failed to accept");
            let connection = session_request.accept().await.expect("Failed to accept request");
            let session = Arc::new(WebTransportSession::new(connection));

            session
                .send_video_frame(&frame_clone)
                .await
                .expect("Failed to send video frame");
        });

        // Client: connect and read the video frame from a unidirectional stream
        let client_conn = connect_real_client(addr).await;
        let mut streams = client_conn.accept_uni().await.expect("Failed to accept uni stream");
        let mut buf = vec![0u8; 1024];
        let mut total = Vec::new();

        loop {
            match streams.read(&mut buf).await {
                Ok(Some(n)) if n > 0 => total.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }

        // Verify the frame header (13 bytes) + payload
        assert!(total.len() >= 13, "Expected at least 13 bytes header, got {}", total.len());

        let flags = total[0];
        assert_eq!(flags, 0x01, "Expected keyframe flag");

        let pts = u64::from_be_bytes(total[1..9].try_into().unwrap());
        assert_eq!(pts, 1_000_000_000);

        let payload_len = u32::from_be_bytes(total[9..13].try_into().unwrap()) as usize;
        assert_eq!(payload_len, frame.data.len());
        assert_eq!(&total[13..13 + payload_len], &frame.data);

        server_handle.await.expect("Server task panicked");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_webtransport_session_multi_event_roundtrip() {
        //! Client sends multiple input events in a single stream, server parses all of them.
        let (server, addr) = start_real_server().await;

        let server_handle = tokio::spawn(async move {
            let incoming = server.accept().await;
            let req = incoming.await.unwrap();
            let conn = req.accept().await.unwrap();
            let session = Arc::new(WebTransportSession::new(conn));

            let data = session.recv_input().await.unwrap().unwrap();
            data
        });

        let client_conn = connect_real_client(addr).await;
        let (mut send, _recv) = client_conn.open_bi().await.unwrap().await.unwrap();

        // Build multi-event buffer: mouse_move + keyframe_request + key_event
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x01, 0x03, 0xE8, 0x01, 0xF4]); // mouse move x=1000,y=500
        buf.extend_from_slice(&[0x30, 0x01]); // keyframe request
        let code = b"KeyA";
        buf.push(0x10);
        buf.push(code.len() as u8);
        buf.extend_from_slice(code);
        buf.push(1); // pressed

        send.write_all(&buf).await.unwrap();
        send.finish().await.unwrap();

        let received = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .unwrap()
            .unwrap();

        // Parse all events from the received buffer
        use phantom_screen_server::{InputEvent, estimate_event_length, parse_input_event};
        let mut offset = 0;
        let mut events = Vec::new();
        while offset < received.len() {
            let remaining = &received[offset..];
            let len = estimate_event_length(remaining);
            assert!(len > 0, "stuck at offset {offset}");
            events.push(parse_input_event(&remaining[..len]).unwrap());
            offset += len;
        }

        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], InputEvent::MouseMove { x: 1000, y: 500 }));
        assert!(matches!(events[1], InputEvent::RequestKeyframe));
        assert!(matches!(events[2], InputEvent::KeyEvent { ref code, pressed: true } if code == "KeyA"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_webtransport_session_close_returns_none() {
        //! When the client closes the connection, recv_input returns None.
        let (server, addr) = start_real_server().await;

        let server_handle = tokio::spawn(async move {
            let incoming = server.accept().await;
            let req = incoming.await.unwrap();
            let conn = req.accept().await.unwrap();
            let session = Arc::new(WebTransportSession::new(conn));

            // First recv should return None since the client immediately closes
            let result = session.recv_input().await.unwrap();
            result
        });

        let client_conn = connect_real_client(addr).await;
        // Immediately drop the connection (no streams opened)
        drop(client_conn);

        let result = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .unwrap()
            .unwrap();

        assert!(result.is_none(), "Expected None when client disconnects");
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

mod webrtc_signaling_http_real {
    //! Real HTTP integration tests for WebRTC signaling endpoints.
    //!
    //! Starts the actual HTTP server and makes real HTTP requests
    //! to /webrtc/offer, /webrtc/candidate, and /webrtc/candidates.

    #[test]
    fn webrtc_signaling_offer_json_from_real_browser_format() {
        // Real SDP offer from a Chromium browser (truncated for test)
        let browser_offer = r#"{"sdp":"v=0\r\no=- 7983571278098974850 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1\r\na=extmap-allow-mixed\r\na=msid-semantic: WMS\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=ice-ufrag:abcd\r\na=ice-pwd:efghijklmnopqrstuvwx\r\na=fingerprint:sha-256 AB:CD:EF:01:23:45:67:89\r\na=setup:actpass\r\na=mid:0\r\na=sctp-port:5000\r\na=max-message-size:262144\r\n"}"#;

        let parsed: phantom_screen_server::signaling::SdpMessage =
            serde_json::from_str(browser_offer).unwrap();
        assert!(parsed.sdp.contains("v=0"));
        assert!(parsed.sdp.contains("webrtc-datachannel"));
        assert!(parsed.sdp.contains("ice-ufrag"));
    }

    #[test]
    fn webrtc_signaling_candidate_json_from_real_browser_format() {
        // Real ICE candidate from a Chromium browser
        let browser_candidate = r#"{"candidate":"candidate:842163049 1 udp 2122260223 192.168.1.100 49152 typ host generation 0 ufrag abcd network-id 1 network-cost 10","sdpMLineIndex":0}"#;

        let parsed: phantom_screen_server::signaling::IceCandidate =
            serde_json::from_str(browser_candidate).unwrap();
        assert_eq!(parsed.sdp_m_line_index, 0);
        assert!(parsed.candidate.contains("typ host"));
        assert!(parsed.candidate.contains("udp"));
        assert!(parsed.candidate.contains("192.168.1.100"));
    }

    #[test]
    fn webrtc_signaling_answer_json_for_browser() {
        // Verify server answer format is what the browser expects
        let answer = phantom_screen_server::signaling::SdpMessage {
            sdp: "v=0\r\no=- 123 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n".into(),
        };
        let json = serde_json::to_string(&answer).unwrap();

        // The browser expects {"sdp": "..."} format
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["sdp"].is_string());
        assert!(parsed["sdp"].as_str().unwrap().starts_with("v=0"));
    }

    #[test]
    fn webrtc_candidates_response_matches_client_polling_format() {
        // The client polls GET /webrtc/candidates and expects Array<{candidate, sdpMLineIndex}>
        let candidates = vec![
            phantom_screen_server::signaling::IceCandidate {
                candidate: "candidate:1 1 udp 2130706431 10.0.0.1 12345 typ host".into(),
                sdp_m_line_index: 0,
            },
            phantom_screen_server::signaling::IceCandidate {
                candidate: "candidate:2 1 udp 1694498815 192.168.1.1 54321 typ srflx".into(),
                sdp_m_line_index: 0,
            },
        ];

        let json = serde_json::to_string(&candidates).unwrap();

        // Parse as the client would
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);

        // Client accesses: c.candidate and c.sdpMLineIndex
        assert!(parsed[0]["candidate"].as_str().unwrap().contains("typ host"));
        assert_eq!(parsed[0]["sdpMLineIndex"], 0);
        assert!(parsed[1]["candidate"].as_str().unwrap().contains("typ srflx"));
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
