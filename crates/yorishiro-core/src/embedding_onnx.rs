use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use crate::embedding::EmbeddingProvider;
use crate::error::YorishiroError;

pub struct LocalOnnxConfig {
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    /// 期待する出力次元。`entities.embedding`は`vector(768)`固定のため通常768。
    /// ロード時にプローブ推論を行い、モデルの実際の出力次元と一致しない場合は
    /// 起動を失敗させる。
    pub dimensions: usize,
    /// トークナイズ時の最大シーケンス長。これを超えるテキストは切り詰められる。
    pub max_sequence_length: usize,
}

/// ローカルのONNXモデル（BERT系エンコーダ）で埋め込みを生成するプロバイダ。
/// 外部サービスへの依存なしで動くため、閉域環境やオフライン開発で使う。
///
/// モデルへの要求:
/// - 入力: `input_ids`と`attention_mask`（いずれもint64）。`token_type_ids`は
///   モデルが宣言している場合のみ渡す。
/// - 出力: 先頭の出力が`[batch, seq, hidden]`のlast_hidden_state相当であること。
///   sentence-transformers系のONNXエクスポートはこの形に従う。
///
/// トークン埋め込みはattention maskで重み付けしたmean poolingで文ベクトルへ集約し、
/// コサイン距離での検索を安定させるためL2正規化して返す。
pub struct LocalOnnxProvider {
    // `Session::run`が`&mut self`を要求するためMutexで直列化する。推論自体も
    // intra-op並列でCPUコアを使うため、プロセス内で推論を直列化することは
    // スループット上の実害が小さい。
    inner: Arc<Inner>,
}

struct Inner {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    dimensions: usize,
    needs_token_type_ids: bool,
    output_name: String,
}

fn internal(message: impl std::fmt::Display) -> YorishiroError {
    YorishiroError::Internal(anyhow::anyhow!("{message}"))
}

impl LocalOnnxProvider {
    /// モデルとトークナイザをファイルから読み込み、プローブ推論で出力次元を検証する。
    /// 数百ms〜数秒かかるブロッキング処理なので、起動時に一度だけ呼ぶこと。
    pub fn load(config: LocalOnnxConfig) -> Result<Self, YorishiroError> {
        let mut tokenizer = Tokenizer::from_file(&config.tokenizer_path).map_err(|err| {
            internal(format!(
                "failed to load tokenizer '{}': {err}",
                config.tokenizer_path.display()
            ))
        })?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: config.max_sequence_length,
                ..Default::default()
            }))
            .map_err(|err| internal(format!("failed to configure truncation: {err}")))?;
        tokenizer.with_padding(Some(PaddingParams::default()));

        let builder = Session::builder()
            .map_err(|err| internal(format!("failed to create onnx session builder: {err}")))?;
        let mut builder = builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|err| internal(format!("failed to configure onnx session: {err}")))?;
        let session = builder
            .commit_from_file(&config.model_path)
            .map_err(|err| {
                internal(format!(
                    "failed to load onnx model '{}': {err}",
                    config.model_path.display()
                ))
            })?;

        let needs_token_type_ids = session
            .inputs()
            .iter()
            .any(|outlet| outlet.name() == "token_type_ids");
        let output_name = session
            .outputs()
            .first()
            .map(|outlet| outlet.name().to_string())
            .ok_or_else(|| internal("onnx model declares no outputs"))?;

        let inner = Inner {
            session: Mutex::new(session),
            tokenizer,
            dimensions: config.dimensions,
            needs_token_type_ids,
            output_name,
        };

        // 次元不一致はここで（＝サーバ起動時に）検出する。最初のentity書き込みまで
        // 発覚しないと、運用中に黙ってembeddingが欠け続けることになるため。
        inner.embed_blocking(&["dimension probe".to_string()])?;

        Ok(Self {
            inner: Arc::new(inner),
        })
    }
}

impl Inner {
    fn embed_blocking(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|err| internal(format!("tokenization failed: {err}")))?;

        let batch = encodings.len();
        // paddingをBatchLongestに設定してあるため、全encodingは同じ長さになる。
        let seq = encodings
            .first()
            .map(|encoding| encoding.get_ids().len())
            .ok_or_else(|| internal("tokenizer returned no encodings"))?;
        if seq == 0 {
            return Err(internal("tokenizer produced an empty sequence"));
        }

        let mut input_ids = Vec::with_capacity(batch * seq);
        let mut attention_mask = Vec::with_capacity(batch * seq);
        let mut token_type_ids = Vec::with_capacity(batch * seq);
        for encoding in &encodings {
            input_ids.extend(encoding.get_ids().iter().map(|&v| i64::from(v)));
            attention_mask.extend(encoding.get_attention_mask().iter().map(|&v| i64::from(v)));
            token_type_ids.extend(encoding.get_type_ids().iter().map(|&v| i64::from(v)));
        }

        let shape = vec![batch as i64, seq as i64];
        let to_tensor = |data: Vec<i64>| {
            Tensor::from_array((shape.clone(), data))
                .map_err(|err| internal(format!("failed to build input tensor: {err}")))
        };

        let mut inputs = ort::inputs![
            "input_ids" => to_tensor(input_ids)?,
            "attention_mask" => to_tensor(attention_mask.clone())?,
        ];
        if self.needs_token_type_ids {
            inputs.push(("token_type_ids".into(), to_tensor(token_type_ids)?.into()));
        }

