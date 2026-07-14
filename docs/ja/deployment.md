# 本番デプロイ

[English](../deployment.md) | **日本語**

リポジトリ直下の`Dockerfile`（マルチステージ）で自己完結の実行イメージをビルドできます:

```console
$ docker build -t yorishiro .
$ docker run --rm -p 8080:8080 \
    -e DATABASE_URL=postgres://... \
    -e YSR_EMBEDDING_BASE_URL=... -e YSR_EMBEDDING_MODEL=... \
    yorishiro
```

マイグレーションはバイナリに埋め込まれており起動時に自動適用されます（複数レプリカの
同時起動もadvisory lockで安全）。SIGTERM/Ctrl-Cでgraceful shutdownし、処理中のリクエストと
バックグラウンドのembedding同期の完了（最大30秒）を待ってから終了します。それでも
embedding同期が失われた場合は`admin resync-embeddings`で回復できます。

管理CLIは同じイメージで実行できます:

```console
$ docker run --rm -e DATABASE_URL=postgres://... yorishiro admin list-tenants
```

## リリース

`vX.Y.Z`タグをpushすると`.github/workflows/release.yml`がトリガーされ、`yorishiro-server`の
`x86_64`/`aarch64` Linux（glibc）バイナリをビルドしてGitHub Releaseに添付し、マルチアーキの
Dockerイメージを`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`（および`:latest`）としてビルド・pushします。
どちらのアーキテクチャも（上記の`ort`/onnxruntimeのビルド要件に合わせて）QEMUを使わず
ネイティブビルドします。

```console
$ git tag vX.Y.Z && git push origin vX.Y.Z
```

## ホスティング版のデプロイ

セルフホスト（コミュニティ）版に必要なのは上記だけです — `YORISHIRO_MAX_TENANTS=1`と
`YSR_WEB_DIR=web`を設定（[configuration.md](configuration.md)参照）すればそれで十分です。
これは[`web/`](../web)のSPAを配信し、そのセットアップウィザード
（[setup.md](setup.md#初回セットアップコミュニティ版)参照）だけでコミュニティ版の
唯一のテナントをオンボードでき、ホスティング版限定のダッシュボード画面は一切必要ありません。

ホスティング版（マルチテナント・課金対応）— Stripeサブスクリプション Webhook、プラン/
使用量計測、管理ダッシュボードSPA — は別プロダクトとして、この`yorishiro-server`のAPIと
DBに対して動作する`yorishiro-hosted-server`という第2プロセスで構成されます。課金ロジックや
プラン・料金の詳細をコミュニティ版のソースツリーから分離するため、非公開リポジトリ
（`yotsunagi/yorishiro-enterprise`）で開発・配布されており、このリポジトリには含まれません。
デプロイ手順はそちらのリポジトリのドキュメントを参照してください。このAPIの上に追加される
エンドポイント（`POST /hosted/stripe/webhook`、`GET /hosted/tenant/overview`）については
参考として[api.md](api.md#ホスティング版限定のエンドポイント)にも記載しています。
