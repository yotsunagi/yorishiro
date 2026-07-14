# 本番デプロイ

[English](../deployment.md) | **日本語**

## Docker（ビルド済みイメージ）

`vX.Y.Z`タグをpushするたびに`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`（および`:latest`）が
公開されます（詳細は下の[リリース](#リリース)参照）。このイメージにはセットアップ
ウィザードのSPA（`web/`）が既に同梱されており、ローカルでビルドする必要はありません:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e DATABASE_URL=postgres://... \
    -e YSR_EMBEDDING_BASE_URL=... -e YSR_EMBEDDING_MODEL=... \
    ghcr.io/yotsunagi/yorishiro:latest
```

`-d --restart unless-stopped`はバックグラウンドで起動し、再起動やクラッシュ後も
自動的に立ち上がり直します。`docker logs -f yorishiro`でログ追跡、`docker stop yorishiro`
でgraceful shutdownできます。マイグレーションはバイナリに埋め込まれており起動時に自動
適用されます（複数レプリカの同時起動もadvisory lockで安全）。SIGTERM/Ctrl-Cでgraceful
shutdownし、処理中のリクエストとバックグラウンドのembedding同期の完了（最大30秒）を
待ってから終了します。それでもembedding同期が失われた場合は`admin resync-embeddings`
で回復できます。

管理CLIは同じイメージで実行できます:

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

未リリースの変更を試すなど、ソースからイメージをビルドしたい場合は、リポジトリ直下の
同じマルチステージ`Dockerfile`を使って`docker build -t yorishiro .`でビルドできます。

## ビルド済みバイナリ（Dockerなし）

各リリースにはLinuxバイナリ（`yorishiro-server-vX.Y.Z-linux-amd64.tar.gz` /
`-linux-arm64.tar.gz`）も[GitHub Release](https://github.com/yotsunagi/yorishiro/releases)
に添付されます。Dockerイメージと異なり、このアーカイブには`yorishiro-server`バイナリ
そのものしか含まれていません — セットアップウィザードの`web/`ディレクトリや
（ローカルONNXプロバイダを使う場合の）`models/`はビルドに組み込まれる資産ではないため
同梱されていません。リリース元と同じタグから取得し、バイナリと同じ場所に置いてください
（`YSR_WEB_DIR=web`のような相対パスは、プロセスの作業ディレクトリを基準に解決されます）:

```console
$ mkdir -p /opt/yorishiro && cd /opt/yorishiro
$ curl -L -o yorishiro.tar.gz \
    https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/yorishiro-server-vX.Y.Z-linux-amd64.tar.gz
$ tar -xzf yorishiro.tar.gz && rm yorishiro.tar.gz

# web/（セットアップウィザード）と、必要なら models/（ローカルONNXプロバイダ）は同じタグから取得
$ curl -L https://github.com/yotsunagi/yorishiro/archive/refs/tags/vX.Y.Z.tar.gz \
    | tar -xz --strip-components=1 "yorishiro-*/web"
```

### バックグラウンドで起動する

ベアメタル/VMへのデプロイでは、systemdユニットを使うと再起動をまたいでプロセスを
維持でき、異常終了時も自動再起動されます:

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
EnvironmentFile=/opt/yorishiro/yorishiro.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

`yorishiro.env`には`DATABASE_URL`、`YSR_WEB_DIR=web`、`YORISHIRO_MAX_TENANTS=1`など
[configuration.md](configuration.md)の各変数を1行1つの`KEY=value`形式で書きます。
その後:

```console
$ sudo systemctl enable --now yorishiro
$ journalctl -u yorishiro -f
```

## リリース

`vX.Y.Z`タグをpushすると`.github/workflows/release.yml`がトリガーされ、`yorishiro-server`の
`x86_64`/`aarch64` Linux（glibc、`linux-amd64`/`linux-arm64`として梱包）バイナリをビルドして
GitHub Releaseに添付し、マルチアーキのDockerイメージを`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`
（および`:latest`）としてビルド・pushします。どちらのアーキテクチャも（上記の
`ort`/onnxruntimeのビルド要件に合わせて）QEMUを使わずネイティブビルドします。

```console
$ git tag vX.Y.Z && git push origin vX.Y.Z
```

## シングルテナント構成

`YORISHIRO_MAX_TENANTS=1`と`YSR_WEB_DIR=web`を設定（[configuration.md](configuration.md)
参照）すれば、[`web/`](../web)のSPAを配信でき、そのセットアップウィザード
（[setup.md](setup.md#初回セットアップ)参照）だけでデプロイの唯一のテナントを
オンボードできます。
