# 本番配布用のマルチステージビルド。開発は .devcontainer/Dockerfile +
# docker-compose.yml を使う（このファイルはcompose構成からは参照されない）。
#
#   docker build -t yorishiro .
#   docker run --rm -e DATABASE_URL=... -e YSR_EMBEDDING_PROVIDER=... yorishiro
#
# 注意: ortクレートがビルド時にonnxruntimeバイナリを取得するため、ビルドには
# ネットワークアクセスが必要（閉域ビルドはORT_LIB_LOCATION、README参照）。
FROM rust:1.97-slim AS builder

# curlはutoipa-swagger-uiのbuild.rs（Swagger UIアセットの取得）が要求する。
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

# onnxruntimeは静的リンクされるため、実行時に必要な共有ライブラリは
# libstdc++6のみ（+ OpenAI互換プロバイダのTLS用にca-certificates）。
# ベースはbuilder（rust:1.97-slim = Debian trixie）とglibcを揃えること。
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home yorishiro

COPY --from=builder /usr/local/bin/yorishiro-server /usr/local/bin/yorishiro-server

USER yorishiro
EXPOSE 8080
ENTRYPOINT ["yorishiro-server"]
