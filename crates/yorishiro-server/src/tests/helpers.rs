use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sea_query::{Alias, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::tenancy;
use yorishiro_core::services::auth::create_api_key;

use crate::*;
use yorishiro_core::db::TenantDb;

#[derive(Iden)]
enum Tenants {
    Table,
    Id,
    Name,
}

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
    Name,
}

/// Tests shouldn't call out to a remote embeddings service, so this dummy provider only
/// satisfies the dimension count (and errors immediately if actually invoked).
pub(super) struct UnreachableEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for UnreachableEmbeddingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::Internal(anyhow::anyhow!(
            "embedding provider should not be called in this test"
        )))
    }
}

pub(super) fn test_state(pool: PgPool) -> AppState {
    AppState::new(
        TenantDb::new(pool.clone()),
        pool,
        Arc::new(UnreachableEmbeddingProvider),
    )
}

/// A provider that returns a deterministic vector, for end-to-end tests of the embedding
/// wiring. Every text maps to the same vector, so the distance between query and entity
/// is always 0 — guaranteeing a hit.
pub(super) struct FixedEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for FixedEmbeddingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts.iter().map(|_| vec![0.1_f32; 768]).collect())
    }
}

pub(super) async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Tenants::Table))
        .columns([Tenants::Name])
        .values_panic(["test-tenant".into()])
        .returning(Query::returning().columns([Tenants::Id]))
        .build_sqlx(PostgresQueryBuilder);
    let (tenant_id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .unwrap();

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Workspaces::Table))
        .columns([Workspaces::TenantId, Workspaces::Name])
        .values_panic([tenant_id.into(), "test-workspace".into()])
        .returning(Query::returning().columns([Workspaces::Id]))
        .build_sqlx(PostgresQueryBuilder);
    let (workspace_id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .unwrap();
    (tenant_id, workspace_id)
}

/// Extracts the `data: {...}` line from a `text/event-stream` body and parses it as JSON.
/// streamable-http returns multiple events separated by `\n\n`, but the response to a
/// single request is carried in the last one, so that's the one targeted.
pub(super) fn parse_sse_json(body: &str) -> serde_json::Value {
    body.split("\n\n")
        .filter_map(|event| event.lines().find_map(|line| line.strip_prefix("data: ")))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .last()
        .unwrap_or_else(|| panic!("no `data:` line found in SSE body: {body:?}"))
}

pub(super) async fn mcp_post(
    app: &Router,
    session_id: Option<&str>,
    auth_header: Option<&str>,
    body: serde_json::Value,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");

    if let Some(session_id) = session_id {
        builder = builder.header("mcp-session-id", session_id);
    }
    if let Some(auth_header) = auth_header {
        builder = builder.header("authorization", auth_header);
    }

    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

/// Performs the initialize + notifications/initialized handshake and returns the
/// session ID to use for subsequent tools/call requests.
pub(super) async fn mcp_handshake(app: &Router) -> String {
    let response = mcp_post(
        app,
        None,
        None,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "yorishiro-test", "version": "0.0.0" },
            },
        }),
    )
    .await;
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        panic!(
            "initialize failed: {status} {}",
            String::from_utf8_lossy(&body)
        );
    }
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .expect("initialize response must carry Mcp-Session-Id")
        .to_str()
        .unwrap()
        .to_string();

    let response = mcp_post(
        app,
        Some(&session_id),
        None,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    session_id
}

/// Fills each tool's required arguments with dummy values that only satisfy their types.
/// The authorization check runs after argument deserialization, so for this test's goal
/// (catching missing authorization checks) to hold, the arguments themselves must
/// already be well-formed.
pub(super) fn dummy_arguments_for_tool(name: &str) -> serde_json::Value {
    const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";
    match name {
        "create_entity" => serde_json::json!({
            "schema_name": "dummy", "entity_type": "dummy", "data": {},
        }),
        "get_entity" => serde_json::json!({ "id": NIL_UUID }),
        "update_entity" => serde_json::json!({ "id": NIL_UUID, "data": {} }),
        "delete_entity" => serde_json::json!({ "id": NIL_UUID }),
        "list_entities" => serde_json::json!({}),
        "create_relation" => serde_json::json!({
            "source_id": NIL_UUID, "target_id": NIL_UUID, "relation_type": "dummy",
        }),
        "get_relation" => serde_json::json!({ "id": NIL_UUID }),
        "delete_relation" => serde_json::json!({ "id": NIL_UUID }),
        "list_relations" => serde_json::json!({}),
        "list_schemas" => serde_json::json!({}),
        "get_active_schema" => serde_json::json!({ "name": "dummy" }),
        "get_schema_by_id" => serde_json::json!({ "schema_id": NIL_UUID }),
        "create_schema" => serde_json::json!({ "definition": {} }),
        "get_entity_type_json_schema" => serde_json::json!({
            "schema_name": "dummy", "entity_type": "dummy",
        }),
        "search_entities" => serde_json::json!({ "query_text": "dummy" }),
        "recall_context" => serde_json::json!({ "entity_id": NIL_UUID }),
        "list_templates" => serde_json::json!({}),
        other => panic!("no dummy arguments registered for tool `{other}`"),
    }
}

pub(super) async fn rest_request(
    app: &Router,
    method: &str,
    uri: &str,
    auth_header: Option<&str>,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(auth_header) = auth_header {
        builder = builder.header("authorization", auth_header);
    }
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };

    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

pub(super) async fn rest_json_body(response: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub(super) async fn seed_task_and_project(
    app: &Router,
    schema_auth: &str,
    write_auth: &str,
) -> (String, String) {
    let response = rest_request(
        app,
        "POST",
        "/api/schemas",
        Some(schema_auth),
        Some(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } },
                "project": { "fields": { "name": { "type": "string", "required": true } } }
            },
            "relation_types": {
                "belongs_to": { "source": "task", "target": "project" }
            },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(
        app,
        "POST",
        "/api/entities",
        Some(write_auth),
        Some(serde_json::json!({
            "schema_name": "task-management",
            "entity_type": "task",
            "data": { "title": "buy milk" },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let task_id = rest_json_body(response).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = rest_request(
        app,
        "POST",
        "/api/entities",
        Some(write_auth),
        Some(serde_json::json!({
            "schema_name": "task-management",
            "entity_type": "project",
            "data": { "name": "groceries" },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let project_id = rest_json_body(response).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    (task_id, project_id)
}

/// Issues an API key attributed to `user_id`, scoped to `role`'s max scope -- exactly what
/// `/auth/login` would hand out for that role.
pub(super) async fn issue_key_for(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    role: tenancy::MembershipRole,
) -> String {
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    create_api_key(&mut conn, workspace_id, role.max_scope(), Some(user_id))
        .await
        .unwrap()
        .plaintext
}
