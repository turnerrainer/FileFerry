FROM rust:1.94-slim-bookworm AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin fileferry

FROM debian:13.6-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/fileferry /app/fileferry
# Ship the demo self-contained: fileferry.yaml + an empty data
# directory. Operators can bind-mount over either to override.
COPY fileferry.yaml /app/fileferry.yaml
RUN mkdir -p /app/data

EXPOSE 8080
RUN useradd -m -u 1000 fileferry && chown -R fileferry:fileferry /app
USER fileferry

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/fileferry"]
