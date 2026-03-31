use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

use crate::capture::CaptureBackend;
use crate::daemon::pipeline::{self, PipelineState};
use crate::storage::Storage;

/// Run the capture-process-store loop on a fixed interval.
///
/// Exits when the provided `shutdown` token is cancelled.
pub async fn run(
    backend: &dyn CaptureBackend,
    storage: Arc<Storage>,
    interval: Duration,
    similarity_threshold: f64,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let mut state = PipelineState::new();
    let mut ticker = time::interval(interval);
    // Don't try to catch up if processing takes longer than one tick.
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match pipeline::process_capture(backend, Arc::clone(&storage), &mut state, similarity_threshold).await {
                    Ok(n) if n > 0 => tracing::debug!(stored = n, "capture cycle complete"),
                    Ok(_) => {} // nothing new stored
                    Err(e) => tracing::error!(error = %e, "capture cycle failed"),
                }
            }
            _ = shutdown_signal(&shutdown) => {
                tracing::info!("scheduler received shutdown signal");
                break;
            }
        }
    }

    Ok(())
}

async fn shutdown_signal(rx: &tokio::sync::watch::Receiver<bool>) {
    let mut rx = rx.clone();
    // Wait until the value becomes `true`.
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}
