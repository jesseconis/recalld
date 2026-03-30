use anyhow::{Context, Result};
use image::DynamicImage;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::capture::{CaptureBackend, Screenshot};
use crate::storage::Storage;

/// Simple perceptual hash: downscale to 8x8 grayscale, compute average, produce 64-bit hash.
#[derive(Clone)]
struct PHash(u64);

impl PHash {
    fn compute(image: &DynamicImage) -> Self {
        let small = image.resize_exact(8, 8, image::imageops::FilterType::Nearest);
        let gray = small.to_luma8();
        let pixels: Vec<u8> = gray.into_raw();
        let avg: u64 = pixels.iter().map(|&p| p as u64).sum::<u64>() / pixels.len().max(1) as u64;
        let mut hash = 0u64;
        for (i, &p) in pixels.iter().enumerate().take(64) {
            if (p as u64) >= avg {
                hash |= 1 << i;
            }
        }
        PHash(hash)
    }

    fn hamming_distance(&self, other: &PHash) -> u32 {
        (self.0 ^ other.0).count_ones()
    }
}

/// Hashes of the last captured screenshot per monitor, used for dedup.
pub struct PipelineState {
    last_hashes: HashMap<String, PHash>,
}

impl PipelineState {
    pub fn new() -> Self {
        Self {
            last_hashes: HashMap::new(),
        }
    }
}

/// Process a single capture cycle: take screenshots → OCR → embed → store.
///
/// Skips screenshots that are perceptually similar to the previous one from the same monitor.
/// Heavy processing (OCR, embedding, WebP encoding) is offloaded to the blocking thread pool
/// so the async runtime stays responsive.
pub async fn process_capture(
    backend: &dyn CaptureBackend,
    storage: &Storage,
    state: &mut PipelineState,
    similarity_threshold: f64,
) -> Result<u32> {
    let screenshots = backend.capture().await?;
    let mut stored_count = 0u32;

    for screenshot in screenshots {
        let Screenshot { image, monitor } = screenshot;

        // Perceptual hash similarity check (cheap — stays on async thread)
        let current_hash = PHash::compute(&image);
        if let Some(prev) = state.last_hashes.get(&monitor.name) {
            let distance = prev.hamming_distance(&current_hash);
            let max_distance = ((1.0 - similarity_threshold) * 64.0) as u32;
            if distance <= max_distance {
                tracing::debug!(monitor = %monitor.name, distance, "screenshot unchanged, skipping");
                continue;
            }
        }

        // Update the hash for this monitor
        state.last_hashes.insert(monitor.name.clone(), current_hash);

        // Yield between monitors so the compositor/input stack can breathe.
        if stored_count > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        // Offload CPU-intensive work (OCR, embedding, WebP encoding) to the blocking pool.
        // Pin the worker thread to limited cores so Tesseract + ORT don't fan across all CPUs.
        let monitor_name = monitor.name.clone();
        let cores = crate::embedding::work_cores();
        let processed = tokio::task::spawn_blocking(move || -> Result<(String, Vec<f32>, Vec<u8>)> {
            crate::embedding::pin_to_limited_cores(cores);
            let text = match crate::ocr::extract_text(&image) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(monitor = %monitor_name, error = %e, "OCR failed");
                    return Ok((String::new(), vec![], vec![]));
                }
            };

            if text.trim().is_empty() {
                return Ok((String::new(), vec![], vec![]));
            }

            let embedding = crate::embedding::embed(&text)?;
            let webp_bytes = encode_webp(&image)?;
            Ok((text, embedding, webp_bytes))
        })
        .await
        .context("blocking task panicked")??;

        let (text, embedding, webp_bytes) = processed;
        if text.trim().is_empty() {
            tracing::debug!(monitor = %monitor.name, "no text extracted, skipping");
            continue;
        }

        // Timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Store (encrypted) — fast enough to stay on the async thread.
        storage.store_entry(
            &monitor.name,
            &monitor.name,
            &text,
            timestamp,
            &embedding,
            &webp_bytes,
        )?;

        stored_count += 1;
        tracing::info!(
            monitor = %monitor.name,
            text_len = text.len(),
            timestamp,
            "captured and stored screenshot"
        );
    }

    Ok(stored_count)
}

fn encode_webp(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::WebP)
        .context("failed to encode image as WebP")?;
    Ok(buf.into_inner())
}
