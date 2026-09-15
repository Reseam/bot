FROM rust:1.94-trixie AS chef
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git jq python3 ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bot \
    && mkdir /data \
    && chown bot:bot /data
COPY --from=build /src/target/release/reseam-bot /usr/local/bin/reseam-bot
COPY config.toml /etc/reseam-bot/config.toml
ENV RESEAM_BOT_CONFIG=/etc/reseam-bot/config.toml DATA_DIR=/data
USER bot
WORKDIR /home/bot
VOLUME /data
CMD ["reseam-bot"]
