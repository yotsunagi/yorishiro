use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[derive(Clone)]
pub struct TenantDb {
    pool: PgPool,
}

impl TenantDb {
    /// 生プールをそのまま包む。呼び出し側は、コネクションがプールに返却される際に
    /// `app.current_tenant`がリセットされることを別途保証しなければならない
    /// （本番用途では`connect`を使うこと）。ロール切り替えも行わないため、
    /// `pool`の接続ロールがRLSをバイパスする権限を持つ場合はテナント分離が
    /// 機能しない点に注意（マイグレーション実行やテストでの直接利用を想定）。
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 本番用のプールを構築する。`after_connect`フックで物理接続ごとに一度
    /// `SET ROLE`を発行し、RLSをバイパスできない`yorishiro_app`ロールとして
    /// 以降の全クエリを実行させる（`database_url`のログインロール自体は
    /// superuserのままでよい。superuserはメンバーシップなしで任意ロールへ
    /// `SET ROLE`できるため）。`after_release`フックでは`app.current_tenant`を
    /// 確実にリセットしてからコネクションをプールへ返却し、あるテナントの
    /// セッション状態が別テナントの借用者へ漏洩することを防ぐ。
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .after_release(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("RESET app.current_tenant")
                        .execute(&mut *conn)
                        .await?;
                    Ok(true)
                })
            })
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// RLSがテナントを分離できるよう、このコネクション上のセッション変数
    /// `app.current_tenant`を設定する。
    ///
    /// `is_local=false`（セッションレベル）で設定する点が重要: `is_local=true`
    /// （`SET LOCAL`相当）は明示的トランザクションの外では単文の暗黙トランザクション
    /// 終了と同時に破棄されてしまい、後続のクエリではRLSのポリシーが
    /// `current_setting('app.current_tenant')`を空文字列として評価してしまう
    /// （＝テナント分離が機能しない）。
    pub async fn acquire_for_tenant(
        &self,
        tenant_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
            .bind(tenant_id.to_string())
            .execute(conn.as_mut())
            .await?;
        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use sqlx::Row;

    use super::*;

    /// `sqlx::test`が渡すpoolはマイグレーションを実行した管理ロール（superuser）
    /// で接続されているため、`TenantDb::new`はRLSを実効させない。ここでは
    /// `SET ROLE`で明示的に`yorishiro_app`へ切り替え、RLSが実際にクロステナント
    /// アクセスを遮断することを検証する（`TenantDb::connect`が本番で行う切り替えの
    /// 効果そのものを裏取りするテスト）。
    #[sqlx::test(migrations = "../../migrations")]
    async fn rls_blocks_cross_tenant_access_under_restricted_role(pool: PgPool) {
        let (tenant_a,): (Uuid,) =
            sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
                .bind("tenant-a")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO tenants (name) VALUES ($1)")
            .bind("tenant-b")
            .execute(&pool)
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("SET ROLE yorishiro_app")
            .execute(conn.as_mut())
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
            .bind(tenant_a.to_string())
            .execute(conn.as_mut())
            .await
            .unwrap();

        let rows = sqlx::query("SELECT name FROM tenants")
            .fetch_all(conn.as_mut())
            .await
            .unwrap();
        let names: Vec<String> = rows.iter().map(|row| row.get("name")).collect();

        assert_eq!(names, vec!["tenant-a".to_string()]);
    }
}
