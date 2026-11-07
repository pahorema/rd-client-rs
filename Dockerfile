# syntax=docker/dockerfile:1.7

# Base images are pinned by digest; Dependabot updates tags and digests together.

FROM oven/bun:1-alpine@sha256:4cf82f5686b196504fe119db3fcd9185b191f82ece82c40680e597228ec6070a AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

# Keep the Rust version in step with rust-toolchain.toml.
FROM rust:1.99.1-alpine3.24@sha256:44819b27be4c97b0e4d7203bd2e02c4820f97700211b6a37fb4c246a2de59383 AS server
# Use the image's toolchain (its RUST_VERSION) instead of installing the channel again.
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

FROM scratch AS binary
COPY --from=server /out/rd-client /rd-client

FROM alpine:3.24@sha256:3f4d6b32e26703c6082839234f43769d5046b3ac7c910033911d1e71d8cd630a
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
