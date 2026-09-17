# --- web UI ------------------------------------------------------------------------------
FROM node:26-slim AS web
WORKDIR /web
RUN npm install -g pnpm@11.15.1

# Manifests first: the install layer then survives every UI source edit.
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile

COPY web/ ./
RUN pnpm build
RUN test -f dist/index.html


# --- workspace skeleton ------------------------------------------------------------------
FROM debian:trixie-slim AS planner
WORKDIR /plan
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo .cargo
COPY crates crates
COPY apps apps
COPY xtask xtask
# Reduce the workspace to manifests plus empty targets. This tree is the cache key of the
# dependency-compilation layer below, so it must not change when a source file changes.
# crates/* are all libraries, apps/* and xtask are all binaries; if that ever stops holding,
# cargo fails loudly with "no targets specified in the manifest". A declared [[bench]] must
# also exist for its manifest to parse; the stubs assume the default benches/<name>.rs path,
# so a bench that sets `path =` fails loudly here too.
RUN find crates apps xtask -type f ! -name Cargo.toml -delete \
    && find crates apps xtask -mindepth 1 -type d -empty -delete \
    && for dir in crates/*/; do mkdir -p "$dir/src" && : > "$dir/src/lib.rs"; done \
    && for dir in apps/*/ xtask/; do mkdir -p "$dir/src" && echo 'fn main() {}' > "$dir/src/main.rs"; done \
    && for m in crates/*/Cargo.toml apps/*/Cargo.toml xtask/Cargo.toml; do \
         grep -A2 '^\[\[bench\]\]' "$m" | sed -n 's/^name *= *"\([^"]*\)".*/\1/p' \
         | while read -r b; do \
             mkdir -p "$(dirname "$m")/benches" \
             && echo 'fn main() {}' > "$(dirname "$m")/benches/$b.rs"; \
           done; \
       done


# --- server binary -----------------------------------------------------------------------
FROM debian:trixie-slim AS builder
# cc + cmake: opusic-sys builds vendored libopus through the cmake crate. Nothing here needs
# SoapySDR: the backend opens it at runtime and links nothing at build time.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       build-essential cmake ca-certificates curl pkg-config python3 clang libclang-dev nasm \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
# `--default-toolchain none` so rust-toolchain.toml is the only thing choosing the compiler.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --default-toolchain none

# Everything below runs from the repo root so .cargo/config.toml applies: it carries
# `-Zpolonius=next` and CMAKE_POLICY_VERSION_MINIMUM. Never set a RUSTFLAGS env var here — the
# variable replaces `[build] rustflags` wholesale and would silently drop polonius.
WORKDIR /src
COPY --from=planner /plan/ ./
COPY scripts/build-media.py scripts/build-media.py
RUN python3 scripts/build-media.py --prefix /opt/sdrmm-media
ENV FFMPEG_DIR=/opt/sdrmm-media
RUN rustup show

ARG FEATURES=soapy,sdrplay,rtlsdr,hackrf,airspy,airspyhf,ad936x,net-client,gpu-fft
# `ci` (Cargo.toml) drops LTO to answer a broken Dockerfile faster on a pull request. Releases
# must never pass this — the published image is built from the default.
ARG PROFILE=release
# Dependency compilation against the stubs: invalidated only by Cargo.lock or a manifest, never
# by a source edit. The stubs reference nothing, so each workspace crate compiles empty while
# cargo still builds every external dependency it declares.
RUN cargo build --profile "$PROFILE" --locked -p sdrmm --no-default-features --features "$FEATURES"

COPY crates crates
COPY apps apps
COPY xtask xtask
COPY fixtures/broadcast_audio fixtures/broadcast_audio
COPY --from=web /web/dist web/dist
# The touch is load-bearing: cargo decides freshness by mtime, and context files older than the
# stub rlibs built above would leave those empty stubs in the shipped binary.
# rust-embed only bakes bytes into the binary when debug assertions are off, which every profile
# used here inherits from `release`.
RUN test -f web/dist/index.html \
    && find crates apps xtask -name '*.rs' -exec touch {} + \
    && cargo build --profile "$PROFILE" --locked -p sdrmm --no-default-features --features "$FEATURES" \
    && install -Dm755 "target/$PROFILE/sdrmm" /out/sdrmm


# --- runtime -----------------------------------------------------------------------------
FROM debian:trixie-slim AS runtime
LABEL org.opencontainers.image.source="https://github.com/newspicel/sdrminusminus" \
      org.opencontainers.image.description="SDR-- — headless SDR server with embedded web UI" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"

# SoapySDR comes from Debian, as it would on the host: the modules named here are the ones no
# native backend in this build covers. Modules are listed one by one rather than through
# soapysdr-module-all, which pulls in SoapyUHD — it aborts the process when it loads headless.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl \
       libsoapysdr0.8 \
       soapysdr-module-bladerf \
       soapysdr-module-lms7 \
       soapysdr-module-remote \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --create-home --home-dir /home/sdrmm sdrmm

COPY --from=builder /out/sdrmm /usr/local/bin/sdrmm
COPY THIRD_PARTY_NOTICES.md /usr/share/doc/sdrmm/THIRD_PARTY_NOTICES.md

# Docker seeds a fresh named or anonymous volume from the image path, ownership included, so
# /data has to belong to the unprivileged user *here* for it to be writable there.
RUN install -d -o sdrmm -g sdrmm /data
VOLUME ["/data"]

# USB access is the one thing the image cannot grant this user: /dev/bus/usb nodes are
# root-owned and mode 0664 by default, and the vendor udev rules hand them to a group
# (plugdev, gid 46 on Debian) rather than to the world, so passing the bus is never enough on
# its own. Run with `--group-add <gid owning /dev/bus/usb/*>`, or `--user root` as a last
# resort. OS USB permissions remain a host concern; static linking cannot change device-node
# permissions.
USER sdrmm

EXPOSE 8080
# `/` is the SPA fallback, which auth::require_token is deliberately not layered over, so this
# keeps working when --token is set. It serves 503 until the UI is embedded, so an image built
# without web assets never reports healthy either. The HTTPS retry covers --tls-*: `-k` because
# the certificate is the operator's business and this probe only asks whether the app answers.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fs -o /dev/null http://127.0.0.1:8080/ \
        || curl -fsSk -o /dev/null https://127.0.0.1:8080/ \
        || exit 1

# The data paths belong in ENTRYPOINT, not CMD: `docker run <image> --bind …` replaces CMD
# wholesale, and the binary's own defaults are dirs::data_dir()-based — /home/sdrmm/.local/share
# inside a container, outside the volume, so every run would come up with an empty database.
ENTRYPOINT ["/usr/local/bin/sdrmm", "--db", "/data/sdrmm.db", "--recordings-dir", "/data/recordings"]
CMD ["--bind", "0.0.0.0:8080"]
