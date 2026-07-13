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
