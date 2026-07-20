# syntax=docker/dockerfile:1.12

FROM node:26-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.97-alpine3.24 AS songrec
RUN apk add --no-cache alsa-lib-dev
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/songrec,sharing=locked \
    RUSTFLAGS="-C target-feature=-crt-static" \
    CARGO_TARGET_DIR=/build/songrec \
    cargo install songrec-lib --version 0.5.3 --locked --root /opt/songrec

FROM rust:1.97-alpine3.24 AS chef
WORKDIR /build
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo install cargo-chef --version 0.1.77 --locked

# Generate a dependency-only build recipe. Changes to application source rerun
# this inexpensive step, but leave the cooked dependency layer reusable.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY migrations/ ./migrations/
COPY src/ ./src/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS backend
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --locked --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY migrations/ ./migrations/
COPY src/ ./src/
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --locked --release && \
    cp target/release/ununknown /tmp/ununknown

FROM alpine:3.24 AS runtime
ARG VERSION=dev
ARG VCS_REF=unknown
ARG SOURCE_URL=https://github.com/artamrj/ununknown
LABEL org.opencontainers.image.title="Ununknown" \
      org.opencontainers.image.description="Local-first music metadata correction" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.source="${SOURCE_URL}" \
      org.opencontainers.image.licenses="MIT"

RUN apk add --no-cache alsa-lib ca-certificates chromaprint ffmpeg su-exec tini && \
    command -v ffmpeg && \
    command -v ffprobe && \
    command -v fpcalc && \
    command -v su-exec && \
    command -v tini && \
    addgroup -S -g 10001 ununknown && \
    adduser -S -D -H -u 10001 -G ununknown ununknown && \
    mkdir -p /data/cache /data/input /data/output /data/reference /usr/share/ununknown && \
    chown -R 10001:10001 /data /usr/share/ununknown

COPY --from=backend --chown=10001:10001 /tmp/ununknown /usr/local/bin/ununknown
COPY --from=songrec --chown=10001:10001 /opt/songrec/bin/songrec-lib-cli /usr/local/bin/songrec-lib-cli
COPY --from=frontend --chown=10001:10001 /build/frontend/dist/ /usr/share/ununknown/
COPY --chmod=755 scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN command -v songrec-lib-cli && \
    sh -n /usr/local/bin/docker-entrypoint.sh

ENV PUID=10001 \
    PGID=10001 \
    HOME=/tmp \
    UNUNKNOWN_BIND=0.0.0.0:7331 \
    UNUNKNOWN_ALLOW_NON_LOOPBACK=true \
    UNUNKNOWN_DB=/data/cache/ununknown.sqlite \
    UNUNKNOWN_INPUT_DIR=/data/input \
    UNUNKNOWN_INPUT_MODE=auto \
    UNUNKNOWN_OUTPUT_DIR=/data/output \
    UNUNKNOWN_STATIC_DIR=/usr/share/ununknown \
    UNUNKNOWN_SONGREC_BIN=/usr/local/bin/songrec-lib-cli \
    RUST_LOG=info

EXPOSE 7331
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["wget", "-q", "-O", "/dev/null", "http://127.0.0.1:7331/api/health"]
ENTRYPOINT ["/sbin/tini", "--", "/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/ununknown"]
