FROM node:22-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libchromaprint-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=backend /build/target/release/ununknown ./ununknown
COPY --from=frontend /build/frontend/dist ./frontend/dist

ENV UNUNKNOWN_DB=/cache/ununknown.sqlite \
    UNUNKNOWN_INPUT_DIR=/music/input \
    UNUNKNOWN_OUTPUT_DIR=/music/output

RUN mkdir -p /cache /music/input /music/output

EXPOSE 7331
CMD ["./ununknown"]
