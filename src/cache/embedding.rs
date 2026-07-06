use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::{Tokenizer, TruncationDirection, TruncationParams, TruncationStrategy};

pub const EMBEDDING_DIM: usize = 384;
const MAX_SEQUENCE_LENGTH: usize = 512;

pub struct LocalEmbeddingEngine {
    // ponytail: Mutex because ort::Session::run needs &mut while generate takes &self
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl LocalEmbeddingEngine {
    pub fn new(model_path: &Path, tokenizer_path: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|err| format!("failed to create ONNX session builder: {err}"))?
            .commit_from_file(model_path)
            .map_err(|err| {
                format!(
                    "failed to load ONNX model at {}: {err}",
                    model_path.display()
                )
            })?;

        let mut tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|err| {
            format!(
                "failed to load tokenizer at {}: {err}",
                tokenizer_path.display()
            )
        })?;

        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_SEQUENCE_LENGTH,
                stride: 0,
                strategy: TruncationStrategy::LongestFirst,
                direction: TruncationDirection::Right,
            }))
            .map_err(|err| format!("failed to configure tokenizer truncation: {err}"))?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    pub fn generate(&self, text: &str) -> Result<Vec<f32>, String> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|err| format!("tokenization failed: {err}"))?;

        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&mask| mask as i64)
            .collect();
        let token_type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .map(|&token_type| token_type as i64)
            .collect();

        let seq_len = input_ids.len();
        let shape = [1_usize, seq_len];

        let input_ids_tensor = Tensor::from_array((shape, input_ids.into_boxed_slice()))
            .map_err(|err| format!("failed to build input_ids tensor: {err}"))?;
        let attention_mask_tensor = Tensor::from_array((shape, attention_mask.into_boxed_slice()))
            .map_err(|err| format!("failed to build attention_mask tensor: {err}"))?;
        let token_type_ids_tensor = Tensor::from_array((shape, token_type_ids.into_boxed_slice()))
            .map_err(|err| format!("failed to build token_type_ids tensor: {err}"))?;

        let session = self
            .session
            .lock()
            .map_err(|_| "ONNX session lock poisoned".to_string())?;

        let outputs = session
            .run(
                ort::inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attention_mask_tensor,
                    "token_type_ids" => token_type_ids_tensor,
                ]
                .map_err(|err| format!("failed to build ONNX inputs: {err}"))?,
            )
            .map_err(|err| format!("ONNX inference failed: {err}"))?;

        let hidden_states = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|err| format!("failed to extract hidden states: {err}"))?;

        let mask = encoding.get_attention_mask();
        let mut pooled = vec![0.0_f32; EMBEDDING_DIM];
        let mut mask_sum = 0.0_f32;

        for token_idx in 0..seq_len {
            let weight = mask[token_idx] as f32;
            if weight == 0.0 {
                continue;
            }

            mask_sum += weight;
            for dim in 0..EMBEDDING_DIM {
                pooled[dim] += hidden_states[[0, token_idx, dim]] * weight;
            }
        }

        if mask_sum > 0.0 {
            for value in &mut pooled {
                *value /= mask_sum;
            }
        }

        l2_normalize(&mut pooled);
        Ok(pooled)
    }

    pub fn dot_product(v1: &[f32], v2: &[f32]) -> f32 {
        v1.iter().zip(v2).map(|(left, right)| left * right).sum()
    }
}

fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}
