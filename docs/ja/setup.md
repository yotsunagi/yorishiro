# セットアップ

[English](../setup.md) | **日本語**

## 起動手順

必要なもの: Docker / Docker Compose / make。`make init`でイメージをビルドし、PostgreSQLと
`app`（リポジトリルートのマルチステージ`Dockerfile`が生成する、本番と同じreleaseバイナリを
実行するコンテナ）を起動します。

埋め込みプロバイダの設定が起動に必須です。`docker-compose.yml`は既に`app`をローカルONNX
プロバイダに向けているので、あとはモデルを配置するだけです（外部サービス不要）:

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro

# 768次元のBERT系ONNXモデルを配置（embedding-providers.md参照）
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json

$ make init
```

起動時にマイグレーションが自動適用されます。エンドポイント:

| パス | 内容 |
|---|---|
| `http://localhost:8080/up` | Liveness probe（プロセスが起動していれば依存関係を見ず常に200） |
| `http://localhost:8080/health` | Readiness check（DB接続も確認し、障害時は503） |
| `http://localhost:8080/` | セットアップ・ログイン用Web UI（`YSR_WEB_DIR`設定時のみ、後述） |
| `http://localhost:8080/docs` | Swagger UI（REST APIドキュメント） |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI仕様 |
| `http://localhost:8080/mcp` | MCPエンドポイント（Streamable HTTP） |
| `http://localhost:8080/whoami` | 認証確認（ワークスペース・テナント・scopeを返す） |

## 初回セットアップ（コミュニティ版）

コミュニティ版デプロイ（`YORISHIRO_MAX_TENANTS`が設定されている状態 —
`docker-compose.yml`の`app`サービスはこれに加えて`YSR_WEB_DIR=web`も設定済み）では、
`http://localhost:8080/`でセットアップウィザードが配信されます — 管理CLIは不要です。
まだテナントが存在しない初回アクセス時は、メールアドレスとパスワードだけを入力する
フォームが表示され、送信するとテナント・`default`ワークスペース・ownerアカウントが
一括作成され、発行されたAPIキー（他のキー同様、表示は一度だけ）が画面に表示されます。
以降は同じページがログインフォームになります。

同じフローはブラウザなしでも利用できます:

```console
$ curl localhost:8080/setup/status
{"setup_required":true}
$ curl -X POST localhost:8080/setup -H "Content-Type: application/json" \
    -d '{"email":"owner@example.com","password":"a strong password"}'
{"user_id":"...","email":"owner@example.com","tenant_id":"...","workspace_id":"...",
 "api_key":"ysr_..."}
```

