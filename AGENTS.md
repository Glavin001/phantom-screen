# AGENTS.md

## Cursor Cloud specific instructions

### Project overview

Phantom Screen is a web-based remote desktop streaming app with two components:
- **Server** (`server/`): Rust binary using GStreamer, X11, WebTransport (port 4443) and HTTP static file serving (port 4444)
- **Client** (`client/`): TypeScript SPA built with Vite, served as static files by the server

### Lint, test, build commands

See `README.md` for full details. Quick reference:

| Task | Client (`client/`) | Server (`server/`) |
|------|-------------------|-------------------|
| Lint | `npx tsc --noEmit` | `cargo fmt -- --check && cargo clippy -- -D warnings` |
| Test | `npm test` | `cargo test --verbose` |
| Build | `npm run build` | `cargo build --release` |
| Dev server | `npm run dev` (port 3000) | N/A |

### Running the server

```bash
./server/target/release/phantom-screen-server \
  --display :99 --resolution 1280x720 --fps 15 --bitrate 3000 \
  --client-dir ./client/dist
```

The server auto-starts Xvfb (`:99`), openbox window manager, and the GStreamer pipeline. It generates self-signed TLS certs if `--cert`/`--key` are not provided.

### Non-obvious caveats

- **Rust edition 2024**: The server requires Rust >= 1.85. The pre-installed Rust in the VM may be older; run `rustup update stable` if `cargo build` fails with edition errors.
- **WebTransport + self-signed certs**: Chrome rejects self-signed certs for WebTransport by default. The `--ignore-certificate-errors` flag does NOT help for QUIC/WebTransport. The correct approach is to use `serverCertificateHashes` in the `WebTransport` constructor with the SHA-256 hash of the server's DER-encoded certificate. See the wtransport docs and `Certificate::hash()` method.
- **Docker is the easiest way to run end-to-end**: `docker build -t phantom-screen -f server/Dockerfile .` then `docker run -p 4443:4443/udp -p 4443:4443/tcp -p 4444:4444/tcp phantom-screen`. To test with cert pinning, generate a cert with `wtransport::Identity::self_signed()`, mount it into the container with `--cert`/`--key`, and use the cert hash in a test client page.
- **WebCodecs H.264 decoding requires GPU support**: The Cloud VM uses SwiftShader (software GL) which does not support H.264 via WebCodecs. The WebTransport connection and frame transport work correctly, but video rendering in the browser will fail with "Unsupported configuration" in this environment. This is a VM limitation, not a code issue.
- **System packages**: GStreamer dev libs and X11 dev headers are needed at compile time. Runtime also needs `xvfb`, `openbox`, `gstreamer1.0-plugins-*`, `xdotool`, and `xclip`. These are installed by the VM snapshot but may need reinstalling if the base image changes.
- **Client must be built before server can serve it**: Run `npm run build` in `client/` to produce `client/dist/` before starting the server. Alternatively, use the Vite dev server (`npm run dev` in `client/`) on port 3000 for hot-reloading during client development.
