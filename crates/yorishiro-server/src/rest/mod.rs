mod entities;
mod relations;
mod schemas;
mod search;

use axum::Router;
use axum::routing::{get, post};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::state::AppState;

/// APIキーをBearerトークンとして送る単一のスキームを`bearer_auth`という名前で登録する。
/// 個々の`#[utoipa::path]`には`security(...)`を付けず、ここでの登録とApiDoc側の
/// トップレベル`security`属性によって全エンドポイントへ一括適用する。
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
        (name = "entities", description = "エンティティ（FR-2）"),
        (name = "relations", description = "リレーション（FR-3）"),
        (name = "schemas", description = "メタスキーマ（FR-1）"),
        (name = "search", description = "ベクトル類似検索（FR-4）"),
    ),
    info(
        title = "Yorishiro（依り代）API",
        description = "ユーザー定義スキーマ・MCPネイティブなナレッジストアのREST API",
    ),
)]
pub struct ApiDoc;

/// REST APIのルーティング。`state`を渡さず`Router<AppState>`のまま返すことで、
/// `main.rs`側でMCPルートやSwaggerUiと合流させたうえで最後に`with_state`できるようにする。
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
