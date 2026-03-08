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
- **WebTransport + self-signed certs**: Chrome rejects self-signed certs for WebTransport by default. The "Connect" button will show "Opening handshake failed." To test the full streaming flow, launch Chrome with `--ignore-certificate-errors --origin-to-force-quic-on=localhost:4443`, or enable `chrome://flags/#allow-insecure-localhost`.
- **System packages**: GStreamer dev libs and X11 dev headers are needed at compile time. Runtime also needs `xvfb`, `openbox`, `gstreamer1.0-plugins-*`, `xdotool`, and `xclip`. These are installed by the VM snapshot but may need reinstalling if the base image changes.
- **Client must be built before server can serve it**: Run `npm run build` in `client/` to produce `client/dist/` before starting the server. Alternatively, use the Vite dev server (`npm run dev` in `client/`) on port 3000 for hot-reloading during client development.
