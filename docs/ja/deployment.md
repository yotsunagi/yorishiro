# 本番デプロイ

[English](../deployment.md) | **日本語**

## Docker(ビルド済みイメージ)

`vX.Y.Z`タグをpushするたびに`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`(および`:latest`)が公開されます(詳細は下の[リリース](#リリース)参照)。セットアップウィザードのSPA(`web/`)はバイナリに組み込まれているため、別途ビルドやマウントは不要です:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -v "$(pwd)/models:/app/models:ro" \
    -e DATABASE_URL=postgres://... \
    ghcr.io/yotsunagi/yorishiro:latest
```

埋め込みプロバイダは既定で(上でマウントした)`models/model.onnx`/`models/tokenizer.json`のローカルONNXモデルを使います — これらのファイルの取得方法は[setup.md](setup.md#起動手順)を、代わりにOpenAI互換エンドポイントを使う方法は[embedding-providers.md](embedding-providers.md)を参照してください。

`-d --restart unless-stopped`はバックグラウンドで起動し、再起動やクラッシュ後も自動的に立ち上がり直します。`docker logs -f yorishiro`でログ追跡、`docker stop yorishiro`でgraceful shutdownできます。マイグレーションはバイナリに埋め込まれており起動時に自動適用されます(複数レプリカの同時起動もadvisory lockで安全)。SIGTERM/Ctrl-Cでgraceful shutdownし、処理中のリクエストとバックグラウンドのembedding同期の完了(最大30秒)を待ってから終了します。それでもembedding同期が失われた場合は`admin resync-embeddings`で回復できます。

管理CLIは同じイメージで実行できます:

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

未リリースの変更を試すなど、ソースからイメージをビルドしたい場合は、リポジトリ直下の同じマルチステージ`Dockerfile`を使って`docker build -t yorishiro .`でビルドできます。

## ビルド済みバイナリ(Dockerなし)

各リリースにはLinuxバイナリ(`yorishiro-server-vX.Y.Z-linux-amd64.tar.gz` / `-linux-arm64.tar.gz`)も[GitHub Release](https://github.com/yotsunagi/yorishiro/releases)に添付されます。このアーカイブには`yorishiro-server`バイナリそのものしか含まれていませんが、セットアップウィザードの`web/`はバイナリに組み込まれているため取得不要です — `models/`(ローカルONNXプロバイダを使う場合)だけは、モデルの重みが埋め込まれていないためバイナリと同じ場所に置く必要があります:

```console
$ mkdir -p /opt/yorishiro && cd /opt/yorishiro

# バイナリ本体
$ curl -L -o yorishiro.tar.gz https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/yorishiro-server-vX.Y.Z-linux-amd64.tar.gz
$ tar -xzf yorishiro.tar.gz && rm yorishiro.tar.gz

# models/(ローカルONNX埋め込みプロバイダ、既定 -- OpenAI互換エンドポイントを使いたい場合はembedding-providers.md参照)
$ mkdir -p models
$ curl -L -o models/model.onnx https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json

# .env.example、コメント付きの全変数リファレンス、同じタグから取得
$ curl -L -o .env https://raw.githubusercontent.com/yotsunagi/yorishiro/vX.Y.Z/.env.example
```

`.env`を編集し、最低限`DATABASE_URL`を設定してください。それ以外はコメントアウトのままで構いません — `YORISHIRO_MAX_TENANTS`・`YSR_EMBEDDING_PROVIDER`(とONNXモデル/トークナイザーのパス)は全て、セルフホスト環境が通常望むシングルテナント・Web UI有効・ローカルONNX埋め込みの値を既定としており、上で取得したファイルとも一致します — 全変数のリファレンスと変更方法は[configuration.md](configuration.md)を参照してください。

このバイナリは実際のプロセス環境変数からしか設定を読みません — `.env`ファイル自体を読む仕組みはありません — そのため直接実行する場合は、まず`.env`をシェルに読み込む必要があります。一方`config.yml`ファイルはバイナリが直接読み込みます（[configuration.md](configuration.md#configyml)と[`config.example.yml`](../../config.example.yml)を参照）— 今回のようなベアメタル/systemdデプロイでは、バイナリの隣に`config.yml`を置くだけで済むため、下記の2通りの`.env`読み込み方法よりシンプルなことが多いです（シェルでのsourceも`EnvironmentFile=`も不要）:

```console
$ set -a; source .env; set +a
$ ./yorishiro-server
```

### バックグラウンドで起動する

ベアメタル/VMへのデプロイでは、systemdユニットを使うと再起動をまたいでプロセスを維持でき、異常終了時も自動再起動されます。プレーンなシェルと異なり、systemdの`EnvironmentFile=`は`.env`を直接読み込むため、`source`/`set -a`は不要です:

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
EnvironmentFile=/opt/yorishiro/.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro
$ journalctl -u yorishiro -f
```

## リリース

`vX.Y.Z`タグをpushすると`.github/workflows/release.yml`がトリガーされ、`yorishiro-server`の`x86_64`/`aarch64` Linux(glibc、`linux-amd64`/`linux-arm64`として梱包)バイナリをビルドしてGitHub Releaseに添付し、マルチアーキのDockerイメージを`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`(および`:latest`)としてビルド・pushします。どちらのアーキテクチャも(上記の`ort`/onnxruntimeのビルド要件に合わせて)QEMUを使わずネイティブビルドします。

```console
$ git tag vX.Y.Z && git push origin vX.Y.Z
```

## シングルテナント構成

`YORISHIRO_MAX_TENANTS=1`・`YSR_EMBEDDING_PROVIDER=local`(いずれも[configuration.md](configuration.md)参照)は共に既定値なので、これらを未設定のままにしたデプロイはそのまま[`web/`](../web)のSPA(バイナリに組み込み済み)を配信し、そのセットアップウィザード([setup.md](setup.md#初回セットアップ)参照)だけでデプロイの唯一のテナントをオンボードでき、埋め込みにはローカルONNXモデルを使います。テナント上限を外すには`YORISHIRO_MAX_TENANTS=0`を設定してください。
