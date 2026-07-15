# セットアップ

[English](../setup.md) | **日本語**

## 前提条件

サーバの起動には埋め込みモデルが必要です。既定のローカルONNXプロバイダは、モデルファイル以外の外部サービスや設定を必要としません。

1. 768次元のBERT系ONNXモデルを取得します。

   ```console
   $ mkdir -p models
   $ curl -L -o models/model.onnx \
       https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
   $ curl -L -o models/tokenizer.json \
       https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json
   ```

OpenAI互換エンドポイントを代わりに使う場合は[embedding-providers.md](embedding-providers.md)を参照してください。

以下3つの起動方法から1つを選んでください。

## Dockerで動かす

最も手早い方法です。DockerとPostgreSQLへの接続先が必要です。

1. 上記の[前提条件](#前提条件)を済ませます。
2. DBと`models`ディレクトリを指定してコンテナを起動します。

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -v "$(pwd)/models:/app/models:ro" \
       -e DATABASE_URL=postgres://... \
       ghcr.io/yotsunagi/yorishiro:latest
   ```

3. 起動を確認します。

   ```console
   $ curl localhost:8080/up
   ```

これだけでシングルテナント構成として完全に動作します。Web UIはバイナリに組み込み済みで、別途`web/`を取得・マウントする必要はありません。`YORISHIRO_MAX_TENANTS`/`YSR_EMBEDDING_PROVIDER`も既定でシングルテナント・ローカルONNXの値になっており、上でマウントした`models/`と一致します。変更方法は[configuration.md](configuration.md)を、バックグラウンド起動やソースからのイメージビルド、同じイメージでの管理CLI実行は[deployment.md](deployment.md#バックグラウンドで起動する)を参照してください。

## ビルド済みバイナリで動かす

Dockerを使わない、ベアメタル/VMへのデプロイ向けです。

1. 上記の[前提条件](#前提条件)を済ませます。
2. 自分のアーキテクチャ向けのリリースアーカイブを取得して展開します。

   ```console
   $ mkdir -p /opt/yorishiro && cd /opt/yorishiro
   $ curl -L -o yorishiro.tar.gz \
       https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/yorishiro-server-vX.Y.Z-linux-amd64.tar.gz
   $ tar -xzf yorishiro.tar.gz && rm yorishiro.tar.gz
   ```

   アーカイブには`yorishiro-server`バイナリのみが含まれます。Web UIはバイナリに組み込み済みなので別途取得は不要です。手順1で用意した`models/`をバイナリの隣に移動(またはシンボリックリンク)してください。
3. 少なくとも`DATABASE_URL`を設定します。バイナリの隣に置く`config.yml`ファイル(バイナリが直接読み込みます。[configuration.md](configuration.md#configyml)と[`config.example.yml`](../../config.example.yml)参照)、または起動するシェルに読み込む方法のいずれかです。

   ```console
   $ curl -L -o .env https://raw.githubusercontent.com/yotsunagi/yorishiro/vX.Y.Z/.env.example
   # (.envを編集してDATABASE_URLを設定。他はコメントアウトのままで構わない)
   $ set -a; source .env; set +a
   ```

4. 起動します。

   ```console
   $ ./yorishiro-server
   ```

systemdで再起動をまたいで動かし続ける方法は[deployment.md](deployment.md#バックグラウンドで起動する)を参照してください。

## ソースから動かす(Docker Compose)

ローカル開発向けです。Docker、Docker Compose、makeが必要です。

1. リポジトリをcloneしてから、その中で上記の[前提条件](#前提条件)を済ませます。

   ```console
   $ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro
   # (上記と同様にmodels/model.onnx、models/tokenizer.jsonを配置)
   ```

2. イメージをビルドし(上記のリリースイメージと同じマルチステージ`Dockerfile`を使用)、PostgreSQLと`app`を起動します。`docker-compose.yml`は既に`app`を上記のローカルONNXプロバイダに向けています。

   ```console
   $ make init
   ```

上記3つの方法いずれで使う`-e`/環境変数も`config.yml`ファイルで代用できます(Dockerなら`/app/config.yml`にマウント)。長い`-e`の羅列より便利です。詳細は[configuration.md](configuration.md#configyml)と[`config.example.yml`](../../config.example.yml)を参照してください。

## エンドポイント

起動時にマイグレーションが自動適用されます(上記3つの方法いずれでも共通)。

| パス | 内容 |
|---|---|
| `http://localhost:8080/up` | Liveness probe。プロセスが起動していれば依存関係を見ず常に200 |
| `http://localhost:8080/health` | Readiness check。DB接続も確認し、障害時は503 |
| `http://localhost:8080/` | セットアップ・ログイン・ワークスペース管理用Web UI。バイナリに組み込み済み。実ディレクトリから配信させる場合は[configuration.md](configuration.md)の`YSR_WEB_DIR`を参照 |
| `http://localhost:8080/docs` | Swagger UI(REST APIドキュメント) |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI仕様 |
| `http://localhost:8080/mcp` | MCPエンドポイント(Streamable HTTP) |
| `http://localhost:8080/whoami` | 認証確認。ワークスペース・テナント・scopeを返す |

## 初回セットアップ

`YORISHIRO_MAX_TENANTS`が実際の上限として解決されるデプロイ(既定は未設定で`1`)は、`http://localhost:8080/`でセットアップウィザードを配信します。管理CLIは不要です。まだテナントが存在しない初回アクセス時は、メールアドレスとパスワードだけを入力するフォームが表示されます。送信するとテナント・`default`ワークスペース・ownerアカウントが一括作成され、発行されたAPIキーが画面に表示されます(他のキー同様、表示は一度だけ)。以降は同じページがログインフォームになります。

同じフローはブラウザなしでも利用できます。

```console
$ curl localhost:8080/setup/status
{"setup_required":true}
$ curl -X POST localhost:8080/setup -H "Content-Type: application/json" \
    -d '{"email":"owner@example.com","password":"a strong password"}'
{"user_id":"...","email":"owner@example.com","tenant_id":"...","workspace_id":"...",
 "api_key":"ysr_..."}
```

`POST /setup`は既にテナントが存在する場合、または`YORISHIRO_MAX_TENANTS`が無制限に解決されるデプロイ(明示的に`0`を設定した場合)では`404`を返します。後者はサインアップ・招待でテナントを増やします。詳しくは[サインアップ・ログイン・メンバー・ワークスペース管理](#サインアップログインメンバーワークスペース管理)を参照してください。下記の管理CLIは、ウィザードがカバーしない操作(追加のワークスペース/テナント、招待、キーのローテーション)に引き続き使えます。

## テナント・ワークスペース・ユーザー

Yorishiroの制御プレーンは2階層構造です。

- **テナント**は組織/アカウントです。`max_workspaces`という課金上限を設定できます(デフォルトは`NULL`で無制限。セルフホスト運用に適します)。任意数の人間の**ユーザー**をロール(`owner`/`admin`/`member`/`viewer`)付きのメンバーシップとして紐付けられ、1人のユーザーが複数のテナントに所属することもできます。
- **ワークスペース**はちょうど1つのテナントに属する、実際の操作対象コンテナです。スキーマ・エンティティ・リレーション・APIキーはすべてテナントではなくワークスペースに紐付きます。`max_entities`という上限も設定できます(デフォルト`NULL`/無制限)。

テナントとワークスペースを分けることで、1つの組織が複数の独立したプロジェクト(環境別・チーム別のワークスペースなど)を新規テナントを都度作らずに運用でき、複数人でメンバーシップを介して同一テナントの管理権限を共有できます。テナント/ワークスペースの**作成**は管理CLI(`DATABASE_URL`を持つ者)からのみ可能です。日々の**メンバーシップ**管理(招待・追加・一覧)はテナントのowner/adminであればRESTから行えます。詳しくは[サインアップ・ログイン・メンバー・ワークスペース管理](#サインアップログインメンバーワークスペース管理)を参照してください。

デフォルト(`YORISHIRO_MAX_TENANTS`未設定)では、1つのデプロイはテナント1つに制限されます。`admin create-tenant`とサインアップフローは2つ目のテナントを作れません。無制限にするには`YORISHIRO_MAX_TENANTS=0`を、特定数までにするにはその数を設定してください([configuration.md](configuration.md)参照)。

## テナント・ワークスペース・APIキーの発行

セットアップウィザードを使ったデプロイは、この節を飛ばせます。既定の`YORISHIRO_MAX_TENANTS=1`の下では最初かつ唯一のテナントになるためです。追加のテナント/ワークスペースの発行や、`YORISHIRO_MAX_TENANTS`が無制限に解決されるデプロイでの発行(この場合ウィザードは無効です)には、引き続きこの節の手順が唯一の方法です。

APIキーはDBにSHA-256ハッシュ、ユーザーパスワードはargon2ハッシュでのみ保存されます。どちらも手作業のSQLでは発行できないため、管理CLIで行います。

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

`create-tenant`は新規テナントの下に`default`ワークスペースも自動作成します。多くの運用ではテナントごとに1ワークスペースで十分なためです。追加のワークスペースが必要な場合は`create-workspace`を使ってください。平文キーは発行時に一度だけ表示されます。管理コマンドは`DATABASE_URL`の接続ロールで直接DBへアクセスします。これはマイグレーションと同じ管理ロールで、`identity.tenants`/`identity.users`/`identity.tenant_memberships`に書き込める唯一のロールです(アプリ自身の`yorishiro_app`ロールにはこの権限がありません)。

その他の管理コマンド:

| コマンド | 内容 |
|---|---|
| `admin list-tenants` | 全テナントの一覧 |
| `admin create-workspace <tenant-id> <name> [--max-entities <n>]` | テナント配下に追加のワークスペースを作成 |
| `admin list-workspaces <tenant-id>` | テナントのワークスペース一覧 |
| `admin create-user <email> <password> [--display-name <name>]` | 人間のユーザーアカウントを作成 |
| `admin add-member <tenant-id> <user-id> <role>` | ユーザーのテナントへのメンバーシップを追加、または既存のroleを変更(`owner`/`admin`/`member`/`viewer`) |
| `admin list-members <tenant-id>` | テナントのメンバーとそのroleの一覧 |
| `admin create-invite <tenant-id> <email> <role> [--ttl-hours <n>]` | 指定したメールアドレスがテナントに参加するための招待トークンを発行(デフォルトTTL: 7日)。詳細は後述 |
| `admin create-api-key <workspace-id> <scope> [--user <user-id>]` | APIキーを発行。`--user`でメンバーに紐付け可能 |
| `admin list-api-keys <workspace-id>` | キーの一覧(ID・scope・prefix・紐付けユーザー・最終使用日時) |
| `admin revoke-api-key <key-id>` | キーの即時失効(漏洩時など) |
| `admin resync-embeddings <workspace-id>` | embedding未生成のentityを再同期(同期失敗からの回復) |

## 認証とscope

すべてのAPIは`Authorization: Bearer <APIキー>`で認証します。キーは`ysr_`で始まる文字列で、発行時に一度だけ表示されます(DBにはSHA-256ハッシュのみ保存)。

scopeは`read` < `write` < `schema`の3段階です。`write`キーは読み取りもでき、`schema`キーはスキーマ登録を含む全操作ができます。

### キーをユーザーに紐付ける

人間の操作も自動化も、最終的にはすべてAPIキーで認証されます。サーバ側にcookie/セッション状態はありません。ただしキーは人間のユーザーに**紐付け**でき、マルチユーザーのアクセス制御はセッションではなくその紐付けとユーザーのテナントroleを結びつける形で実現しています。

`create-api-key`に`--user <user-id>`を渡すとそのメンバーにキーが紐付き、要求できるscopeは`MembershipRole::max_scope()`で上限が決まります。`owner`/`admin`は`schema`まで、`member`は`write`まで、`viewer`は`read`まで発行可能です。この上限を超えるscopeの要求や、ワークスペースの所属テナントのメンバーでないユーザーへの紐付けは、発行時点で拒否されます。このチェックはキー発行時に一度だけ行われ、キー自体のscopeと同様にリクエストのたびには再評価されません。メンバーシップを剥奪しても、発行済みのキーのscopeは遡って狭まりません。その場合はキー自体を失効させてください。

サービス・自動化用の紐付け不要なキーには`--user`を省略してください。roleによる上限はかかりません。`GET /whoami`はワークスペース・テナント・scopeに加えて、紐付けられた`user_id`(未紐付けなら`null`)も返します。

`POST /auth/login`(後述)は`admin create-api-key --user`のセルフサービス版です。`DATABASE_URL`へのアクセスではなくパスワードで認証し、呼び出し元自身のroleに上限を設定済みのキーを発行します。

## サインアップ・ログイン・メンバー・ワークスペース管理

アカウント作成は招待制のみで、公開・無認証のセルフサインアップはありません。テナントのowner/adminが招待を発行し、招待された人がそれを一度だけ使ってアカウントを作成します。それ以降はメールアドレス/パスワードで認証してAPIキーを取得します。

1. 招待

   テナントのowner/adminがメールアドレスとroleに対して招待トークンを作成します。

   ```console
   $ make admin ARGS="create-invite 019f565d-f1e3-7afb-b876-b7003e43c230 newperson@example.com member"
   invite created (the plaintext token is shown ONLY once — send it now)
     token:      c8b9ea1f...
     ...
     expires at: 2026-07-20 16:57 UTC
   ```

   - 平文の`token`は帯域外(メール・チャット等)で招待された人に送ってください。APIキー同様、表示は一度だけで、DBにはハッシュのみ保存されます。
   - `--ttl-hours`(デフォルト7日)経過時か使用済みになった時点のいずれか早い方で失効します。

2. サインアップ

   招待された人がトークンを使ってアカウントを作成します。

   ```console
   $ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
       -d '{"invite_token":"c8b9ea1f...","password":"a strong password","display_name":"New Person"}'
   {"user_id":"...","email":"newperson@example.com","tenant_id":"...","role":"member",
    "workspaces":[{"id":"...","name":"default"}]}
   ```

   これにより`identity.users`の行が作成され、招待で指定されたメンバーシップも追加されます。同じ(既に消費済みの)トークンでの2回目のサインアップは拒否されます(422)。

3. ログイン

   以降、ユーザーはパスワードと引き換えに新しいAPIキーを取得します。キーは1つのワークスペースにスコープされ、自身のroleの`max_scope()`で上限が設定されます(前述参照)。

   - `workspace_id`は省略可能です。アカウントがちょうど1つのワークスペースにしかアクセスできない場合(既定のコミュニティ版デプロイは常にこれに該当します)は自動解決されます。
   - 複数のワークスペースに所属している場合のみ明示的な指定が必要で、その場合は422が返ります。

   ```console
   $ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
       -d '{"email":"newperson@example.com","password":"a strong password"}'
   {"api_key":"ysr_...","api_key_id":"...","workspace_id":"...","scope":"write","user_id":"..."}
   ```

   ログインのたびに既存キーの再利用ではなく*新しい*キーが発行されます。不要になった古いキーは`admin revoke-api-key`で失効させてください。

4. メンバー管理

   認証後は、テナントのowner/adminは`DATABASE_URL`/管理CLIを使わずRESTでメンバーの一覧・追加ができます。

   ```console
   $ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
   $ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
       -H "Content-Type: application/json" \
       -d '{"email":"existing-user@example.com","role":"admin"}'
   ```

   - `POST /api/members`は既存のアカウント(サインアップ済みのもの)を呼び出し元のテナントに追加するだけで、新規アカウントは作成しません。まだアカウントを持たない人を招き入れるには、代わりに招待(手順1)を発行してください。
   - 両エンドポイントとも、呼び出し元自身のキーがOwner/Adminメンバーに紐付いている必要があります。Memberロールのキーはそのキー自身のscopeに関わらず403で拒否されます。メンバー管理はscopeではなくテナントroleの問題だからです。

5. ワークスペース管理

   同様に、認証済みのメンバーであれば誰でもテナントのワークスペース一覧(エンティティ・リレーション・スキーマの件数を含む)を取得できます。作成・削除はメンバー管理と同じくowner/adminに限定されます。

   ```console
   $ curl localhost:8080/api/workspaces -H "Authorization: Bearer $YSR_KEY"
   $ curl -X POST localhost:8080/api/workspaces -H "Authorization: Bearer $YSR_KEY" \
       -H "Content-Type: application/json" -d '{"name":"staging"}'
   $ curl localhost:8080/api/workspaces/$WORKSPACE_ID -H "Authorization: Bearer $YSR_KEY"
   $ curl -X DELETE localhost:8080/api/workspaces/$WORKSPACE_ID -H "Authorization: Bearer $YSR_KEY"
   ```

   - ワークスペースを削除すると配下の全て(エンティティ・リレーション・スキーマ・APIキー)も削除されます。テナントに残る唯一のワークスペースは削除できません(409)。`DATABASE_URL`へのアクセスなしには代わりのワークスペースを発行する手段がないためです。
   - Web UI(`/`)でもログイン後に同じ作成・一覧・削除・詳細表示の操作ができます。
