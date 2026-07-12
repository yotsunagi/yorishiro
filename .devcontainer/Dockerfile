FROM rust:1.97-slim

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    curl \
    g++ \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt

RUN cargo install sqlx-cli --no-default-features --features postgres

WORKDIR /workspace
