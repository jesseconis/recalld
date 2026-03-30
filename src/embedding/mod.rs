use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::{Mutex, OnceLock};

static MODEL: OnceLock<Mutex<TextEmbedding>> = OnceLock::new();

/// Number of cores the heavy-work threads are pinned to.
static WORK_CORES: OnceLock<usize> = OnceLock::new();

/// Pin the calling thread to the first `n` CPUs.
/// This limits parallelism for any library that reads available_parallelism()
/// or spawns OpenMP threads (Tesseract, ONNX Runtime).
pub fn pin_to_limited_cores(n: usize) {
    unsafe {
        let mut restricted: libc::cpu_set_t = std::mem::zeroed();
        for i in 0..n.min(libc::CPU_SETSIZE as usize) {
            libc::CPU_SET(i, &mut restricted);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &restricted);
    }
}

fn init_model(threads: usize) -> TextEmbedding {
    pin_to_limited_cores(threads);
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

/// Return the configured number of work cores (for use by other modules).
pub fn work_cores() -> usize {
    *WORK_CORES.get_or_init(|| 2)
}

/// Generate a 384-dimensional embedding for the given text.
///
/// Concatenates all non-empty lines into a single string and embeds it in one
/// pass to avoid rayon's parallel batching overhead for many small sentences.
pub fn embed(text: &str) -> Result<Vec<f32>> {
    let model = get_model()?;
    let model = model.lock().unwrap();

    // Collapse to a single string — the model handles short texts well and this
    // avoids spawning rayon threads for per-line batches.
    let collapsed: String = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(". ");

    if collapsed.is_empty() {
        return Ok(vec![0.0; 384]);
    }

    let embeddings = model
        .embed(vec![collapsed.as_str()], None)
        .context("fastembed encoding failed")?;

    Ok(embeddings.into_iter().next().unwrap_or_else(|| vec![0.0; 384]))
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
