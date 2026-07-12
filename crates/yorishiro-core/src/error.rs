use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, thiserror::Error)]
pub enum YorishiroError {
    #[error("validation failed: {message}")]
    ValidationFailed {
        message: String,
        details: Vec<ValidationDetail>,
        hint: String,
    },

    #[error("not found: {message}")]
    NotFound { message: String },

    #[error("scope insufficient: {message}")]
    ScopeInsufficient { message: String, hint: String },

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("relation type mismatch: {message}")]
    RelationTypeMismatch { message: String },

    #[error("unauthenticated")]
    Unauthenticated,

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ValidationDetail {
    pub field: String,
    pub problem: String,
}
