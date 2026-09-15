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

FROM node:24-trixie-slim AS sandbox
WORKDIR /sandbox
COPY sandbox/package.json sandbox/package-lock.json ./
RUN npm ci
COPY sandbox/tsconfig.json ./
COPY sandbox/src ./src
RUN npm run build && npm prune --omit=dev --omit=optional

FROM node:24-trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bot \
    && install -d -o bot -g bot /var/lib/reseam-bot
COPY --from=build /src/target/release/reseam-bot /usr/local/bin/reseam-bot
COPY --from=sandbox /sandbox/package.json /usr/local/lib/reseam-bot/sandbox/package.json
COPY --from=sandbox /sandbox/node_modules /usr/local/lib/reseam-bot/sandbox/node_modules
COPY --from=sandbox /sandbox/dist /usr/local/lib/reseam-bot/sandbox/dist
COPY config.toml /etc/reseam-bot/config.toml
ENV RESEAM_BOT_CONFIG=/etc/reseam-bot/config.toml \
    DATA_DIR=/var/lib/reseam-bot \
    SANDBOX_ENTRY=/usr/local/lib/reseam-bot/sandbox/dist/main.js \
    NODE_ENV=production
USER bot
WORKDIR /home/bot
VOLUME /var/lib/reseam-bot
CMD ["reseam-bot"]