        let mut session = self
            .session
            .lock()
            .map_err(|_| internal("onnx session lock poisoned"))?;
        let outputs = session
            .run(inputs)
            .map_err(|err| internal(format!("onnx inference failed: {err}")))?;
        let output = outputs
            .get(self.output_name.as_str())
            .ok_or_else(|| internal(format!("onnx output '{}' is missing", self.output_name)))?;
        let (out_shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|err| internal(format!("failed to read onnx output: {err}")))?;

        let dims: &[i64] = out_shape;
        if dims.len() != 3 || dims[0] != batch as i64 || dims[1] != seq as i64 {
            return Err(internal(format!(
                "unexpected onnx output shape {dims:?} (expected [batch={batch}, seq={seq}, hidden])"
            )));
        }
        let hidden = dims[2] as usize;
        if hidden != self.dimensions {
            return Err(internal(format!(
                "onnx model produces {hidden}-dimensional embeddings, expected {}",
                self.dimensions
            )));
        }

        let mut results = Vec::with_capacity(batch);
        for b in 0..batch {
            results.push(mean_pool_normalized(
                &data[b * seq * hidden..(b + 1) * seq * hidden],
                &attention_mask[b * seq..(b + 1) * seq],
                seq,
                hidden,
            ));
        }
        Ok(results)
    }
}

/// attention maskが1のトークンだけを平均してL2正規化する。全トークンがmask=0の
/// 場合（実運用ではspecial tokensが必ず入るため起きない）はゼロベクトルを返す。
fn mean_pool_normalized(
    token_embeddings: &[f32],
    attention_mask: &[i64],
    seq: usize,
    hidden: usize,
) -> Vec<f32> {
    let mut pooled = vec![0.0_f32; hidden];
    let mut count = 0.0_f32;
    for t in 0..seq {
        if attention_mask[t] == 0 {
            continue;
        }
        count += 1.0;
        let row = &token_embeddings[t * hidden..(t + 1) * hidden];
        for (acc, value) in pooled.iter_mut().zip(row) {
            *acc += value;
        }
    }
    if count > 0.0 {
        for value in &mut pooled {
            *value /= count;
        }
    }

    let norm = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut pooled {
            *value /= norm;
        }
    }
    pooled
}

#[async_trait]
impl EmbeddingProvider for LocalOnnxProvider {
    fn dimensions(&self) -> usize {
        self.inner.dimensions
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        // ONNX推論はCPUバウンドかつ数十ms〜数百msブロックするため、tokioの
        // ワーカースレッドを塞がないようblockingプールへ逃がす。
        let texts: Vec<String> = texts.iter().map(|text| text.to_string()).collect();
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.embed_blocking(&texts))
            .await
            .map_err(|err| internal(format!("embedding task panicked: {err}")))?
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn mean_pooling_ignores_masked_tokens_and_normalizes() {
        // seq=3, hidden=2。3トークン目はmask=0なので平均に入らない。
        let embeddings = [1.0, 0.0, 3.0, 4.0, 100.0, 100.0];
        let mask = [1_i64, 1, 0];
        let pooled = mean_pool_normalized(&embeddings, &mask, 3, 2);

        // 平均は(2.0, 2.0)、L2正規化で(1/√2, 1/√2)。
        assert!((pooled[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((pooled[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn load_reports_missing_files_clearly() {
        let result = LocalOnnxProvider::load(LocalOnnxConfig {
            model_path: "/nonexistent/model.onnx".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            dimensions: 768,
            max_sequence_length: 512,
        });
        let Err(err) = result else {
            panic!("load should fail for missing files");
        };
        assert!(err.to_string().contains("tokenizer"));
    }

    /// 実モデルでのend-to-end検証。モデルファイルはリポジトリに含めない
    /// （models/は.gitignore済み）ため、存在しない場合はスキップする。
    /// README記載の手順でmodels/model.onnxとmodels/tokenizer.jsonを配置すると有効になる。
    #[tokio::test]
    async fn embeds_texts_with_a_real_model() {
        let model_path = std::env::var("YSR_TEST_ONNX_MODEL")
            .unwrap_or_else(|_| "../../models/model.onnx".into());
        let tokenizer_path = std::env::var("YSR_TEST_ONNX_TOKENIZER")
            .unwrap_or_else(|_| "../../models/tokenizer.json".into());
        if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
            eprintln!("skipping embeds_texts_with_a_real_model: model files not found");
            return;
        }

        let provider = LocalOnnxProvider::load(LocalOnnxConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            dimensions: 768,
            max_sequence_length: 512,
        })
        .unwrap();
        assert_eq!(provider.dimensions(), 768);

        let vectors = provider
            .embed_batch(&[
                "The weather is lovely and sunny today.",
                "It is a beautiful clear day outside.",
                "PostgreSQL row level security policies",
            ])
            .await
            .unwrap();

        assert_eq!(vectors.len(), 3);
        for vector in &vectors {
            assert_eq!(vector.len(), 768);
            let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "vector must be L2-normalized");
        }

        // 意味的に近い2文は、無関係な文よりコサイン類似度が高いはず。
        let same_topic = cosine(&vectors[0], &vectors[1]);
        let different_topic = cosine(&vectors[0], &vectors[2]);
        assert!(
            same_topic > different_topic,
            "similar sentences should be closer: {same_topic} vs {different_topic}"
        );
    }
}
