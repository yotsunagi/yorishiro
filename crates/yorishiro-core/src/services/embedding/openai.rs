use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::error::{ResultExt, YorishiroError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct OpenAiCompatibleConfig {
    /// Example: `https://api.openai.com/v1` (a trailing `/` is optional).
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    /// Some OpenAI-compatible implementations (vLLM, Ollama, etc.) don't
    /// recognize the `dimensions` parameter, so callers can explicitly choose
    /// whether to include it in the request.
    pub send_dimensions_param: bool,
}

pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
    send_dimensions_param: bool,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client configuration is static and always valid");

        Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key,
            model: config.model,
            dimensions: config.dimensions,
            send_dimensions_param: config.send_dimensions_param,
        }
    }
}

#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleProvider {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&EmbeddingsRequest {
                model: &self.model,
                input: texts,
                dimensions: self.send_dimensions_param.then_some(self.dimensions),
            })
            .send()
            .await
            .internal()?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned HTTP {status}: {body}"
            )));
        }

        let parsed: EmbeddingsResponse = response.json().await.internal()?;

        let vectors: Vec<Vec<f32>> = parsed.data.into_iter().map(|d| d.embedding).collect();

        if vectors.len() != texts.len() {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned {} vectors for {} inputs",
                vectors.len(),
                texts.len()
            )));
        }

        for vector in &vectors {
            if vector.len() != self.dimensions {
                return Err(YorishiroError::Internal(anyhow::anyhow!(
                    "embedding provider returned a vector of length {} but expected {}",
                    vector.len(),
                    self.dimensions
                )));
            }
        }

        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn provider(base_url: String) -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "test-key".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 3,
            send_dimensions_param: true,
        })
    }

    #[tokio::test]
    async fn embeds_a_batch_of_texts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] },
                    { "embedding": [0.4, 0.5, 0.6] }
                ]
            })))
            .mount(&server)
            .await;

        let provider = provider(server.uri());
        let vectors = provider.embed_batch(&["hello", "world"]).await.unwrap();

        assert_eq!(vectors, vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]]);
    }

    #[tokio::test]
    async fn embed_delegates_to_embed_batch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": [1.0, 2.0, 3.0] }]
            })))
            .mount(&server)
            .await;

        let provider = provider(server.uri());
        let vector = provider.embed("hello").await.unwrap();
        assert_eq!(vector, vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn empty_batch_short_circuits_without_a_request() {
        let server = MockServer::start().await;
        // `expect(0)` means an actual request would panic when the mock server
        // is dropped, catching a regression here.
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let provider = provider(server.uri());
        let vectors = provider.embed_batch(&[]).await.unwrap();
        assert!(vectors.is_empty());
    }

    #[tokio::test]
    async fn omits_dimensions_param_when_disabled() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(wiremock::matchers::body_json(json!({
                "model": "text-embedding-3-small",
                "input": ["hello"]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": [0.1, 0.2, 0.3] }]
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.uri(),
            api_key: "test-key".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 3,
            send_dimensions_param: false,
        });

        let vectors = provider.embed_batch(&["hello"]).await.unwrap();
        assert_eq!(vectors, vec![vec![0.1, 0.2, 0.3]]);
    }

    #[tokio::test]
    async fn rejects_mismatched_vector_dimensions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": [0.1, 0.2] }]
            })))
            .mount(&server)
            .await;

        let provider = provider(server.uri());
        let err = provider.embed_batch(&["hello"]).await.unwrap_err();
        assert!(matches!(err, YorishiroError::Internal(_)));
    }

    #[tokio::test]
    async fn rejects_non_success_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let provider = provider(server.uri());
        let err = provider.embed_batch(&["hello"]).await.unwrap_err();
        assert!(matches!(err, YorishiroError::Internal(_)));
    }
}
