# Builds hbbs (rendezvous/ID server) and hbbr (relay) from the pinned rustdesk-server source.
# Build from the repo root:  docker build -f server/Dockerfile -t tether-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY vendor/rustdesk-server/ .
RUN cargo build --release --bin hbbs --bin hbbr

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/hbbs /src/target/release/hbbr /usr/local/bin/
WORKDIR /data
VOLUME /data
# 21115 nat test, 21116 tcp+udp rendezvous, 21117 relay, 21118/21119 web clients (unused)
EXPOSE 21115 21116 21116/udp 21117
CMD ["hbbs"]
