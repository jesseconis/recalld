pub mod pipeline;
pub mod scheduler;

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::storage::Storage;

/// Shared daemon state accessible by the gRPC service layer.
pub struct DaemonState {
    pub config: Config,
    pub storage: Arc<Storage>,
    pub plugin_manager: Arc<Mutex<crate::plugin::PluginManager>>,
    pub start_time: Instant,
}

/// Run the daemon: start capture scheduler + gRPC server.
///
/// Blocks until SIGTERM or SIGINT is received.
pub async fn run(config: Config, storage: Storage) -> Result<()> {
    let start_time = Instant::now();
    let storage = Arc::new(storage);

    // Lower CPU scheduling priority so we don't compete with the desktop.
    // SAFETY: nice(2) is always safe to call; errors just mean we stay at default priority.
    unsafe {
        libc::nice(10);
    }
    tracing::debug!("set process nice level to 10");

    // Pre-warm the embedding model in the background so the first capture isn't delayed.
    // Thread counts are constrained via env vars before runtime startup; avoid per-thread
    // CPU affinity here because it can hurt compositor/input responsiveness.
    let emb_threads = config.processing.embedding_threads;
    let warmup = tokio::task::spawn_blocking(move || {
        tracing::info!(threads = emb_threads, "pre-loading embedding model...");
        if let Err(e) = crate::embedding::warm_up_with_threads(emb_threads) {
            tracing::error!(error = %e, "failed to pre-load embedding model");
        } else {
            tracing::info!("embedding model ready");
        }
    });

    // Write PID file
    let pid_path = config.pid_path();
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, std::process::id().to_string())
        .context("failed to write PID file")?;

    // Shutdown coordination
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Select capture backend
    let backend = crate::capture::select_backend(&config.capture.backend).await?;
    tracing::info!(backend = backend.name(), "capture backend selected");

    let metadata_provider: Arc<dyn crate::metadata::MetadataProvider> =
        Arc::new(crate::metadata::build_provider_stack());
    let event_bus = Arc::new(crate::plugin::events::EventBus::new(256));
    let plugin_manager = Arc::new(Mutex::new(crate::plugin::PluginManager::new(
        config.plugins_dir(),
        &config.plugins.enabled,
    )?));

    let state = Arc::new(DaemonState {
        config: config.clone(),
        storage: Arc::clone(&storage),
        plugin_manager: Arc::clone(&plugin_manager),
        start_time,
    });

    // Start gRPC server
    let grpc_addr = config.grpc.listen_addr.parse()?;
    let grpc_state = state.clone();
    let grpc_shutdown_rx = shutdown_rx.clone();
    let grpc_handle = tokio::spawn(async move {
        if let Err(e) = crate::api::serve(grpc_state, grpc_addr, grpc_shutdown_rx).await {
            tracing::error!(error = %e, "gRPC server failed");
        }
    });

    // Start HTTP server (optional)
    let http_handle = if config.http.enabled {
        let http_addr = config.http.listen_addr.parse()?;
        let http_state = state.clone();
        let http_shutdown_rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = crate::web::serve(http_state, http_addr, http_shutdown_rx).await {
                tracing::error!(error = %e, "HTTP server failed");
            }
        }))
    } else {
        tracing::info!("HTTP server disabled by config");
        None
    };

    // Wait for the embedding model to finish loading before starting the scheduler.
    let _ = warmup.await;

    // Start capture scheduler
    let scheduler_shutdown_rx = shutdown_rx.clone();
    let capture_interval = Duration::from_secs(config.capture.interval_secs);
    let similarity_threshold = config.capture.similarity_threshold;
    let ocr_options = crate::ocr::OcrOptions::from_config_width(config.processing.ocr_max_width);
    tracing::info!(
        interval_secs = config.capture.interval_secs,
        similarity_threshold,
        ocr_max_width = ocr_options.max_width.unwrap_or(0),
        max_hamming_distance = pipeline::max_hamming_distance(similarity_threshold),
        dedupe_baseline = "last_captured",
        "capture scheduler configured"
    );
    let scheduler_storage = Arc::clone(&storage);
    let scheduler_metadata = Arc::clone(&metadata_provider);
    let scheduler_events = Arc::clone(&event_bus);
    let scheduler_handle = tokio::spawn(async move {
        if let Err(e) = scheduler::run(
            backend.as_ref(),
            scheduler_storage,
            scheduler_metadata,
            scheduler_events,
            capture_interval,
            similarity_threshold,
            ocr_options,
            scheduler_shutdown_rx,
        )
        .await
        {
            tracing::error!(error = %e, "scheduler failed");
        }
    });

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received SIGINT, shutting down...");
        }
        _ = sigterm() => {
            tracing::info!("received SIGTERM, shutting down...");
        }
    }

    // Signal all tasks to stop
    let _ = shutdown_tx.send(true);

    // Wait for tasks to finish
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = grpc_handle.await;
        if let Some(handle) = http_handle {
            let _ = handle.await;
        }
        let _ = scheduler_handle.await;
    })
    .await;

    // Clean up PID file
    let _ = std::fs::remove_file(&pid_path);

    tracing::info!("daemon shut down cleanly");
    Ok(())
}

async fn sigterm() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sig = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    sig.recv().await;
}
