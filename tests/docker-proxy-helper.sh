#!/usr/bin/env bash
# Helper for building Docker images behind an HTTPS-only egress proxy.
#
# When HTTP_PROXY is set, Docker build containers cannot reach Debian/Ubuntu
# package repos over plain HTTP (proxy blocks it). This helper patches the
# Dockerfile to use HTTPS sources with peer verification disabled and injects
# a proxy CA cert so cargo/npm/curl work.
#
# Usage: source this file, then call docker_build_proxied <dockerfile> [docker build args...]

docker_build_proxied() {
  local dockerfile="$1"
  shift
  # remaining args forwarded to docker build

  if [ -n "${HTTP_PROXY:-}" ]; then
    local tmpdir
    tmpdir=$(mktemp -d)
    local tmpfile="$tmpdir/Dockerfile"

    # Find a proxy CA cert if available
    local ca_cert=""
    for cert in /usr/local/share/ca-certificates/*.crt; do
      [ -f "$cert" ] && ca_cert="$cert" && break
    done

    if [ -n "$ca_cert" ]; then
      # Determine the build context (last positional arg)
      local context_dir="${*: -1}"
      cp "$ca_cert" "$context_dir/proxy-ca.crt"

      # After each FROM line, inject CA cert install + HTTPS apt sources
      python3 -c "
import sys
inject = '''COPY proxy-ca.crt /usr/local/share/ca-certificates/proxy-ca.crt
RUN (update-ca-certificates 2>/dev/null || (cat /usr/local/share/ca-certificates/proxy-ca.crt >> /etc/ssl/certs/ca-certificates.crt 2>/dev/null)) ; \\\\
    sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/*.sources 2>/dev/null; \\\\
    sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list 2>/dev/null; \\\\
    echo 'Acquire::https::Verify-Peer \"false\";' > /etc/apt/apt.conf.d/99proxy-no-verify 2>/dev/null; true
ENV NODE_EXTRA_CA_CERTS=/usr/local/share/ca-certificates/proxy-ca.crt
ENV CARGO_HTTP_CAINFO=/etc/ssl/certs/ca-certificates.crt'''
with open(sys.argv[1]) as f:
    for line in f:
        sys.stdout.write(line)
        if line.startswith('FROM '):
            sys.stdout.write(inject + '\n')
" "$dockerfile" > "$tmpfile"

      docker build \
        --build-arg "http_proxy=$HTTP_PROXY" \
        --build-arg "https_proxy=${HTTPS_PROXY:-}" \
        --build-arg "HTTP_PROXY=$HTTP_PROXY" \
        --build-arg "HTTPS_PROXY=${HTTPS_PROXY:-}" \
        -f "$tmpfile" "$@"
      local rc=$?
      rm -f "$context_dir/proxy-ca.crt"
      rm -rf "$tmpdir"
      return $rc
    else
      # No CA cert — just disable apt peer verification
      python3 -c "
import sys
inject = '''RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/*.sources 2>/dev/null; \\\\
    sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list 2>/dev/null; \\\\
    echo 'Acquire::https::Verify-Peer \"false\";' > /etc/apt/apt.conf.d/99proxy-no-verify; true'''
with open(sys.argv[1]) as f:
    for line in f:
        if 'apt-get update' in line:
            sys.stdout.write(inject + '\n')
        sys.stdout.write(line)
" "$dockerfile" > "$tmpfile"

      docker build \
        --build-arg "http_proxy=$HTTP_PROXY" \
        --build-arg "https_proxy=${HTTPS_PROXY:-}" \
        --build-arg "HTTP_PROXY=$HTTP_PROXY" \
        --build-arg "HTTPS_PROXY=${HTTPS_PROXY:-}" \
        -f "$tmpfile" "$@"
      local rc=$?
      rm -rf "$tmpdir"
      return $rc
    fi
  else
    docker build -f "$dockerfile" "$@"
  fi
}
