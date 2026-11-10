# Builds hbbs (rendezvous/ID server) and hbbr (relay) from the pinned rustdesk-server source.
# Build from the repo root:  docker build -f server/Dockerfile -t tether-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY vendor/rustdesk-server/ .
# The patched turn crate ([patch.crates-io] in Cargo.toml).
COPY server/patched/ patched/
RUN cargo build --release --bin hbbs --bin hbbr

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/hbbs /src/target/release/hbbr /usr/local/bin/
# Not root (as Dockerfile.static and the Helm chart); the data dir holds the private key.
RUN install -d -m 0700 -o 65532 -g 65532 /data
USER 65532:65532
WORKDIR /data
VOLUME /data
# 21115 nat test, 21116 tcp+udp rendezvous, 21117 relay, 21118/21119 web clients (unused)
EXPOSE 21115 21116 21116/udp 21117
CMD ["hbbs"]
