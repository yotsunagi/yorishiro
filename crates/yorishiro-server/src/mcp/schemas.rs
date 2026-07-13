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
use yorishiro_core::YorishiroError;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::metaschema::{self, MetaSchemaDefinition};
use yorishiro_core::schemas;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetActiveSchemaArgs {
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSchemaByIdArgs {
    pub schema_id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateSchemaArgs {
    /// `MetaSchemaDefinition`（name/description/entity_types/relation_types）に
    /// 従うJSONオブジェクト。同名のスキーマが既に存在する場合は非破壊/破壊的変更を
    /// 自動判定した上で新バージョンとして登録される。
    pub definition: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEntityTypeJsonSchemaArgs {
    /// アクティブなスキーマの名前。
    pub schema_name: String,
    /// そのスキーマ内のentity_type名。
    pub entity_type: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_schemas)]
impl YorishiroMcpServer {
    #[tool(
        description = "テナントに登録済みの全スキーマ（全バージョン、archived含む）のサマリを一覧する。どんなスキーマがあるかの発見に使う（read scope必須）"
    )]
    pub async fn list_schemas(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::list(authorized.conn(), tenant_id).await {
            Ok(summaries) => ok_json(summaries),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "名前を指定して現在アクティブなスキーマ定義を取得する（read scope必須）")]
    pub async fn get_active_schema(
        &self,
        Parameters(args): Parameters<GetActiveSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::get_active_schema(authorized.conn(), tenant_id, &args.name).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "IDを指定して特定バージョンのスキーマ定義を取得する（read scope必須）")]
    pub async fn get_schema_by_id(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::get_by_id(authorized.conn(), tenant_id, args.schema_id).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(
        description = "新しいスキーマを登録する、または既存スキーマの新バージョンを追加する\
                           （schema scope必須）"
    )]
    pub async fn create_schema(
        &self,
        Parameters(args): Parameters<CreateSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Schema).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let definition: MetaSchemaDefinition = match serde_json::from_value(args.definition) {
            Ok(definition) => definition,
            Err(err) => {
                return Ok(err_to_tool_result(YorishiroError::ValidationFailed {
                    message: format!("invalid schema definition: {err}"),
                    details: vec![],
                    hint:
                        "MetaSchemaDefinitionの構造（name/description/entity_types/relation_types）\
                           を確認してください"
                            .into(),
                }));
            }
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::create_schema(authorized.conn(), tenant_id, definition).await {
            Ok((record, diff)) => ok_json(serde_json::json!({
                "schema": record,
                "diff": diff,
            })),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(
        description = "アクティブなスキーマ内の特定entity_typeを、JSON Schemaとして取得する\
                           （read scope必須）。フィールドの型・必須・enum等をエージェントが\
                           事前に把握するために使う。"
    )]
    pub async fn get_entity_type_json_schema(
        &self,
        Parameters(args): Parameters<GetEntityTypeJsonSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        let record =
            match schemas::get_active_schema(authorized.conn(), tenant_id, &args.schema_name).await
            {
                Ok(record) => record,
                Err(err) => return Ok(err_to_tool_result(err)),
            };

        match record.definition.entity_types.get(&args.entity_type) {
            Some(entity_type_def) => {
                ok_json(metaschema::entity_type_to_json_schema(entity_type_def))
            }
            None => Ok(err_to_tool_result(YorishiroError::NotFound {
                message: format!(
                    "entity_type '{}' not found in schema '{}'",
                    args.entity_type, args.schema_name
                ),
            })),
        }
    }
}
