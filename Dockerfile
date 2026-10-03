# syntax=docker/dockerfile:1
# HSMP dedicated server image: hsmp-server (+ optional hsmp-master), built from the Cargo
# workspace. No game files are needed: the dedicated server is pure Rust UDP + HTTP.
#
# Build (from the repository root):
#   docker build -t hsmp-server .
# Run (game server only; state, identity key and bans persist in the named volume):
#   docker run -d --name hsmp -p 7777:7777/udp -v hsmp-data:/hsmp/data hsmp-server
# Run with a local master server (server list) on TCP 7778:
#   docker run -d --name hsmp -p 7777:7777/udp -p 7778:7778 -e HSMP_WITH_MASTER=1 \
#     -v hsmp-data:/hsmp/data hsmp-server
# RCON stays off unless HSMP_RCON_BIND is set. It is plain text: publish it on the host's
# loopback only and reach it over SSH (see docs/hosting/rcon.md):
#   docker run ... -p 127.0.0.1:2345:2345 -e HSMP_RCON_BIND=0.0.0.0:2345 \
#     -e HSMP_RCON_ALLOW_REMOTE=1 -e HSMP_RCON_PASSWORD=<at least 16 random characters> hsmp-server

# Keep in step with rust-toolchain.toml (not copied in: the image already has this toolchain).
ARG RUST_VERSION=1.98.1

FROM rust:${RUST_VERSION}-slim-bookworm AS build
WORKDIR /src
# The whole workspace manifest set is needed to resolve the lockfile; only the server and
# its library are compiled.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY server ./server
COPY launcher/Cargo.toml launcher/Cargo.toml
COPY tools/hsmp-tools/Cargo.toml tools/hsmp-tools/Cargo.toml
COPY tools/release/Cargo.toml tools/release/Cargo.toml
COPY tools/pose-truth/Cargo.toml tools/pose-truth/Cargo.toml
COPY tests ./tests
# Stub targets for the members that are not built here, so cargo can load the workspace.
RUN for d in launcher tools/hsmp-tools tools/release tools/pose-truth; do \
        mkdir -p "$d/src" && echo 'fn main() {}' > "$d/src/main.rs" && touch "$d/src/lib.rs"; \
    done
RUN cargo build --release --locked -p hsmp-server --bin hsmp-server --bin hsmp-master \
 && cp target/release/hsmp-server target/release/hsmp-master /usr/local/bin/

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home /hsmp --uid 1000 hsmp \
 && mkdir -p /hsmp/data && chown -R hsmp:hsmp /hsmp
COPY --from=build /usr/local/bin/hsmp-server /usr/local/bin/hsmp-server
COPY --from=build /usr/local/bin/hsmp-master /usr/local/bin/hsmp-master
COPY --chmod=0755 scripts/docker-entry.sh /hsmp/docker-entry.sh
COPY LICENSE-MIT LICENSE-APACHE NOTICE /usr/share/doc/hsmp/
# Strip CRLF in case the build context came from a Windows checkout without .gitattributes.
RUN sed -i 's/\r$//' /hsmp/docker-entry.sh
WORKDIR /hsmp
USER hsmp
ENV HSMP_BIND=0.0.0.0:7777 \
    HSMP_MAX_PEERS=8 \
    HSMP_NAME="HSMP Dedicated" \
    HSMP_MODE=duel \
    HSMP_STATE_DIR=/hsmp/data \
    HSMP_BANS_FILE=/hsmp/data/bans.txt \
    HSMP_ADMINS_FILE=/hsmp/data/admins.txt \
    HSMP_WITH_MASTER=0 \
    HSMP_MASTER_BIND=0.0.0.0:7778 \
    RUST_LOG=hsmp_server=info,hsmp_master=info
EXPOSE 7777/udp 7778/tcp
# hsmp-server shuts down cleanly (telling players) on SIGINT; SIGTERM is not handled yet.
STOPSIGNAL SIGINT
VOLUME /hsmp/data
ENTRYPOINT ["/usr/bin/tini", "-g", "--", "/hsmp/docker-entry.sh"]
