# syntax=docker/dockerfile:1
#
# chess-server (NET-03) as a container. Build from the repository root:
#
#   docker build -f server/Dockerfile -t chess-server .
#
# The data directory is the /data volume; it must be a local disk, not NFS
# or SMB (databases rely on file locks and fsync). See docs/server.md.

# ---- the admin web UI -----------------------------------------------------
FROM node:24-trixie-slim AS admin
WORKDIR /src/server/admin
COPY server/admin/package.json server/admin/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY server/admin/ ./
RUN npm run build

# ---- the server, with the UI embedded ------------------------------------
FROM rust:1.94-trixie AS build
# Parallel rustc jobs: each can take a gigabyte or two.
ARG CARGO_BUILD_JOBS=4
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
WORKDIR /src
# Cargo loads every workspace member's manifest, the desktop app's too
# (it isn't built). Server/Dockerfile.dockerignore keeps the rest out.
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates crates
COPY app/src-tauri app/src-tauri
COPY server server
COPY --from=admin /src/server/admin/dist server/admin/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p chess-server \
    && cp target/release/chess-server /usr/local/bin/chess-server

# ---- what runs ------------------------------------------------------------
FROM debian:trixie-slim
# The certificates TLS checks servers against: e-mail (SMTP) and TWIC downloads.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --system --gid 10001 chess \
    && useradd --system --uid 10001 --gid chess --home-dir /data --no-create-home chess \
    && mkdir /data /backups && chown chess:chess /data /backups
COPY --from=build /usr/local/bin/chess-server /usr/local/bin/chess-server
USER chess
ENV CHESS_SERVER_DATA=/data \
    CHESS_SERVER_BIND=0.0.0.0:8080 \
    RUST_LOG=info
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["chess-server", "healthcheck"]
ENTRYPOINT ["chess-server"]
CMD ["serve"]
