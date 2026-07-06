# callbro central hub — builds the standalone WebSocket server.
# Runs on AWS Lightsail container service (see DEPLOY.md). Build for linux/amd64.
# Build context is the repo root; only the `server/` crate is compiled.

FROM rust:1-slim AS build
WORKDIR /build
COPY server/ ./server
WORKDIR /build/server
RUN cargo build --release

FROM debian:stable-slim
RUN useradd --create-home --uid 10001 app \
    && mkdir -p /data && chown app:app /data
COPY --from=build /build/server/target/release/callbro-server /usr/local/bin/callbro-server
USER app
ENV CALLBRO_STATE_DIR=/data \
    PORT=8080
EXPOSE 8080
# Seat layout + roster live here. NOTE: Lightsail container services can't mount
# a volume, so /data is ephemeral there (wiped on redeploy) — re-seed after deploy.
VOLUME ["/data"]
CMD ["callbro-server"]
