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


# --- pinned Soapy runtime ---------------------------------------------------------------
FROM mambaorg/micromamba:2.9.0 AS soapy
ARG TARGETARCH
COPY --chown=$MAMBA_USER:$MAMBA_USER packaging/soapy/conda-linux-64.lock /tmp/conda-linux-64.lock
COPY --chown=$MAMBA_USER:$MAMBA_USER packaging/soapy/conda-linux-aarch64.lock /tmp/conda-linux-aarch64.lock
COPY --chown=$MAMBA_USER:$MAMBA_USER packaging/soapy/licenses /opt/conda/share/licenses/sdrmm-soapy
# The explicit per-platform locks pin every transitive package URL and checksum.
RUN case "$TARGETARCH" in \
      amd64) lock=/tmp/conda-linux-64.lock ;; \
      arm64) lock=/tmp/conda-linux-aarch64.lock ;; \
      *) echo "unsupported Docker architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && micromamba install --yes --name base --file "$lock" \
    && micromamba clean --all --yes \
    && test -f /opt/conda/lib/libSoapySDR.so \
    && test -n "$(find /opt/conda/lib/SoapySDR/modules0.8 -iname '*rtlsdr*' -print -quit)" \
    && test -n "$(find /opt/conda/lib/SoapySDR/modules0.8 -iname '*hackrf*' -print -quit)" \
    && test -f /opt/conda/share/licenses/sdrmm-soapy/HackRF-GPL-2.0-or-later.txt \
    && for module in airspy blade lms7 pluto remote; do \
         test -n "$(find /opt/conda/lib/SoapySDR/modules0.8 -iname "*$module*" -print -quit)"; \
       done


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
# cc + cmake: audiopus_sys builds vendored libopus through the cmake crate. The private Soapy
# environment supplies the core library used to link the canonical hardware backend.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       build-essential cmake ca-certificates curl pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY --from=soapy /opt/conda /opt/conda

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    SOAPY_SDR_ROOT=/opt/conda \
    PKG_CONFIG_PATH=/opt/conda/lib/pkgconfig \
    LD_LIBRARY_PATH=/opt/conda/lib \
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

ARG FEATURES=soapy,sdrplay,net-client,gpu-fft
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

# The pinned private environment includes the core, curated modules, transitive shared libraries,
# package metadata, and licenses. UHD remains an optional pack because of its size.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --create-home --home-dir /home/sdrmm sdrmm

COPY --from=soapy /opt/conda /opt/conda
COPY --from=builder /out/sdrmm /usr/local/bin/sdrmm
COPY THIRD_PARTY_NOTICES.md /usr/share/doc/sdrmm/THIRD_PARTY_NOTICES.md

ENV SOAPY_SDR_ROOT=/opt/conda \
    SOAPY_SDR_PLUGIN_PATH=/opt/conda/lib/SoapySDR/modules0.8 \
    LD_LIBRARY_PATH=/opt/conda/lib \
    PATH=/opt/conda/bin:$PATH

# Docker seeds a fresh named or anonymous volume from the image path, ownership included, so
# /data has to belong to the unprivileged user *here* for it to be writable there.
RUN install -d -o sdrmm -g sdrmm /data
VOLUME ["/data"]

# USB access is the one thing the image cannot grant this user: /dev/bus/usb nodes are
# root-owned unless the *host* udev rules relax them (the stock rtl-sdr and hackrf rules ship
# MODE="0666"/GROUP="plugdev", in which case this works as-is). Where they do not, run with
# `--group-add <gid owning /dev/bus/usb/*>` or `--user root`. OS USB permissions remain a host
# concern; static linking cannot change device-node permissions.
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
