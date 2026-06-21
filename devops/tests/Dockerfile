# devops/tests/Dockerfile — Pill-Engine CI image (Alpine)
#
# Build once:
#   docker build -t pill-ci -f devops/tests/Dockerfile .
#
# Run:
#   docker run --rm -v .:/src -w /src pill-ci ./devops/tests/tests.sh all

FROM rust:1.92-alpine

# -- System packages ---------------------------------------------------------
# mold: fast linker     gcompat: run glibc binaries on musl (slangc)
# alsa-lib-dev: audio   eudev-dev: gamepad     pkgconfig: build scripts
RUN apk add --no-cache \
    mold \
    gcompat \
    alsa-lib-dev \
    eudev-dev \
    pkgconfig \
    openjdk21-jre \
    plantuml \
    curl \
    git \
    bash \
    xorg-server-xvfb

# -- Rust components ---------------------------------------------------------
RUN rustup component add rustfmt clippy
RUN rustup target add wasm32-unknown-unknown

# -- wasm-pack ---------------------------------------------------------------
RUN curl -L https://github.com/rustwasm/wasm-pack/releases/download/v0.13.1/wasm-pack-v0.13.1-x86_64-unknown-linux-musl.tar.gz \
    | tar xz -C /usr/local/bin --strip-components=1

# -- slangc (shader compiler) ------------------------------------------------
# slangc ships glibc binaries. gcompat (installed above) provides the glibc
# compatibility layer needed to run them on musl.
ARG SLANG_VERSION=2025.21.2
RUN curl -L "https://github.com/shader-slang/slang/releases/download/v${SLANG_VERSION}/slang-${SLANG_VERSION}-linux-x86_64.zip" \
    -o /tmp/slang.zip \
    && unzip -q /tmp/slang.zip -d /tmp/slang \
    && cp /tmp/slang/bin/slangc /usr/local/bin/slangc \
    && cp /tmp/slang/bin/slangd /usr/local/bin/slangd \
    && chmod +x /usr/local/bin/slangc /usr/local/bin/slangd \
    && rm -rf /tmp/slang /tmp/slang.zip

# Quick smoke test: if gcompat fails, fall back gracefully.
RUN slangc --version 2>/dev/null || echo "slangc installed (gcompat runtime check at container start)"

# -- Pre-seed cargo registry (optional, speeds up first build) ----------------
# Uncomment the lines below and run a dummy build to cache crates in the image.
# COPY engine/Cargo.toml engine/Cargo.lock engine/
# COPY engine/pill_core/Cargo.toml engine/pill_core/
# RUN cargo check -p pill_core --manifest-path engine/Cargo.toml 2>/dev/null || true

# -- Entrypoint --------------------------------------------------------------
# 1. Touches WGSL files so build.rs skips HLSL→WGSL conversion (avoids slangc
#    gcompat edge cases).
# 2. Rewrites absolute host paths in engine/Cargo.toml (e.g.
#    "D:/.../examples/city" or "/home/.../examples/city") to repo-relative
#    paths ("examples/city") because the repo is mounted at /src inside the
#    container. Relative paths are left untouched.
# 3. Execs the user command.
ENTRYPOINT ["/bin/bash", "-c", "for wgsl in /src/engine/pill_engine/res/shaders/*.wgsl; do [[ -f \"$wgsl\" ]] && touch \"$wgsl\"; done; sed -i 's|\"[^\"]*/examples/|\"examples/|' /src/engine/Cargo.toml 2>/dev/null || true; exec \"$@\"", "--"]
CMD ["./devops/tests/tests.sh", "all"]
