use anyhow::{Context, Result, bail};
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::auth::{self, ApiKeyScope, CreatedApiKey};

/// 管理サブコマンドの入口。サーバ起動（引数なし）とは異なり、DATABASE_URLの
/// 接続ロール（マイグレーションを実行できる管理ロール）で直接DBを操作する。
/// APIキーはDBにSHA-256ハッシュでしか保存されないため、キー発行はSQLの手作業では
/// 行えず、このCLIが唯一のブートストラップ手段になる。
pub async fn run(args: &[String]) -> Result<()> {
    let usage = "usage:\n  \
        yorishiro-server admin create-tenant <name>\n  \
        yorishiro-server admin create-api-key <tenant-id> <read|write|schema>\n  \
        yorishiro-server admin list-tenants";

    let [command, rest @ ..] = args else {
        bail!("{usage}");
    };
    if command != "admin" {
        bail!("unknown command '{command}'\n{usage}");
    }

    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL must be set for admin commands")?;
    let pool = PgPool::connect(&database_url)
        .await
        .context("failed to connect to database")?;
    // 初回セットアップ（サーバをまだ一度も起動していないDB）でも動くよう、
    // サーバ起動時と同じマイグレーションを適用しておく（適用済みならno-op）。
    sqlx::migrate!("../../migrations").run(&pool).await?;

    match rest {
        [sub, name] if sub == "create-tenant" => {
            let tenant_id = create_tenant(&pool, name).await?;
            println!("tenant created");
            println!("  id:   {tenant_id}");
            println!("  name: {name}");
        }
        [sub, tenant_id, scope] if sub == "create-api-key" => {
            let tenant_id: Uuid = tenant_id
                .parse()
                .context("tenant-id must be a UUID (see `admin list-tenants`)")?;
            let scope = parse_scope(scope)?;
            let created = create_api_key(&pool, tenant_id, scope).await?;
            println!("api key created (the plaintext key is shown ONLY once — store it now)");
            println!("  key:       {}", created.plaintext);
            println!("  key id:    {}", created.id);
            println!("  tenant id: {}", created.tenant_id);
            println!("  scope:     {scope:?}");
        }
        [sub] if sub == "list-tenants" => {
            let tenants = list_tenants(&pool).await?;
            if tenants.is_empty() {
                println!("no tenants (create one with `admin create-tenant <name>`)");
            }
            for (id, name) in tenants {
                println!("{id}  {name}");
            }
        }
        _ => bail!("{usage}"),
    }

    Ok(())
}

fn parse_scope(s: &str) -> Result<ApiKeyScope> {
    match s {
        "read" => Ok(ApiKeyScope::Read),
        "write" => Ok(ApiKeyScope::Write),
        "schema" => Ok(ApiKeyScope::Schema),
        other => bail!("unknown scope '{other}' (expected read, write, or schema)"),
    }
}

async fn create_tenant(pool: &PgPool, name: &str) -> Result<Uuid> {
    let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .context("failed to create tenant")?;
    Ok(id)
}

async fn create_api_key(
    pool: &PgPool,
    tenant_id: Uuid,
    scope: ApiKeyScope,
) -> Result<CreatedApiKey> {
    // 先にテナントの存在を確かめ、FK違反より分かりやすいエラーにする。
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM tenants WHERE id = $1")
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        bail!("tenant '{tenant_id}' does not exist (see `admin list-tenants`)");
    }

    let mut conn = pool.acquire().await?;
    let created = auth::create_api_key(&mut conn, tenant_id, scope)
        .await
        .context("failed to create api key")?;
    Ok(created)
}

async fn list_tenants(pool: &PgPool) -> Result<Vec<(Uuid, String)>> {
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM tenants ORDER BY created_at")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_tenant_and_issues_a_usable_key(pool: PgPool) {
        let tenant_id = create_tenant(&pool, "bootstrap-tenant").await.unwrap();

        let created = create_api_key(&pool, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        assert_eq!(created.tenant_id, tenant_id);
        assert!(created.plaintext.starts_with("ysr_"));

        // 発行されたキーが実際に認証を通ることまで確かめる。
        let ctx = auth::authenticate(&pool, &created.plaintext).await.unwrap();
        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.scope, ApiKeyScope::Write);

        let tenants = list_tenants(&pool).await.unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].1, "bootstrap-tenant");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_key_creation_for_unknown_tenant(pool: PgPool) {
        let result = create_api_key(&pool, Uuid::nil(), ApiKeyScope::Read).await;
        let Err(err) = result else {
            panic!("key creation should fail for an unknown tenant");
        };
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn parses_scopes() {
        assert!(matches!(parse_scope("read"), Ok(ApiKeyScope::Read)));
        assert!(matches!(parse_scope("write"), Ok(ApiKeyScope::Write)));
        assert!(matches!(parse_scope("schema"), Ok(ApiKeyScope::Schema)));
        assert!(parse_scope("admin").is_err());
    }
}
