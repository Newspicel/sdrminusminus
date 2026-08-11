# syntax=docker/dockerfile:1

# sdr-- multi-arch image (PLAN §15: Pi/NAS deployment, `--device /dev/bus/usb`).
#
# Self-contained by construction: the `soapy` feature is OFF (soapysdr-sys pkg-configs
# libSoapySDR and `links`-declares it, which would make a shared library a launch dependency)
# and the pure-Rust RTL-SDR/HackRF backends are ON, so the runtime image needs no SDR package.
#
#   docker buildx build --platform linux/amd64,linux/arm64 -t sdrmm .
#
# There is no cross-compilation here: buildx builds each platform on a node of that platform
# (native runners in CI, QEMU locally).


# --- web UI ------------------------------------------------------------------------------
FROM node:26-slim AS web
WORKDIR /web
RUN npm install -g pnpm@11.15.1

# Manifests first: the install layer then survives every UI source edit.
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile

# .dockerignore keeps web/node_modules and web/dist out of the context; without those two
# entries this copy would clobber the install above.
COPY web/ ./
RUN pnpm build
# crates/server/build.rs creates web/dist when it is missing, so a Rust build with no UI
# succeeds and silently ships the "not built" 503 page. Fail here, where the cause is local.
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
# cargo fails loudly with "no targets specified in the manifest".
RUN find crates apps xtask -type f ! -name Cargo.toml -delete \
    && find crates apps xtask -mindepth 1 -type d -empty -delete \
    && for dir in crates/*/; do mkdir -p "$dir/src" && : > "$dir/src/lib.rs"; done \
    && for dir in apps/*/ xtask/; do mkdir -p "$dir/src" && echo 'fn main() {}' > "$dir/src/main.rs"; done


# --- server binary -----------------------------------------------------------------------
FROM debian:trixie-slim AS builder
# cc + cmake: audiopus_sys builds vendored libopus through the cmake crate, libsqlite3-sys is
# bundled C. Nothing else is needed with `soapy` off — no pkg-config, and no libusb because
# nusb talks to usbfs directly.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake ca-certificates curl \
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
RUN rustup show

ARG FEATURES=rtl-native,hackrf-native,net-client
# Dependency compilation against the stubs: invalidated only by Cargo.lock or a manifest, never
# by a source edit. The stubs reference nothing, so each workspace crate compiles empty while
# cargo still builds every external dependency it declares.
RUN cargo build --release --locked -p sdrmm --no-default-features --features "$FEATURES"

COPY crates crates
COPY apps apps
COPY xtask xtask
COPY --from=web /web/dist web/dist
# The touch is load-bearing: cargo decides freshness by mtime, and context files older than the
# stub rlibs built above would leave those empty stubs in the shipped binary.
# rust-embed only bakes bytes into the binary for non-debug profiles — hence --release.
RUN test -f web/dist/index.html \
    && find crates apps xtask -name '*.rs' -exec touch {} + \
    && cargo build --release --locked -p sdrmm --no-default-features --features "$FEATURES" \
    && install -Dm755 target/release/sdrmm /out/sdrmm


# --- runtime -----------------------------------------------------------------------------
FROM debian:trixie-slim AS runtime
LABEL org.opencontainers.image.source="https://github.com/newspicel/sdrminusminus" \
      org.opencontainers.image.description="sdr-- — headless SDR server with embedded web UI" \
      org.opencontainers.image.licenses="MIT"

# curl is here only for HEALTHCHECK; ca-certificates so outbound TLS works if anything grows it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --create-home --home-dir /home/sdrmm sdrmm

COPY --from=builder /out/sdrmm /usr/local/bin/sdrmm

# Docker seeds a fresh named or anonymous volume from the image path, ownership included, so
# /data has to belong to the unprivileged user *here* for it to be writable there.
RUN install -d -o sdrmm -g sdrmm /data
VOLUME ["/data"]

# USB access is the one thing the image cannot grant this user: /dev/bus/usb nodes are
# root-owned unless the *host* udev rules relax them (the stock rtl-sdr and hackrf rules ship
# MODE="0666"/GROUP="plugdev", in which case this works as-is). Where they do not, run with
# `--group-add <gid owning /dev/bus/usb/*>` or `--user root`. PLAN §15 keeps OS USB permissions
# explicitly out of scope rather than pretending static linking can fix them.
USER sdrmm

EXPOSE 8080
# `/` is the SPA fallback, which auth::require_token is deliberately not layered over, so this
# keeps working when --token is set. It serves 503 until the UI is embedded, so an image built
# without web assets never reports healthy either.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS -o /dev/null http://127.0.0.1:8080/ || exit 1

# The data paths belong in ENTRYPOINT, not CMD: `docker run <image> --bind …` replaces CMD
# wholesale, and the binary's own defaults are dirs::data_dir()-based — /home/sdrmm/.local/share
# inside a container, outside the volume, so every run would come up with an empty database.
ENTRYPOINT ["/usr/local/bin/sdrmm", "--db", "/data/sdrmm.db", "--recordings-dir", "/data/recordings"]
CMD ["--bind", "0.0.0.0:8080"]
