use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static MODEL: OnceLock<Mutex<TextEmbedding>> = OnceLock::new();

/// Configured thread count for embedding-related libraries.
static WORK_CORES: OnceLock<usize> = OnceLock::new();

fn init_model(threads: usize) -> TextEmbedding {
    WORK_CORES.get_or_init(|| threads);
    TextEmbedding::try_new(InitOptions::new(EmbeddingModel::AllMiniLML6V2))
        .expect("failed to initialise fastembed model (all-MiniLM-L6-v2)")
}

fn get_model() -> Result<&'static Mutex<TextEmbedding>> {
    let model = MODEL.get_or_init(|| {
        Mutex::new(init_model(*WORK_CORES.get_or_init(|| 2)))
    });
    Ok(model)
}

/// Pre-load the embedding model, limiting ort + rayon to `threads` CPU cores.
pub fn warm_up_with_threads(threads: usize) -> Result<()> {
    WORK_CORES.get_or_init(|| threads);
    MODEL.get_or_init(|| Mutex::new(init_model(threads)));
    Ok(())
}

/// Generate a 384-dimensional embedding for the given text.
///
/// Concatenates all non-empty lines into a single string and embeds it in one
/// pass to avoid rayon's parallel batching overhead for many small sentences.
pub fn embed(text: &str) -> Result<Vec<f32>> {
    let total_start = Instant::now();
    let model_start = Instant::now();
    let model = get_model()?;
    let model = model.lock().unwrap();
    let lock_ms = model_start.elapsed().as_millis();

    // Collapse to a single string — the model handles short texts well and this
    // avoids spawning rayon threads for per-line batches.
    let collapse_start = Instant::now();
    let collapsed: String = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(". ");
    let collapse_ms = collapse_start.elapsed().as_millis();

    if collapsed.is_empty() {
        tracing::debug!(
            text_len = text.len(),
            lock_ms,
            collapse_ms,
            total_ms = total_start.elapsed().as_millis(),
            "embedding skipped (empty text)"
        );
        return Ok(vec![0.0; 384]);
    }

    let collapsed_len = collapsed.len();
    let embed_start = Instant::now();
    let embeddings = model
        .embed(vec![collapsed.as_str()], None)
        .context("fastembed encoding failed")?;
    let embed_ms = embed_start.elapsed().as_millis();

    let result = embeddings.into_iter().next().unwrap_or_else(|| vec![0.0; 384]);
    let result_text = match text.char_indices().nth(100).map(|(idx, _)| idx) {
        Some(idx) => format!("{}...", &text[..idx]),
        None => text.to_string(),
    };
    tracing::debug!(
        text_len = text.len(),
        collapsed_len,
        lock_ms,
        collapse_ms,
        embed_ms,
        total_ms = total_start.elapsed().as_millis(),
        result_text,
    );
    Ok(result)
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}
