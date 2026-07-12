use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::entities;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateEntityArgs {
    /// エンティティが従うスキーマの名前。そのテナントの現在アクティブなバージョンが使われる。
    pub schema_name: String,
    /// スキーマ内で定義されたentity_type名。
    pub entity_type: String,
    /// スキーマの`fields`定義に従ったエンティティ本体（JSONオブジェクト）。
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEntityArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateEntityArgs {
    pub id: Uuid,
    /// 置き換え後のエンティティ本体。作成時点のスキーマバージョンに対して検証される。
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteEntityArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListEntitiesArgs {
    /// 指定した場合、このentity_typeのエンティティのみに絞り込む。
    pub entity_type: Option<String>,
    /// 最大件数（省略時は50）。
    pub limit: Option<i64>,
    /// スキップする件数（省略時は0）。
    pub offset: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_entities)]
impl YorishiroMcpServer {
    #[tool(description = "新しいエンティティを作成する（write scope必須）")]
    pub async fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let input = entities::CreateEntityInput {
            schema_name: args.schema_name,
            entity_type: args.entity_type,
            data: args.data,
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::create(authorized.conn(), tenant_id, input).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "IDを指定してエンティティを1件取得する（read scope必須）")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::get(authorized.conn(), tenant_id, args.id).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "既存のエンティティのdataを置き換える（write scope必須）")]
    pub async fn update_entity(
        &self,
        Parameters(args): Parameters<UpdateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::update(authorized.conn(), tenant_id, args.id, args.data).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "エンティティを削除する（write scope必須）")]
    pub async fn delete_entity(
        &self,
        Parameters(args): Parameters<DeleteEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::delete(authorized.conn(), tenant_id, args.id).await {
            Ok(()) => ok_json(serde_json::json!({ "deleted": true })),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "エンティティを一覧取得する（read scope必須）")]
    pub async fn list_entities(
        &self,
        Parameters(args): Parameters<ListEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let default = entities::ListEntitiesQuery::default();
        let query = entities::ListEntitiesQuery {
            entity_type: args.entity_type,
            limit: args.limit.unwrap_or(default.limit),
            offset: args.offset.unwrap_or(default.offset),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::list(authorized.conn(), tenant_id, query).await {
            Ok(records) => ok_json(records),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
