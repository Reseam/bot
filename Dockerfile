FROM node:24-trixie-slim AS release
ADD https://git.reseam.app/reseam/bot/releases/download/latest/reseam-bot-linux-x64.tar.gz /release.tar.gz
RUN mkdir /release && tar -xzf /release.tar.gz -C /release

FROM node:24-trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bot \
    && install -d -o bot -g bot /var/lib/reseam-bot
COPY --from=release /release /
ENV RESEAM_BOT_CONFIG=/etc/reseam-bot/config.toml \
    DATA_DIR=/var/lib/reseam-bot \
    SANDBOX_ENTRY=/usr/local/lib/reseam-bot/sandbox/dist/main.js \
    NODE_ENV=production
USER bot
WORKDIR /home/bot
VOLUME /var/lib/reseam-bot
CMD ["reseam-bot"]
