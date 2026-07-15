//! Domain logic that isn't itself a record's CRUD: API-key auth/authorization and the
//! embeddings pipeline (provider abstraction, ONNX/OpenAI-compatible implementations, and the
//! sync job that keeps `entities.embedding` current).

pub mod auth;
pub mod embedding;
pub mod embedding_onnx;
pub mod embedding_sync;
