mod entities;
mod export;
mod relations;
mod schemas;
mod search;

use axum::Router;
use axum::routing::{get, post};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use yorishiro_core::YorishiroError;

use crate::state::AppState;

/// Parses a JSON-object query parameter (e.g. `?filter={"status":"active"}`) shared by the
/// `entities` and `search` list endpoints. `None`/empty input means "no filter".
pub(crate) fn parse_filter_param(
    raw: Option<String>,
) -> Result<Option<serde_json::Value>, YorishiroError> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    serde_json::from_str(&raw).map_err(|err| YorishiroError::ValidationFailed {
        message: "filter is not valid JSON".into(),
        details: vec![],
        hint: format!("filter must be a JSON object, e.g. {{\"status\":\"active\"}}: {err}"),
    })
}

/// Registers a single scheme named `bearer_auth` for sending the API key as a
/// Bearer token. Individual `#[utoipa::path]` items don't carry `security(...)`;
/// this registration plus `ApiDoc`'s top-level `security` attribute apply it to
/// every endpoint at once.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("yorishiro-api-key")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        entities::create_entity,
        entities::get_entity,
        entities::update_entity,
        entities::delete_entity,
        entities::list_entities,
        entities::get_entity_context,
        relations::create_relation,
        relations::get_relation,
        relations::delete_relation,
        relations::list_relations,
        schemas::list_schemas,
        schemas::get_active_schema,
        schemas::get_schema_by_id,
        schemas::create_schema,
        schemas::get_entity_type_json_schema,
        schemas::list_templates,
        search::search_entities,
        export::export_jsonl,
    ),
    components(schemas(
        entities::CreateEntityRequest,
        entities::UpdateEntityRequest,
        relations::CreateRelationRequest,
        schemas::CreateSchemaResponse,
        schemas::CreateSchemaRequest,
    )),
    modifiers(&SecurityAddon),
    security(("bearer_auth" = [])),
    tags(
        (name = "entities", description = "Entity operations"),
        (name = "relations", description = "Relation operations"),
        (name = "schemas", description = "Meta-schema operations"),
        (name = "search", description = "Vector similarity search"),
        (name = "export", description = "Bulk data export"),
    ),
    info(
        title = "Yorishiro API",
        description = "REST API for a user-defined-schema, MCP-native knowledge store",
    ),
)]
pub struct ApiDoc;

/// REST API routing. Returned as `Router<AppState>` without state applied, so
/// that `main.rs` can merge in the MCP routes and SwaggerUi before calling
/// `with_state` at the end.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/entities",
            post(entities::create_entity).get(entities::list_entities),
        )
        .route(
            "/api/entities/{id}",
            get(entities::get_entity)
                .put(entities::update_entity)
                .delete(entities::delete_entity),
        )
        .route(
            "/api/entities/{id}/context",
            get(entities::get_entity_context),
        )
        .route(
            "/api/relations",
            post(relations::create_relation).get(relations::list_relations),
        )
        .route(
            "/api/relations/{id}",
            get(relations::get_relation).delete(relations::delete_relation),
        )
        .route(
            "/api/schemas",
            post(schemas::create_schema).get(schemas::list_schemas),
        )
        .route(
            "/api/schemas/active/{name}",
            get(schemas::get_active_schema),
        )
        .route(
            "/api/schemas/active/{name}/entity-types/{entity_type}/json-schema",
            get(schemas::get_entity_type_json_schema),
        )
        .route("/api/schemas/{schema_id}", get(schemas::get_schema_by_id))
        .route("/api/templates", get(schemas::list_templates))
        .route("/api/search", get(search::search_entities))
        .route("/api/export.jsonl", get(export::export_jsonl))
}
