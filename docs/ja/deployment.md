# 本番デプロイ

[English](../deployment.md) | **日本語**

起動そのものの手順(Docker・ビルド済みバイナリ・ソースから)は[setup.md](setup.md)を参照してください。このガイドはバックグラウンド起動、リリースの切り方、シングルテナント構成をカバーします。

## バックグラウンドで起動する

### Docker

[setup.md](setup.md#dockerで動かす)で使う`-d --restart unless-stopped`は、バックグラウンドで起動し、再起動やクラッシュ後も自動的に立ち上がり直します。

```console
$ docker logs -f yorishiro      # ログ追跡
$ docker stop yorishiro         # graceful shutdown
```

マイグレーションはバイナリに埋め込まれており起動時に自動適用されます(複数レプリカの同時起動もadvisory lockで安全)。SIGTERM/Ctrl-Cでgraceful shutdownし、処理中のリクエストとバックグラウンドのembedding同期の完了(最大30秒)を待ってから終了します。それでもembedding同期が失われた場合は`admin resync-embeddings`で回復できます。

管理CLIは同じイメージで実行できます。

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

未リリースの変更を試すなど、ソースからイメージをビルドしたい場合は、リポジトリ直下の同じマルチステージ`Dockerfile`を使います。

```console
$ docker build -t yorishiro .
```

### systemd(ビルド済みバイナリ)

[setup.md](setup.md#ビルド済みバイナリで動かす)で起動したプロセスを、systemdユニットで再起動をまたいで維持し、異常終了時も自動再起動できます。プレーンなシェルと異なり、systemdの`EnvironmentFile=`は`.env`を直接読み込むため、`source`/`set -a`は不要です。

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

`vX.Y.Z`タグをpushすると`.github/workflows/release.yml`がトリガーされます。`yorishiro-server`の`x86_64`/`aarch64` Linux(glibc、`linux-amd64`/`linux-arm64`として梱包)バイナリをビルドしてGitHub Releaseに添付し、マルチアーキのDockerイメージを`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`(および`:latest`)としてビルド・pushします。どちらのアーキテクチャも`ort`/onnxruntimeのビルド要件に合わせて、QEMUを使わずネイティブビルドします。

```console
$ git tag vX.Y.Z && git push origin vX.Y.Z
```

## シングルテナント構成

`YORISHIRO_MAX_TENANTS=1`・`YSR_EMBEDDING_PROVIDER=local`(いずれも[configuration.md](configuration.md)参照)は共に既定値です。これらを未設定のままにしたデプロイはそのまま[`web/`](../crates/yorishiro-web/web)のSPA(バイナリに組み込み済み)を配信し、そのセットアップウィザード([setup.md](setup.md#初回セットアップ)参照)だけでデプロイの唯一のテナントをオンボードでき、埋め込みにはローカルONNXモデルを使います。テナント上限を外すには`YORISHIRO_MAX_TENANTS=0`を設定してください。
