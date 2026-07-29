# syntax=docker/dockerfile:1
# reviewer — multi-stage Docker build
#
# Stage 1: Build a statically-linked binary via musl (no glibc dependency).
# Stage 2: Copy into distroless/static (~20 MB final image).
#
# See ADR-018 for rationale on static linking + distroless.
#
# Multi-arch support:
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     --tag ghcr.io/devstroop/reviewer:latest .

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:slim-bookworm AS builder

ARG TARGETPLATFORM

RUN apt-get update && apt-get install -y \
    musl-tools \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Map Docker platform to Rust target triple (musl for fully static binary).
RUN case "$TARGETPLATFORM" in \
      "linux/amd64")  RUST_TARGET="x86_64-unknown-linux-musl" ;; \
      "linux/arm64")  RUST_TARGET="aarch64-unknown-linux-musl" ;; \
      *) echo "Unsupported platform: $TARGETPLATFORM"; exit 1 ;; \
    esac && \
    rustup target add "$RUST_TARGET" && \
    echo "$RUST_TARGET" > /tmp/rust-target

WORKDIR /app

# Cache dependencies by copying manifests first.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src/ prompts/ && \
    echo "fn main() {}" > src/main.rs && \
    touch src/lib.rs && \
    cargo build --release --target "$(cat /tmp/rust-target)" 2>/dev/null || true
RUN rm -rf src/ prompts/

# Now copy the real source and rebuild (layer cache is warm).
COPY src/ src/
COPY prompts/ prompts/

RUN cargo build --release --target "$(cat /tmp/rust-target)" && \
    cp target/"$(cat /tmp/rust-target)"/release/reviewer /reviewer

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM gcr.io/distroless/static:nonroot

COPY --from=builder /reviewer /reviewer

USER nonroot:nonroot
ENTRYPOINT ["/reviewer"]
