# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.99.1
ARG ALPINE_VERSION=3.24

FROM oven/bun:1-alpine AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS server
ARG RUST_VERSION
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}
RUN apk add --no-cache build-base cmake perl linux-headers
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY vendor ./vendor
COPY --from=web /src/web/build ./web/build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p rdc-server \
    && install -D target/release/rd-client /out/rd-client

FROM alpine:${ALPINE_VERSION}
RUN apk add --no-cache ca-certificates su-exec tzdata \
    && mkdir -p /data/db /data/downloads
COPY --from=server /out/rd-client /app/rd-client
COPY docker/entrypoint.sh /entrypoint.sh
ENV DATABASE_PATH=/data/db/rd-client.db \
    PORT=6500 \
    PUID=1000 \
    PGID=1000
EXPOSE 6500
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 CMD ["/app/rd-client", "healthcheck"]
ENTRYPOINT ["/entrypoint.sh"]
