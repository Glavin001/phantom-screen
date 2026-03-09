# Phantom Screen

Web-based remote desktop streaming over WebTransport + WebCodecs.

Phantom Screen has two deliverables:

- `server/`: a Rust WebTransport server for Linux/X11 hosts.
- `client/`: a browser client that now ships both as an embeddable npm package and as a plain HTML/IIFE bundle.

## Architecture

```text
SERVER (Linux/X11)                         CLIENT (browser)
┌──────────────────────┐                   ┌─────────────────────────┐
│ Xvfb / real display  │                   │ WebTransport            │
│ + window manager     │                   │ + WebCodecs decode      │
└──────────┬───────────┘                   └──────────┬──────────────┘
           │ X11 capture                                 │
┌──────────┴───────────┐       H.264 over QUIC           │
│ GStreamer pipeline   │ ===============================>│
│ ximagesrc -> x264enc │                                 │
└──────────────────────┘                                 │
           ^                                             │
           └──────────── input + clipboard ──────────────┘
```

## Release artifacts

Every `main` commit produces immutable GitHub Release assets tagged as `build-<commit-sha>`.

Artifacts include:

- `phantom-screen-server-<version>-linux-x64.tar.gz`
- `phantom-screen-server-<version>-linux-arm64.tar.gz`
- `phantom-screen-web-client-<version>.tgz` for `npm install <tarball>`
- `phantom-screen-html-client-<version>.tar.gz` with `phantom-screen-client.iife.js`
- `phantom-screen-standalone-client-<version>.tar.gz`

Every CI run also uploads the same distributables as workflow artifacts before any release publish step.

## Install the server bundle

The server bundle targets Linux because the capture pipeline depends on X11 + GStreamer.

### Runtime packages (Debian/Ubuntu)

```bash
sudo apt-get install -y \
  xvfb openbox \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-x \
  xdotool xclip
```

### Run from a release archive

```bash
tar -xzf phantom-screen-server-0.1.0-linux-x64.tar.gz
cd phantom-screen-server-0.1.0-linux-x64

bash ./tools/generate-dev-cert.sh ./certs

./run-server.sh \
  --display :99 \
  --resolution 1280x720 \
  --fps 15 \
  --bitrate 3000 \
  --cert ./certs/cert.pem \
  --key ./certs/key.pem
```

The wrapper script points the binary at the bundled standalone client automatically.

## Install the browser client in an app

### Install from a release tarball

```bash
npm install ./phantom-screen-web-client-0.1.0.tgz
```

### Next.js / React example

```tsx
'use client';

import { useEffect, useRef } from 'react';
import { mountPhantomScreen } from '@phantom-screen/web-client';

export function RemoteDesktop() {
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!ref.current) return;

    const client = mountPhantomScreen(ref.current, {
      serverUrl: 'https://127.0.0.1:4443',
      serverCertificateHash: process.env.NEXT_PUBLIC_PHANTOM_CERT_HASH,
    });

    return () => client.destroy();
  }, []);

  return <div ref={ref} style={{ width: '100%', height: '70vh' }} />;
}
```

### Plain HTML bundle example

```html
<div id="remote-desktop" style="height:70vh"></div>
<script src="./phantom-screen-client.iife.js"></script>
<script>
  window.PhantomScreenClient.mountPhantomScreen(
    document.getElementById('remote-desktop'),
    {
      serverUrl: 'https://127.0.0.1:4443',
      serverCertificateHash: 'paste-the-sha256-cert-hash-here',
    },
  );
</script>
```

The release HTML bundle archive also includes `embed.html` as a ready-to-edit example page.

## Build from source

### Prerequisites

- Linux with X11 or Xvfb for the server
- GStreamer 1.20+ with plugins
- Rust stable
- Node.js 20+
- Chrome or Edge for the client

### Build the client and server

```bash
cd client
npm install
npm run build
cd ..

cd server
cargo build --release
cd ..
```

Client output directories:

- `client/dist/npm`: npm-consumable ESM/CJS bundle + types
- `client/dist/html`: plain browser IIFE bundle
- `client/dist/standalone`: standalone web app served by the server

## Local end-to-end test flow

`WebTransport` does not trust self-signed certs just because the browser ignores normal HTTPS errors. Use certificate hashes instead, and make sure the development certificate is WebTransport-compatible (ECDSA P-256 and short-lived; Chrome rejects the old RSA/365-day profile during the QUIC handshake).

### 1. Generate a dev cert and copy the printed SHA-256 hash

```bash
bash scripts/generate-dev-cert-linux.sh ./.tmp/dev-cert
```

### 2. Start the server with the generated cert

```bash
./server/target/release/phantom-screen-server \
  --display :99 \
  --resolution 1280x720 \
  --fps 15 \
  --bitrate 3000 \
  --client-dir ./client/dist/standalone \
  --cert ./.tmp/dev-cert/cert.pem \
  --key ./.tmp/dev-cert/key.pem
```

### 3. Open the standalone client

Use the printed hash either in the form input or directly in the URL:

```text
http://127.0.0.1:4444/?serverUrl=https://127.0.0.1:4443&certHash=<hex-or-base64-hash>&autoconnect=1
```

The packaged client defaults to software decoding (`prefer-software`) so it works on cloud VMs and other environments without GPU decode support.

## Docker

### docker compose (recommended)

```bash
docker compose run --rm gen-cert
docker compose up --build
```

### Manual

```bash
docker build -t phantom-screen -f server/Dockerfile .
docker run \
  -p 4443:4443/udp -p 4443:4443/tcp -p 4444:4444/tcp \
  -v ./certs:/certs:ro \
  phantom-screen \
  --cert=/certs/cert.pem --key=/certs/key.pem
```

## Configuration

```text
USAGE: phantom-screen-server [OPTIONS]

OPTIONS:
  --display <DISPLAY>          X11 display to capture [default: :99]
  --resolution <WxH>           Virtual display resolution [default: 1920x1080]
  --listen <ADDR:PORT>         Listen address [default: 0.0.0.0:4443]
  --fps <N>                    Video framerate [default: 60]
  --bitrate <KBPS>             H.264 bitrate in kbps [default: 6000]
  --keyframe-interval <N>      Keyframe interval in frames [default: 60]
  --cert <PATH>                TLS certificate PEM file
  --key <PATH>                 TLS private key PEM file
  --client-dir <PATH>          Web client files directory [default: ../client/dist/standalone]
  --no-xvfb                    Skip starting Xvfb
  --wm <COMMAND>               Window manager command [default: openbox]
  --jwt-secret <SECRET>        JWT secret for auth (env: PHANTOM_JWT_SECRET)
```

## Browser support

| Browser | Status |
|---------|--------|
| Chrome 97+ | Supported |
| Edge 97+ | Supported |
| Firefox 130+ | Experimental |
| Safari | Pending |
