# Pictesa: one image for local or hosted use.
#   docker build -t pictesa .
#   docker run -p 4747:4747 -v pictesa:/data -e PICTESA_TOKEN=change-me pictesa
FROM node:22-bookworm-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
RUN cargo build --release --locked -p pictesa-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data pictesa && mkdir -p /data && chown pictesa /data
COPY --from=build /src/target/release/pictesa /usr/local/bin/pictesa
USER pictesa
ENV PICTESA_DATA_DIR=/data PICTESA_HOST=0.0.0.0 PICTESA_PORT=4747
EXPOSE 4747
VOLUME /data
ENTRYPOINT ["pictesa"]
CMD ["serve"]
