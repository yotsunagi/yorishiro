use sea_query::{Alias, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

use super::{
    ApiKeyScope, ApiKeys, CreatedApiKey, KEY_PREFIX_BYTES, KEY_SECRET_BYTES, hash_key, random_hex,
};

/// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part
/// (192 bits) is the actual credential. SHA-256 is sufficient here rather than a slow KDF
/// like bcrypt/argon2, since API keys already carry enough entropy that offline
/// brute-forcing isn't a realistic threat.
pub async fn create_api_key(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    scope: ApiKeyScope,
    user_id: Option<Uuid>,
) -> Result<CreatedApiKey, YorishiroError> {
    let prefix = format!("ysr_{}", random_hex(KEY_PREFIX_BYTES));
    let secret = random_hex(KEY_SECRET_BYTES);
    let plaintext = format!("{prefix}_{secret}");
    let key_hash = hash_key(&plaintext);

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), ApiKeys::Table))
        .columns([
            ApiKeys::WorkspaceId,
            ApiKeys::KeyHash,
            ApiKeys::KeyPrefix,
            ApiKeys::Scope,
            ApiKeys::UserId,
        ])
        .values_panic([
            workspace_id.into(),
            key_hash.into(),
            prefix.into(),
            scope.as_db_str().into(),
            user_id.into(),
        ])
        .returning(Query::returning().columns([ApiKeys::Id]))
        .build_sqlx(PostgresQueryBuilder);

    let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;

    Ok(CreatedApiKey {
        id,
        workspace_id,
        scope,
        user_id,
        plaintext,
    })
}
