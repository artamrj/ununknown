# syntax=docker/dockerfile:1.12

FROM node:24-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.96-alpine3.24 AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY migrations/ ./migrations/
COPY src/ ./src/
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target \
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

RUN apk add --no-cache ca-certificates chromaprint ffmpeg tini && \
    addgroup -S -g 10001 ununknown && \
    adduser -S -D -H -u 10001 -G ununknown ununknown && \
    mkdir -p /data/cache /data/input /data/output /usr/share/ununknown && \
    chown -R 10001:10001 /data /usr/share/ununknown

COPY --from=backend --chown=10001:10001 /tmp/ununknown /usr/local/bin/ununknown
COPY --from=frontend --chown=10001:10001 /build/frontend/dist/ /usr/share/ununknown/

ENV UNUNKNOWN_BIND=0.0.0.0:7331 \
    UNUNKNOWN_ALLOW_NON_LOOPBACK=true \
    UNUNKNOWN_DB=/data/cache/ununknown.sqlite \
    UNUNKNOWN_INPUT_DIR=/data/input \
    UNUNKNOWN_OUTPUT_DIR=/data/output \
    UNUNKNOWN_STATIC_DIR=/usr/share/ununknown \
    RUST_LOG=info

USER 10001:10001
EXPOSE 7331
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["wget", "-q", "-O", "/dev/null", "http://127.0.0.1:7331/api/health"]
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/ununknown"]
