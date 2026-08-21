# syntax=docker/dockerfile:1

# Three stages, in the order things change least often: assets, then Rust,
# then a final image with neither toolchain in it.

# --- Stage 1: the frontend bundle ------------------------------------------
FROM node:22-bookworm-slim AS assets
WORKDIR /build
# Copy the lockfile alone first so `npm ci` is cached until dependencies move.
COPY package.json package-lock.json* ./
RUN npm ci
# `svelte.config.js` only exists in the Svelte scaffold. A wildcard that
# matches nothing is fine as long as one source in the same COPY matched.
COPY tsconfig.json vite.config.ts svelte.config.js* ./
COPY resources ./resources
COPY public ./public
RUN npm run build

# --- Stage 2: the binary ---------------------------------------------------
FROM rust:1.97.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY .cargo ./.cargo
COPY src ./src
COPY app ./app
COPY bootstrap ./bootstrap
COPY config ./config
COPY database ./database
COPY routes ./routes
# The registry and the target directory are the two expensive things to
# rebuild; a cache mount keeps both across builds without baking them in.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked && \
    cp target/release/__RUST_NAME__ /build/__RUST_NAME__

# --- Stage 3: the runtime --------------------------------------------------
# Distroless: a libc, CA certificates, timezone data, and nothing else. No
# shell, so a compromised process has no interpreter to reach for.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
WORKDIR /app
COPY --from=build /build/__RUST_NAME__ /app/__RUST_NAME__
COPY --from=assets /build/public /app/public
COPY --chown=nonroot:nonroot storage /app/storage

ENV APP_ENV=production \
    APP_BIND=0.0.0.0 \
    APP_PORT=3000

EXPOSE 3000
USER nonroot
ENTRYPOINT ["/app/__RUST_NAME__"]