`POST /setup`は既にテナントが存在する場合、または`YORISHIRO_MAX_TENANTS`が未設定の
デプロイ（ホスティング版はサインアップ・招待でテナントを増やすため — 
[サインアップ・ログイン・メンバー管理](#サインアップログインメンバー管理)参照）では
`404`を返します。下記の管理CLIは、ウィザードがカバーしない操作（追加のワークスペース/
テナント、招待、キーのローテーション）のために引き続き利用できます。

## テナント・ワークスペース・ユーザー

Yorishiroの制御プレーンは2階層構造です。

- **テナント**は組織/アカウント。`max_workspaces`を設定でき（課金上限。デフォルトの
  `NULL`は無制限で、セルフホスト運用に適する）、任意数の人間の**ユーザー**を
  ロール（`owner`/`admin`/`member`/`viewer`）付きのメンバーシップとして紐付けられる。
  1人のユーザーが複数のテナントに所属することもできる。
- **ワークスペース**はちょうど1つのテナントに属し、実際の操作対象コンテナ。
  スキーマ・エンティティ・リレーション・APIキーはすべてテナントではなくワークスペースに
  紐付く。ワークスペースは`max_entities`を設定できる（これもデフォルト`NULL`/無制限）。

テナントとワークスペースを分けることで、1つの組織が複数の独立したプロジェクト
（環境別・チーム別のワークスペースなど）を新規テナントを都度作らずに運用でき、
複数人でメンバーシップを介して同一テナントの管理権限を共有できます。テナント/
ワークスペースの**作成**は下記の管理CLI（`DATABASE_URL`を持つ者）からのみ可能です。
日々の**メンバーシップ**管理（招待・追加・一覧）はテナントのowner/adminであれば
RESTから行えます — [サインアップ・ログイン・メンバー管理](#サインアップログインメンバー管理)を参照。

デフォルトでは、1つのデプロイでいくつでもテナントを作成できます。セルフホスト
（コミュニティ）版では`YORISHIRO_MAX_TENANTS=1`を設定し（[configuration.md](configuration.md)参照）、
`admin create-tenant`と下記のサインアップフローが2つ目のテナントを作れないようにして
ください。多数のテナントを扱うホスティング版では未設定のままにします。

## テナント・ワークスペース・APIキーの発行

コミュニティ版デプロイは、この節を飛ばして上記のセットアップウィザードを使うこともできます
（`YORISHIRO_MAX_TENANTS=1`の下では最初かつ唯一のテナントになります）。追加のテナント/
ワークスペースの発行や、ホスティング版（`YORISHIRO_MAX_TENANTS`未設定）でのあらゆる発行には
引き続きこの節の手順が唯一の方法です。

APIキーはDBにSHA-256ハッシュ、ユーザーパスワードはargon2ハッシュでのみ保存されるため、
どちらも手作業のSQLでは発行できず、管理CLIで行います:

```console
$ make admin ARGS="create-tenant my-team"
tenant created
  id:            019f565d-f1e3-7afb-b876-b7003e43c230
  name:          my-team
  max_workspaces: unlimited
default workspace created
  id:   019f565d-f204-7f3e-9a1e-2b6b6e2b6b6e
  name: default

$ make admin ARGS="create-api-key 019f565d-f204-7f3e-9a1e-2b6b6e2b6b6e write"
api key created (the plaintext key is shown ONLY once — store it now)
  key:          ysr_928e48292888_ef72...
  ...

$ make admin ARGS="list-tenants"
```

`create-tenant`は新規テナントの下に`default`ワークスペースも自動作成します
（多くの運用ではテナントごとに1ワークスペースで十分なため）。追加のワークスペースが
必要な場合は`create-workspace`を使ってください。平文キーは発行時に一度だけ表示されます。
管理コマンドは`DATABASE_URL`の接続ロール（マイグレーションと同じ管理ロールで、
`identity.tenants`/`identity.users`/`identity.tenant_memberships`に書き込める唯一の
ロール。アプリ自身の`yorishiro_app`ロールにはこの権限がない）で直接DBへアクセスします。

その他の管理コマンド:

| コマンド | 内容 |
|---|---|
| `admin list-tenants` | 全テナントの一覧 |
| `admin create-workspace <tenant-id> <name> [--max-entities <n>]` | テナント配下に追加のワークスペースを作成 |
| `admin list-workspaces <tenant-id>` | テナントのワークスペース一覧 |
| `admin create-user <email> <password> [--display-name <name>]` | 人間のユーザーアカウントを作成 |
| `admin add-member <tenant-id> <user-id> <role>` | ユーザーのテナントへのメンバーシップを追加（または既存のroleを変更）（`owner`/`admin`/`member`/`viewer`） |
| `admin list-members <tenant-id>` | テナントのメンバーとそのroleの一覧 |
| `admin create-invite <tenant-id> <email> <role> [--ttl-hours <n>]` | 指定したメールアドレスがテナントに参加するための招待トークンを発行（デフォルトTTL: 7日）— 詳細は後述 |
| `admin create-api-key <workspace-id> <scope> [--user <user-id>]` | APIキーを発行（`--user`でメンバーに紐付け可能） |
| `admin list-api-keys <workspace-id>` | キーの一覧（ID・scope・prefix・紐付けユーザー・最終使用日時） |
| `admin revoke-api-key <key-id>` | キーの即時失効（漏洩時など） |
| `admin resync-embeddings <workspace-id>` | embedding未生成のentityを再同期（同期失敗からの回復） |

## 認証とscope

すべてのAPIは`Authorization: Bearer <APIキー>`で認証します。キーは`ysr_`で始まる文字列で、
発行時に一度だけ表示されます（DBにはSHA-256ハッシュのみ保存）。

scopeは3段階の包含関係: `read` < `write` < `schema`。
`write`キーは読み取りもでき、`schema`キーはスキーマ登録を含む全操作ができます。

### キーをユーザーに紐付ける

人間の操作も自動化も、最終的にはすべてAPIキーで認証されます（サーバ側にcookie/セッション
状態はありません）が、キーは人間のユーザーに**紐付け**でき、マルチユーザーのアクセス制御は
セッションではなくその紐付けとユーザーのテナントroleを結びつける形で実現しています。
`create-api-key`に`--user <user-id>`を渡すとそのメンバーにキーが紐付き、要求できるscopeは
`MembershipRole::max_scope()`で上限が決まります: `owner`/`admin`は`schema`まで、`member`は
`write`まで、`viewer`は`read`まで発行可能です。この上限を超えるscopeの要求や、ワークスペース
の所属テナントのメンバーでないユーザーへの紐付けは、発行時点で拒否されます。このチェックは
キー発行時に一度だけ行われ、キー自体のscopeと同様にリクエストのたびに再評価されるわけでは
ありません。そのため、メンバーシップを剥奪してもすでに発行済みのキーのscopeが遡って狭まる
ことはありません（その場合はキー自体を失効させてください）。サービス・自動化用の紐付け不要な
キーには`--user`を省略してください（roleによる上限はかかりません）。`GET /whoami`は
ワークスペース・テナント・scopeに加えて、紐付けられた`user_id`（未紐付けなら`null`）も
返します。

`POST /auth/login`（後述）は`admin create-api-key --user`のセルフサービス版に相当します。
`DATABASE_URL`へのアクセスではなくパスワードで認証し、発行時点で呼び出し元自身のroleに
上限を設定済みのキーを発行します。

## サインアップ・ログイン・メンバー管理

アカウント作成は招待制のみです — 公開・無認証のセルフサインアップはありません。
テナントのowner/adminが招待を発行し、招待された人がそれを一度だけ使ってアカウントを作成し、
それ以降はメールアドレス/パスワードで認証してAPIキーを取得します（帯域外でキーを渡されるの
ではなく）。

1. **招待** — テナントのowner/adminがメールアドレスとroleに対して招待トークンを作成します:

   ```console
   $ make admin ARGS="create-invite 019f565d-f1e3-7afb-b876-b7003e43c230 newperson@example.com member"
   invite created (the plaintext token is shown ONLY once — send it now)
     token:      c8b9ea1f...
     ...
     expires at: 2026-07-20 16:57 UTC
   ```

   平文の`token`は帯域外（メール・チャット等）で招待された人に送ってください — APIキー同様、
   表示されるのは一度だけで、DBにはハッシュのみ保存されます。`--ttl-hours`（デフォルト7日）が
   経過するか、使用済みになった時点のいずれか早い方で失効します。

2. **サインアップ** — 招待された人がトークンを使ってアカウントを作成します:

   ```console
   $ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
       -d '{"invite_token":"c8b9ea1f...","password":"a strong password","display_name":"New Person"}'
   {"user_id":"...","email":"newperson@example.com","tenant_id":"...","role":"member",
    "workspaces":[{"id":"...","name":"default"}]}
   ```

   これにより`identity.users`の行が作成されると同時に、招待で指定されたメンバーシップも
   追加されます — 同じ（既に消費済みの）トークンでの2回目のサインアップは拒否されます（422）。

3. **ログイン** — 以降、ユーザーはパスワードと引き換えに、1つのワークスペースにスコープされ、
   自身のroleの`max_scope()`で上限が設定された新しいAPIキーを取得します（前述参照）:

   ```console
   $ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
       -d '{"email":"newperson@example.com","password":"a strong password","workspace_id":"..."}'
   {"api_key":"ysr_...","api_key_id":"...","workspace_id":"...","scope":"write","user_id":"..."}
   ```

   ログインのたびに既存キーの再利用ではなく*新しい*キーが発行されます — 不要になった古い
   キーは`admin revoke-api-key`で失効させてください。

4. **メンバー管理** — 認証後は、テナントのowner/adminは`DATABASE_URL`/管理CLIを一切使わずに
   RESTでメンバーの一覧・追加ができます:

   ```console
   $ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
   $ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
       -H "Content-Type: application/json" \
       -d '{"email":"existing-user@example.com","role":"admin"}'
   ```

   `POST /api/members`は**既存の**アカウント（既にサインアップを完了しているもの）を呼び出し
   元のテナントに追加します — 新規アカウントを作成することはありません。まだアカウントを
   持たない人を招き入れるには、代わりに招待（手順1）を発行してください。両エンドポイントとも、
   呼び出し元自身のキーがOwner/Adminメンバーに紐付いていることが必要です — Memberロールの
   キーは、そのキー自身のscopeに関わらず403で拒否されます。メンバー管理はscopeの問題ではなく
   テナントroleの問題だからです。

ホスティング版の管理ダッシュボードSPAは手順3・4をブラウザUIでラップしたものです —
[deployment.md](deployment.md#ホスティング版のデプロイ)を参照してください。
