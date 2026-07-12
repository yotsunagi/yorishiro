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
use yorishiro_core::relations;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateRelationArgs {
    pub source_id: Uuid,
    pub target_id: Uuid,
    /// スキーマの`relation_types`定義で宣言されたrelation_type名。
    pub relation_type: String,
    /// リレーションに付随する任意のプロパティ（JSONオブジェクト、省略時は空オブジェクト）。
    pub properties: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRelationArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteRelationArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRelationsArgs {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// 最大件数（省略時は50）。
    pub limit: Option<i64>,
    /// スキップする件数（省略時は0）。
    pub offset: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_relations)]
impl YorishiroMcpServer {
    #[tool(
        description = "2つのエンティティ間にリレーションを作成する（write scope必須）。\
                           updateは提供されないため、変更する場合は削除して作り直すこと。"
    )]
    pub async fn create_relation(
        &self,
        Parameters(args): Parameters<CreateRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let input = relations::CreateRelationInput {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            properties: args.properties.unwrap_or_else(|| serde_json::json!({})),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match relations::create(authorized.conn(), tenant_id, input).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "IDを指定してリレーションを1件取得する（read scope必須）")]
    pub async fn get_relation(
        &self,
        Parameters(args): Parameters<GetRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match relations::get(authorized.conn(), tenant_id, args.id).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "リレーションを削除する（write scope必須）")]
    pub async fn delete_relation(
        &self,
        Parameters(args): Parameters<DeleteRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match relations::delete(authorized.conn(), tenant_id, args.id).await {
            Ok(()) => ok_json(serde_json::json!({ "deleted": true })),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "リレーションを一覧取得する（read scope必須）")]
    pub async fn list_relations(
        &self,
        Parameters(args): Parameters<ListRelationsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let default = relations::ListRelationsQuery::default();
        let query = relations::ListRelationsQuery {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            limit: args.limit.unwrap_or(default.limit),
            offset: args.offset.unwrap_or(default.offset),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match relations::list(authorized.conn(), tenant_id, query).await {
            Ok(records) => ok_json(records),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
