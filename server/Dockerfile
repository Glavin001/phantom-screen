FROM debian:trixie-slim AS base

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb \
    openbox \
    gstreamer1.0-tools \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-x \
    xdotool \
    xclip \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# --- Build stage for server ---
FROM rust:1.88-slim-bookworm AS builder

# Install GStreamer dev packages for compilation
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libx11-dev \
    libxcb1-dev \
    libxcb-xtest0-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY server/ ./

RUN cargo build --release

# --- Build stage for client ---
FROM node:22-slim AS client-builder

WORKDIR /build
COPY client/package.json client/package-lock.json* ./
RUN npm install

COPY client/ ./
RUN npm run build

# --- Final image ---
FROM base

# Copy server binary
COPY --from=builder /build/target/release/phantom-screen-server /usr/local/bin/phantom-screen-server

# Copy client dist
COPY --from=client-builder /build/dist /var/www/phantom-screen/

EXPOSE 4443/udp
EXPOSE 4443/tcp
EXPOSE 4444/tcp

ENV DISPLAY=:99
ENV RUST_LOG=phantom_screen_server=info

ENTRYPOINT ["phantom-screen-server"]
CMD ["--display=:99", "--resolution=1920x1080", "--client-dir=/var/www/phantom-screen"]
