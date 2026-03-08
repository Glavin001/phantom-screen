# Phantom Screen

Web-based remote desktop streaming via WebTransport + WebCodecs.

A remote desktop server that captures a Linux desktop using GStreamer, encodes it as H.264 video, and streams it to a browser over WebTransport. The browser decodes with hardware-accelerated WebCodecs and renders to canvas. Mouse and keyboard input flows back over the same connection.

## Architecture

```
SERVER (Linux container)                    CLIENT (Chrome)
┌───────────────────┐                       ┌──────────────────────┐
│  Xvfb (:99)       │                       │  WebTransport        │
│  Virtual Display   │                       │  + WebCodecs decode  │
│  + Window Manager  │                       │                      │
└────────┬──────────┘                       │  Video → Canvas      │
         │ XShmGetImage                      │                      │
┌────────┴──────────┐                       │  Input events        │
│  GStreamer Pipeline│    WebTransport       │  → server via        │
│  ximagesrc →       │ ═══════════════════> │     bidi stream      │
│  x264enc →         │  (single HTTPS port)  │                      │
│  appsink           │                       │                      │
└───────────────────┘                       └──────────────────────┘
```

## Features

- **Screen capture**: GStreamer `ximagesrc` at up to 60fps
- **H.264 encoding**: Software (`x264enc`) with auto-upgrade to `nvh264enc` (NVIDIA) or `vaapih264enc` (AMD/Intel)
- **Transport**: WebTransport over QUIC — no head-of-line blocking, no STUN/TURN
- **Browser decode**: WebCodecs `VideoDecoder` with `hardwareAcceleration: 'prefer-hardware'`
- **Input**: Full mouse (move, click, scroll) and keyboard via X11 XTest injection
- **Clipboard**: Bidirectional text clipboard sync
- **Adaptive quality**: Keyframe-on-demand, encoder bitrate adjustment
- **Single port**: All traffic over one HTTPS/WebTransport port
- **Docker ready**: Single container deployment

## Quick Start

### Prerequisites

- Linux with X11 (or headless with Xvfb)
- GStreamer 1.20+ with plugins
- Rust (latest stable)
- Node.js 20+
- Chrome browser

### Install System Dependencies (Debian/Ubuntu)

```bash
apt-get install -y \
  xvfb openbox \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-x \
  xdotool xclip \
  pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libx11-dev libxcb1-dev libxcb-xtest0-dev
```

### Build & Run

```bash
# Build client
cd client
npm install
npm run build
cd ..

# Build server
cd server
cargo build --release
cd ..

# Run (starts Xvfb, window manager, and streaming server)
./server/target/release/phantom-screen-server \
  --display :99 \
  --resolution 1920x1080 \
  --fps 30 \
  --bitrate 6000

# Open in Chrome: https://localhost:4443
# Static files served on http://localhost:4444
```

### Docker

```bash
# Build
docker build -t phantom-screen -f server/Dockerfile .

# Run
docker run -p 4443:4443/udp -p 4443:4443/tcp -p 4444:4444/tcp \
  phantom-screen

# Open in Chrome: https://localhost:4443
```

### Chrome Self-Signed Certificate

For development with self-signed certificates, launch Chrome with:

```bash
chrome --ignore-certificate-errors --origin-to-force-quic-on=localhost:4443
```

Or navigate to `chrome://flags/#allow-insecure-localhost` and enable it.

## Configuration

```
USAGE: phantom-screen-server [OPTIONS]

OPTIONS:
  --display <DISPLAY>          X11 display to capture [default: :99]
  --resolution <WxH>           Virtual display resolution [default: 1920x1080]
  --listen <ADDR:PORT>         Listen address [default: 0.0.0.0:4443]
  --fps <N>                    Video framerate [default: 30]
  --bitrate <KBPS>             H.264 bitrate in kbps [default: 6000]
  --keyframe-interval <N>      Keyframe interval in frames [default: 60]
  --cert <PATH>                TLS certificate PEM file
  --key <PATH>                 TLS private key PEM file
  --client-dir <PATH>          Web client files directory [default: ../client/dist]
  --no-xvfb                    Skip starting Xvfb
  --wm <COMMAND>               Window manager command [default: openbox]
  --jwt-secret <SECRET>        JWT secret for auth (env: PHANTOM_JWT_SECRET)
```

## Input Protocol

Binary protocol over WebTransport bidirectional stream:

| Event | Format | Size |
|-------|--------|------|
| Mouse Move | `[0x01] [x: u16] [y: u16]` | 5 bytes |
| Mouse Button | `[0x02] [button: u8] [pressed: u8]` | 3 bytes |
| Mouse Scroll | `[0x03] [dx: i16] [dy: i16]` | 5 bytes |
| Key Event | `[0x10] [code_len: u8] [code: utf8] [pressed: u8]` | variable |
| Clipboard | `[0x20] [length: u32] [utf8 data...]` | variable |
| Keyframe Req | `[0x30] [0x01]` | 2 bytes |
| Set Bitrate | `[0x30] [0x02] [kbps: u32]` | 6 bytes |
| Set Resolution | `[0x30] [0x03] [w: u16] [h: u16]` | 6 bytes |

Keyboard events use `KeyboardEvent.code` strings (e.g., "KeyA", "ShiftLeft", "Enter").

## Project Structure

```
phantom-screen/
├── server/                     # Rust server
│   ├── Cargo.toml
│   ├── Dockerfile
│   └── src/
│       ├── main.rs             # Entry point, WebTransport server, static files
│       ├── config.rs           # CLI args, encoder auto-detection
│       ├── pipeline.rs         # GStreamer pipeline construction
│       ├── input.rs            # Input protocol parsing, X11 XTest injection
│       └── control.rs          # Keyframe/bitrate/resolution control
├── client/                     # TypeScript web client
│   ├── index.html              # Single-page UI
│   ├── package.json
│   ├── tsconfig.json
│   ├── vite.config.ts
│   └── src/
│       ├── main.ts             # Entry: WebTransport, WebCodecs, video rendering
│       ├── input.ts            # Mouse/keyboard capture + binary serialization
│       ├── clipboard.ts        # Clipboard sync
│       ├── control.ts          # Stats, keyframe requests, resolution negotiation
│       └── ui.ts               # Fullscreen, cursor, status bar, auto-hide
└── README.md
```

## Target Latency

| Environment | Expected Latency |
|-------------|-----------------|
| LAN / Tailscale | 25–50ms |
| Good internet | 50–80ms |
| Software encode (no GPU) | +8–15ms encode |
| Hardware encode (GPU) | +2–5ms encode |

## Browser Support

| Browser | Status |
|---------|--------|
| Chrome 97+ | Supported |
| Edge 97+ | Supported |
| Firefox 130+ | Experimental |
| Safari | Pending (WebTransport in stable) |

## License

MIT
