# Multi-stage build producing a self-contained runtime image. Used both for production
# distribution and as the `app` service in docker-compose.yml; day-to-day development
# (test/fmt/clippy) instead runs through the `dev` service, see .devcontainer/Dockerfile.
#
#   docker build -t yorishiro .
#   docker run --rm -e DATABASE_URL=... -e YSR_EMBEDDING_PROVIDER=... yorishiro
#
# Note: the `ort` crate fetches an onnxruntime binary at build time, so the build needs
# network access (for air-gapped builds, see ORT_LIB_LOCATION in the README).
FROM rust:1.97-slim AS builder

# curl is required by utoipa-swagger-ui's build.rs (fetches the Swagger UI assets).
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    g++ \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/root/.cache/ort.pyke.io \
    cargo build --release -p yorishiro-server \
    && cp target/release/yorishiro-server /usr/local/bin/yorishiro-server

# onnxruntime is statically linked, so the only shared library needed at runtime is
# libstdc++6 (plus ca-certificates for the OpenAI-compatible provider's TLS, and curl for
# the HEALTHCHECK below). Keep the base (debian trixie, matching builder's rust:1.97-slim)
# on the same glibc as the builder.
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home yorishiro

COPY --from=builder /usr/local/bin/yorishiro-server /usr/local/bin/yorishiro-server

# Relative paths in embedding provider settings (e.g. YSR_ONNX_MODEL_PATH=models/model.onnx)
# resolve against this directory, so a model directory can be bind-mounted here without
# also needing an absolute-path override.
WORKDIR /app
RUN chown -R yorishiro:yorishiro /app

USER yorishiro
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
    CMD curl -sf http://localhost:8080/up || exit 1
ENTRYPOINT ["yorishiro-server"]
