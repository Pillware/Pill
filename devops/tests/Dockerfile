# devops/tests/Dockerfile - Pill CI image (Debian Bookworm)
#
# Multi-stage build: compiles PillLauncher once at image build time,
# so it's instantly available when the container starts.
#
# Build once:
#   docker build -t pill-ci -f devops/tests/Dockerfile .
#
# Run:
#   docker run --rm -v .:/src -w /src pill-ci bash devops/tests/run_basic_tests.sh all

# ---------------------------------------------------------------------------
# Stage 1: Build PillLauncher
# ---------------------------------------------------------------------------
FROM rust:1.92-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    mold clang pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Copy only the engine workspace (launcher + its dependencies).
COPY engine/ /src/engine/

# Fix any absolute host paths that leaked into Cargo.toml (e.g. from Windows).
# Also strip stale workspace members left by interrupted launcher builds.
RUN sed -i 's|"[^"]*/examples/|"examples/|' /src/engine/Cargo.toml 2>/dev/null || true \
    && sed -i '/pill-launcher-managed-workspace-member/d' /src/engine/Cargo.toml 2>/dev/null || true

RUN cargo build --release --manifest-path /src/engine/pill_launcher/Cargo.toml --target-dir /tmp/pill-launcher-build

# ---------------------------------------------------------------------------
# Stage 2: CI runtime image
# ---------------------------------------------------------------------------
FROM rust:1.92-slim-bookworm

# -- System packages ---------------------------------------------------------
# mold: fast linker     clang: C linker for cargo
# libasound2-dev: audio   libudev-dev: gamepad     pkg-config: build scripts
# plantuml: docs
# Headless benchmarking is handled via --headless (no xvfb needed).
RUN apt-get update && apt-get install -y --no-install-recommends \
    mold \
    clang \
    libasound2-dev \
    libudev-dev \
    pkg-config \
    plantuml \
    curl \
    git \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# -- Rust components ---------------------------------------------------------
RUN rustup component add rustfmt clippy
RUN rustup target add wasm32-unknown-unknown

# -- Git safe.directory ------------------------------------------------------
# GitHub Actions runs containers as root on a mounted workspace. Git 2.35+
# refuses to operate in repos owned by a different user unless the directory
# is explicitly marked safe.
RUN git config --global --add safe.directory '*'

# -- wasm-pack ---------------------------------------------------------------
RUN curl -L https://github.com/rustwasm/wasm-pack/releases/download/v0.13.1/wasm-pack-v0.13.1-x86_64-unknown-linux-musl.tar.gz \
    | tar xz -C /usr/local/bin --strip-components=1

# -- slangc (shader compiler) ------------------------------------------------
# Debian Bookworm is glibc-based, so slangc's glibc binaries work natively.
ARG SLANG_VERSION=2025.21.2
RUN curl -L "https://github.com/shader-slang/slang/releases/download/v${SLANG_VERSION}/slang-${SLANG_VERSION}-linux-x86_64.zip" \
    -o /tmp/slang.zip \
    && unzip -q /tmp/slang.zip -d /tmp/slang \
    && cp /tmp/slang/bin/slangc /usr/local/bin/slangc \
    && cp /tmp/slang/bin/slangd /usr/local/bin/slangd \
    && chmod +x /usr/local/bin/slangc /usr/local/bin/slangd \
    && cp /tmp/slang/lib/*.so* /usr/local/lib/ \
    && ldconfig \
    && rm -rf /tmp/slang /tmp/slang.zip

RUN slangc --version 2>/dev/null || echo "slangc installed (runtime check)"

# -- PillLauncher binary (pre-built in Stage 1) ------------------------------
COPY --from=builder /tmp/pill-launcher-build/release/PillLauncher /usr/local/bin/PillLauncher
ENV PILL_LAUNCHER_BIN=/usr/local/bin/PillLauncher

# -- Entrypoint --------------------------------------------------------------
# Runtime fixups on the mounted source volume. Works both with local Docker
# (mount at /src) and GitHub Actions (mount at GITHUB_WORKSPACE).
RUN echo '#!/bin/bash'                   > /usr/local/bin/entrypoint.sh \
 && echo 'set -euo pipefail'            >> /usr/local/bin/entrypoint.sh \
 && echo ''                             >> /usr/local/bin/entrypoint.sh \
 && echo '# Resolve the source root: use /src if mounted, else GITHUB_WORKSPACE, else PWD' >> /usr/local/bin/entrypoint.sh \
 && echo 'SRC_ROOT="${GITHUB_WORKSPACE:-/src}"' >> /usr/local/bin/entrypoint.sh \
 && echo '[ -d "$SRC_ROOT" ] || SRC_ROOT="$PWD"' >> /usr/local/bin/entrypoint.sh \
 && echo ''                             >> /usr/local/bin/entrypoint.sh \
 && echo '# Rewrite absolute host paths to repo-relative in engine/Cargo.toml' >> /usr/local/bin/entrypoint.sh \
 && echo 'sed -i "s|\"[^\"]*/examples/|\"examples/|" "$SRC_ROOT/engine/Cargo.toml" 2>/dev/null || true' >> /usr/local/bin/entrypoint.sh \
 && echo ''                             >> /usr/local/bin/entrypoint.sh \
 && echo '# Touch WGSL shaders so build.rs skips HLSL->WGSL conversion' >> /usr/local/bin/entrypoint.sh \
 && echo 'for wgsl in "$SRC_ROOT"/engine/pill_engine/res/shaders/*.wgsl; do' >> /usr/local/bin/entrypoint.sh \
 && echo '    [ -f "$wgsl" ] && touch "$wgsl"'                        >> /usr/local/bin/entrypoint.sh \
 && echo 'done'                                                        >> /usr/local/bin/entrypoint.sh \
 && echo ''                                                             >> /usr/local/bin/entrypoint.sh \
 && echo 'exec "$@"'                                                   >> /usr/local/bin/entrypoint.sh \
 && chmod +x /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["bash", "devops/tests/run_basic_tests.sh", "all"]
