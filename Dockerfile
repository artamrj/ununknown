FROM node:24-bookworm-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY migrations migrations
COPY src src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libchromaprint-tools \
 && fpcalc -version \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /app/target/release/ununknown /usr/local/bin/ununknown
COPY --from=frontend /app/frontend/dist frontend/dist
ENV RUST_LOG=info
EXPOSE 7331
CMD ["ununknown"]
