use axum::http::header;
use axum::response::IntoResponse;
use yorishiro_core::YorishiroError;
use yorishiro_core::export;

use crate::auth::{Authorized, ReadScope};
use crate::error::ApiError;

/// Line-delimited JSON export of every schema, entity, and relation belonging to the
/// tenant, one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
#[utoipa::path(
    get,
    path = "/api/export.jsonl",
    responses(
        (status = 200, description = "JSON Lines export of every schema, entity, and relation for the tenant", content_type = "application/x-ndjson"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "export",
)]
pub async fn export_jsonl(
    mut authorized: Authorized<ReadScope>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let records = export::export_all(authorized.conn(), workspace_id).await?;

    let mut body = Vec::new();
    for record in &records {
        serde_json::to_writer(&mut body, record)
            .map_err(|err| YorishiroError::Internal(err.into()))?;
        body.push(b'\n');
    }

    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body))
}
