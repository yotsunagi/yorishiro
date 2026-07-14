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

## ホスティング版のデプロイ

セルフホスト（コミュニティ）版に必要なのは上記だけです — `YORISHIRO_MAX_TENANTS=1`を
設定（[configuration.md](configuration.md)参照）すればそれで十分です。*ホスティング*版
では追加で`yorishiro-hosted-server`という別プロセスを実行します。これは意図的に分離した
別の`Dockerfile.hosted`からビルドされ（コミュニティ版イメージの依存関係ツリーや攻撃対象
領域に含まれないようにするため）、以下を提供します:

- `POST /hosted/stripe/webhook` — Stripeのサブスクリプション Webhook受信。StripeのPrice ID
  をプラン（`free`/`pro`/`team`）にマッピングし、結果として得られる上限を
  `identity.tenants`に書き込みます。
- `GET /hosted/tenant/overview` — 管理ダッシュボードを支えるデータ（プラン・使用量・
  メンバー一覧）を、呼び出し元自身のテナントに限定して返します。
- 管理ダッシュボードSPA本体（ログイン・使用量/課金状況・メンバー管理 — フレームワークを
  使わない静的サイト。[`web/`](../../web)配下にあり、`ServeDir`で直接配信されます）。

### Docker Compose経由（`hosted`プロファイル）

`docker-compose.yml`は`hosted`をopt-inの`"hosted"`Composeプロファイル配下に定義しているため、
通常の`docker compose up`/`make up`（セルフホスト版）では起動しません:

```console
$ make up-hosted   # docker compose --profile hosted up -d db app hosted
```

これにより`db` + `app`（`yorishiro-server`、8080番ポート）+ `hosted`
（`yorishiro-hosted-server`、8081番ポート、ダッシュボードは`http://localhost:8081/`）が
起動します。ダッシュボードのログイン/メンバー管理呼び出しは`app`のオリジンへ向かう一方、
ダッシュボードのページ自体は`hosted`のオリジンから配信されるため、`docker-compose.yml`では
`app`の`YSR_CORS_ORIGINS`に既定で`http://localhost:8081`が含まれています — hostedサービスを
別オリジンで公開する場合は調整してください。[`web/config.js`](../../web/config.js)を編集する
（またはこのファイルの上に置き換えをbind-mountする）ことで、ダッシュボードの`apiBase`を
実際に到達可能な`yorishiro-server`の場所に向けられます。

### 単体イメージ

```console
$ docker build -f Dockerfile.hosted -t yorishiro-hosted .
$ docker run --rm -p 8081:8081 -e DATABASE_URL=postgres://... yorishiro-hosted
```

### 実際のStripe/メール認証情報

実際のStripe認証情報と実際のトランザクションメールプロバイダの設定は意図的に見送られて
います: `YORISHIRO_STRIPE_WEBHOOK_SECRET`が未設定の場合、Webhookエンドポイントは検証不能な
リクエストを受け付ける代わりに`501`を返し、組み込みの`EmailProvider`は送信するはずだった
メッセージをログに出力するだけです。実運用に切り替えるには、Stripe関連の環境変数を設定し
（[configuration.md](configuration.md#ホスティング版限定yorishiro-hosted-server)参照）、
`NoopEmailProvider`の代わりに`yorishiro_hosted::email::EmailProvider`を実際のプロバイダ
（SES、Postmark等）向けに実装してください。
