use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::capture::{CaptureBackend, Screenshot};
use crate::storage::Storage;

/// Simple perceptual hash: downscale to 8x8 grayscale, compute average, produce 64-bit hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashBaseline {
    FirstSeen,
    PreviousCapture,
}

impl HashBaseline {
    fn as_str(self) -> &'static str {
        match self {
            Self::FirstSeen => "first_seen",
            Self::PreviousCapture => "last_captured",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DedupeDecision {
    baseline: HashBaseline,
    distance: Option<u32>,
    max_distance: u32,
    skip: bool,
}

pub fn max_hamming_distance(similarity_threshold: f64) -> u32 {
    ((1.0 - similarity_threshold) * 64.0) as u32
}

fn dedupe_decision(
    state: &mut PipelineState,
    monitor_name: &str,
    current_hash: PHash,
    similarity_threshold: f64,
) -> DedupeDecision {
    let max_distance = max_hamming_distance(similarity_threshold);
    match state.last_hashes.insert(monitor_name.to_owned(), current_hash) {
        Some(previous_hash) => {
            let distance = previous_hash.hamming_distance(&current_hash);
            DedupeDecision {
                baseline: HashBaseline::PreviousCapture,
                distance: Some(distance),
                max_distance,
                skip: distance <= max_distance,
            }
        }
        None => DedupeDecision {
            baseline: HashBaseline::FirstSeen,
            distance: None,
            max_distance,
            skip: false,
        },
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
    storage: Arc<Storage>,
    metadata_provider: Arc<dyn crate::metadata::MetadataProvider>,
    event_bus: Arc<crate::plugin::events::EventBus>,
    state: &mut PipelineState,
    similarity_threshold: f64,
    ocr_options: crate::ocr::OcrOptions,
) -> Result<u32> {
    let capture_start = Instant::now();
    tracing::debug!(similarity_threshold, "capture cycle start");
    let screenshots = backend.capture().await?;
    tracing::debug!(
        count = screenshots.len(),
        elapsed_ms = capture_start.elapsed().as_millis(),
        "capture cycle screenshots ready"
    );
    let mut stored_count = 0u32;

    for screenshot in screenshots {
        let Screenshot { image, monitor } = screenshot;
        let (width, height) = image.dimensions();
        tracing::debug!(
            monitor = %monitor.name,
            width,
            height,
            "processing screenshot"
        );

        // Perceptual hash similarity check (cheap — stays on async thread)
        let hash_start = Instant::now();
        let current_hash = PHash::compute(&image);
        let decision = dedupe_decision(state, &monitor.name, current_hash, similarity_threshold);
        match decision.distance {
            Some(distance) => {
                tracing::debug!(
                    monitor = %monitor.name,
                    baseline = decision.baseline.as_str(),
                    distance,
                    max_distance = decision.max_distance,
                    skip = decision.skip,
                    hash_ms = hash_start.elapsed().as_millis(),
                    "perceptual hash compared"
                );
            }
            None => {
                tracing::debug!(
                    monitor = %monitor.name,
                    baseline = decision.baseline.as_str(),
                    max_distance = decision.max_distance,
                    hash_ms = hash_start.elapsed().as_millis(),
                    "perceptual hash computed (first seen)"
                );
            }
        }

        if decision.skip {
            tracing::debug!(
                monitor = %monitor.name,
                baseline = decision.baseline.as_str(),
                distance = decision.distance.unwrap_or_default(),
                max_distance = decision.max_distance,
                "screenshot unchanged, skipping"
            );
            continue;
        }

        // Yield between monitors so the compositor/input stack can breathe.
        if stored_count > 0 {
            tracing::debug!(monitor = %monitor.name, "yielding between monitors");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        // Offload CPU-intensive work (OCR, embedding, WebP encoding) to the blocking pool.
        // The thread-count environment variables set in main() limit parallelism without
        // pinning the desktop to a small set of CPUs.
        let monitor_name = monitor.name.clone();
        let blocking_start = Instant::now();
        let storage = Arc::clone(&storage);
        let metadata_provider = Arc::clone(&metadata_provider);
        let stored = tokio::task::spawn_blocking(move || -> Result<Option<(usize, i64, String)>> {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            let context = metadata_provider.snapshot_for_monitor(&monitor_name, timestamp);
            let app = context
                .as_ref()
                .map(|c| c.display_app(&monitor_name))
                .unwrap_or_else(|| monitor_name.clone());
            let title = context
                .as_ref()
                .map(|c| c.display_title(&monitor_name))
                .unwrap_or_else(|| monitor_name.clone());

            let ocr_start = Instant::now();
            let text = match crate::ocr::extract_text_with_options(&image, ocr_options) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(monitor = %monitor_name, error = %e, "OCR failed");
                    return Ok(None);
                }
            };
            let ocr_elapsed = ocr_start.elapsed();

            if text.trim().is_empty() {
                tracing::debug!(
                    monitor = %monitor_name,
                    ocr_ms = ocr_elapsed.as_millis(),
                    "OCR produced empty text"
                );
                return Ok(None);
            }

            tracing::debug!(
                monitor = %monitor_name,
                text_len = text.len(),
                ocr_max_width = ocr_options.max_width.unwrap_or(0),
                ocr_ms = ocr_elapsed.as_millis(),
                "OCR complete"
            );

            let embed_start = Instant::now();
            let embedding = crate::embedding::embed(&text)?;
            tracing::debug!(
                monitor = %monitor_name,
                embedding_len = embedding.len(),
                embed_ms = embed_start.elapsed().as_millis(),
                "embedding complete"
            );

            let webp_start = Instant::now();
            let webp_bytes = encode_webp(&image)?;
            tracing::debug!(
                monitor = %monitor_name,
                webp_bytes = webp_bytes.len(),
                webp_ms = webp_start.elapsed().as_millis(),
                "WebP encode complete"
            );

            let store_start = Instant::now();
            let screenshot_filename = format!("{timestamp}.webp.enc");
            storage.store_entry(
                &app,
                &title,
                &text,
                timestamp,
                &embedding,
                &webp_bytes,
                context.as_ref(),
            )?;
            tracing::debug!(
                monitor = %monitor_name,
                context_source = context.as_ref().map(|c| c.source.as_str()).unwrap_or("fallback"),
                store_ms = store_start.elapsed().as_millis(),
                "stored entry"
            );

            Ok(Some((text.len(), timestamp, screenshot_filename)))
        })
        .await
        .context("blocking task panicked")??;

        tracing::debug!(
            monitor = %monitor.name,
            blocking_ms = blocking_start.elapsed().as_millis(),
            "blocking pipeline steps complete"
        );

        let Some((text_len, timestamp, screenshot_filename)) = stored else {
            tracing::debug!(monitor = %monitor.name, "no text extracted, skipping");
            continue;
        };

        event_bus.publish(crate::plugin::events::Event::ScreenshotCaptured {
            timestamp,
            monitor: monitor.name.clone(),
            screenshot_filename,
        });

        stored_count += 1;
        tracing::info!(
            monitor = %monitor.name,
            text_len,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_distance_reflects_threshold() {
        assert_eq!(max_hamming_distance(0.9), 6);
        assert_eq!(max_hamming_distance(0.1), 57);
    }

    #[test]
    fn first_capture_is_never_skipped() {
        let mut state = PipelineState::new();

        let decision = dedupe_decision(&mut state, "DP-1", PHash(0), 0.9);

        assert_eq!(decision.baseline, HashBaseline::FirstSeen);
        assert_eq!(decision.distance, None);
        assert!(!decision.skip);
        assert_eq!(state.last_hashes.get("DP-1"), Some(&PHash(0)));
    }

    #[test]
    fn repeated_capture_is_skipped() {
        let mut state = PipelineState::new();
        let _ = dedupe_decision(&mut state, "DP-1", PHash(0), 0.9);

        let decision = dedupe_decision(&mut state, "DP-1", PHash(0), 0.9);

        assert_eq!(decision.baseline, HashBaseline::PreviousCapture);
        assert_eq!(decision.distance, Some(0));
        assert!(decision.skip);
    }

    #[test]
    fn skipped_capture_advances_baseline_to_last_capture() {
        let mut state = PipelineState::new();
        let _ = dedupe_decision(&mut state, "DP-1", PHash(0), 0.9);

        let skipped = dedupe_decision(&mut state, "DP-1", PHash(0b11_1111), 0.9);
        assert!(skipped.skip);
        assert_eq!(skipped.distance, Some(6));
        assert_eq!(state.last_hashes.get("DP-1"), Some(&PHash(0b11_1111)));

        let next = dedupe_decision(&mut state, "DP-1", PHash(0b1111_1111), 0.9);
        assert!(next.skip);
        assert_eq!(next.distance, Some(2));
        assert_eq!(state.last_hashes.get("DP-1"), Some(&PHash(0b1111_1111)));
    }
}
