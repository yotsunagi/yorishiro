use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;
use yorishiro_core::YorishiroError;
use yorishiro_core::error::ValidationDetail;

/// `YorishiroError`をHTTPレスポンスへ変換する薄いラッパー。軸となる分類は
/// 「クライアント起因（4xx、詳細を返してよい）」と「内部起因（5xx、詳細は
/// ログにのみ出しクライアントには漏らさない）」の2つ。
pub struct ApiError(pub YorishiroError);

impl From<YorishiroError> for ApiError {
    fn from(err: YorishiroError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self.0 {
            YorishiroError::ValidationFailed {
                message,
                details,
                hint,
            } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": { "message": message, "details": details, "hint": hint } }),
            ),
            YorishiroError::NotFound { message } => (
                StatusCode::NOT_FOUND,
                json!({ "error": { "message": message } }),
            ),
            YorishiroError::ScopeInsufficient { message, hint } => (
                StatusCode::FORBIDDEN,
                json!({ "error": { "message": message, "hint": hint } }),
            ),
            YorishiroError::Conflict { message } => (
                StatusCode::CONFLICT,
                json!({ "error": { "message": message } }),
            ),
            YorishiroError::RelationTypeMismatch { message } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": { "message": message } }),
            ),
            YorishiroError::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                json!({ "error": { "message": "authentication required" } }),
            ),
            YorishiroError::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": { "message": "internal server error" } }),
                )
            }
        };
        (status, Json(body)).into_response()
    }
}

/// OpenAPIドキュメント上でエラーレスポンスの形を表現するためのDTO。
/// 実際のレスポンスボディは`ApiError::into_response`が個別に組み立てるため、
/// このデータの値自体が使われることはなくスキーマ定義専用として存在する。
#[derive(Serialize, ToSchema)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ApiErrorDetail {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<ValidationDetail>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}
