mod entities;
mod relations;
mod schemas;
mod search;

use axum::Router;
use axum::routing::{get, post};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::state::AppState;

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
        relations::create_relation,
        relations::get_relation,
        relations::delete_relation,
        relations::list_relations,
        schemas::list_schemas,
        schemas::get_active_schema,
        schemas::get_schema_by_id,
        schemas::create_schema,
        schemas::get_entity_type_json_schema,
        search::search_entities,
    ),
    components(schemas(
        entities::CreateEntityRequest,
        entities::UpdateEntityRequest,
        relations::CreateRelationRequest,
        schemas::CreateSchemaResponse,
    )),
    modifiers(&SecurityAddon),
    security(("bearer_auth" = [])),
    tags(
        (name = "entities", description = "Entity operations"),
        (name = "relations", description = "Relation operations"),
        (name = "schemas", description = "Meta-schema operations"),
        (name = "search", description = "Vector similarity search"),
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
        .route("/api/search", get(search::search_entities))
}
