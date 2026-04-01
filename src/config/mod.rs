use std::path::PathBuf;

use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};

/// Default capture interval in seconds.
const DEFAULT_CAPTURE_INTERVAL: u64 = 30;
/// Default idle timeout in seconds before pausing captures.
const DEFAULT_IDLE_TIMEOUT: u64 = 60;
/// Default similarity threshold (0.0–1.0) to skip duplicate screenshots.
/// Higher values are stricter; lower values skip more aggressively.
const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.9;
/// Default gRPC listen address.
const DEFAULT_GRPC_ADDR: &str = "[::1]:50051";
/// Default HTTP listen address for the web UI/API.
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:58080";
/// Default HTTP session TTL for web logins.
const DEFAULT_HTTP_SESSION_TTL_SECS: u64 = 60 * 60 * 24;
/// Default number of gallery items per page.
const DEFAULT_HTTP_PAGE_SIZE: u32 = 24;
/// Max page size accepted by the web API.
const DEFAULT_HTTP_MAX_PAGE_SIZE: u32 = 100;
/// Default max search results.
const DEFAULT_SEARCH_LIMIT: u32 = 20;
/// Default number of threads for the embedding model (ONNX Runtime intra-op).
const DEFAULT_EMBEDDING_THREADS: usize = 2;
/// Default max OCR input width. `0` disables downscaling.
const DEFAULT_OCR_MAX_WIDTH: u32 = 1280;
/// Default lexical-vs-semantic blend weight for search ranking.
const DEFAULT_LEXICAL_WEIGHT: f32 = 0.35;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub capture: CaptureConfig,
    pub storage: StorageConfig,
    pub grpc: GrpcConfig,
    pub http: HttpConfig,
    pub plugins: PluginsConfig,
    pub processing: ProcessingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// Screenshot capture interval in seconds.
    pub interval_secs: u64,
    /// Seconds of user inactivity before pausing captures.
    pub idle_timeout_secs: u64,
    /// Perceptual-hash similarity threshold in the range [0.0, 1.0].
    /// Higher values are stricter; lower values skip more captures.
    pub similarity_threshold: f64,
    /// Force a specific backend: "portal", "wayshot", "grim", or "auto".
    pub backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Base data directory (screenshots + DB). Defaults to XDG data dir.
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GrpcConfig {
    /// Address the gRPC server listens on.
    pub listen_addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    /// Whether the HTTP server is enabled.
    pub enabled: bool,
    /// Address the HTTP server listens on.
    pub listen_addr: String,
    /// Session cookie lifetime for web logins.
    pub session_ttl_secs: u64,
    /// Default gallery page size.
    pub default_page_size: u32,
    /// Max gallery page size accepted from clients.
    pub max_page_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// Directory to scan for plugins.
    pub dir: Option<PathBuf>,
    /// Names of plugins that are enabled.
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessingConfig {
    /// Number of threads for the embedding model (ONNX Runtime intra-op parallelism).
    pub embedding_threads: usize,
    /// Downscale OCR inputs wider than this many pixels before Tesseract.
    /// Set to `0` to keep the original resolution.
    pub ocr_max_width: u32,
    /// Blend weight for lexical ranking in hybrid search.
    /// `0.0` = semantic only, `1.0` = lexical only.
    pub lexical_weight: f32,
}

// --- Defaults ---

impl Default for Config {
    fn default() -> Self {
        Self {
            capture: CaptureConfig::default(),
            storage: StorageConfig::default(),
            grpc: GrpcConfig::default(),
            http: HttpConfig::default(),
            plugins: PluginsConfig::default(),
            processing: ProcessingConfig::default(),
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            interval_secs: DEFAULT_CAPTURE_INTERVAL,
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT,
            similarity_threshold: DEFAULT_SIMILARITY_THRESHOLD,
            backend: "auto".into(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { data_dir: None }
    }
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_GRPC_ADDR.into(),
        }
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_addr: DEFAULT_HTTP_ADDR.into(),
            session_ttl_secs: DEFAULT_HTTP_SESSION_TTL_SECS,
            default_page_size: DEFAULT_HTTP_PAGE_SIZE,
            max_page_size: DEFAULT_HTTP_MAX_PAGE_SIZE,
        }
    }
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            dir: None,
            enabled: Vec::new(),
        }
    }
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        Self {
            embedding_threads: DEFAULT_EMBEDDING_THREADS,
            ocr_max_width: DEFAULT_OCR_MAX_WIDTH,
            lexical_weight: DEFAULT_LEXICAL_WEIGHT,
        }
    }
}

// --- Path helpers ---

impl Config {
    /// Load config from the standard XDG path, falling back to defaults.
    pub fn load() -> anyhow::Result<Self> {
        let cfg = match config_file_path() {
            Ok(path) => path,
            Err(_) => {
                let cfg = Config::default();
                cfg.validate()?;
                return Ok(cfg);
            }
        };

        let text = std::fs::read_to_string(&cfg)?;
        let cfg: Config = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Save config to the standard XDG path.
    pub fn save(&self) -> anyhow::Result<()> {
        self.validate()?;
        let path = config_file_path_raw();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    /// Validate configuration values that can make capture behavior surprising.
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.capture.interval_secs > 0, "capture.interval_secs must be greater than 0");
        ensure!(
            (0.0..=1.0).contains(&self.capture.similarity_threshold),
            "capture.similarity_threshold must be between 0.0 and 1.0 inclusive"
        );
        ensure!(
            self.processing.embedding_threads > 0,
            "processing.embedding_threads must be greater than 0"
        );
        ensure!(
            (0.0..=1.0).contains(&self.processing.lexical_weight),
            "processing.lexical_weight must be between 0.0 and 1.0 inclusive"
        );
        ensure!(self.http.session_ttl_secs > 0, "http.session_ttl_secs must be greater than 0");
        ensure!(
            self.http.default_page_size > 0,
            "http.default_page_size must be greater than 0"
        );
        ensure!(
            self.http.max_page_size > 0,
            "http.max_page_size must be greater than 0"
        );
        ensure!(
            self.http.default_page_size <= self.http.max_page_size,
            "http.default_page_size must be <= http.max_page_size"
        );
        Ok(())
    }

    /// Resolved data directory (XDG default or user override).
    pub fn data_dir(&self) -> PathBuf {
        self.storage
            .data_dir
            .clone()
            .unwrap_or_else(default_data_dir)
    }

    /// Directory where encrypted screenshots are stored.
    pub fn screenshots_dir(&self) -> PathBuf {
        self.data_dir().join("screenshots")
    }

    /// Path to the SQLite database.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir().join("recalld.db")
    }

    /// Path to the SQLite WAL sidecar file.
    pub fn db_wal_path(&self) -> PathBuf {
        path_with_suffix(&self.db_path(), "-wal")
    }

    /// Path to the SQLite shared-memory sidecar file.
    pub fn db_shm_path(&self) -> PathBuf {
        path_with_suffix(&self.db_path(), "-shm")
    }

    /// Path to the encrypted data-encryption key file.
    pub fn key_path(&self) -> PathBuf {
        self.data_dir().join("key.enc")
    }

    /// Path to the PID file for the daemon.
    pub fn pid_path(&self) -> PathBuf {
        self.data_dir().join("recalld.pid")
    }

    /// Plugins directory (XDG config default or user override).
    pub fn plugins_dir(&self) -> PathBuf {
        self.plugins
            .dir
            .clone()
            .unwrap_or_else(|| config_dir().join("plugins"))
    }

    /// Default search result limit.
    pub fn search_limit(&self) -> u32 {
        DEFAULT_SEARCH_LIMIT
    }
}

/// `~/.config/recalld/`
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("could not determine XDG config dir")
        .join("recalld")
}

/// `~/.config/recalld/config.toml`
pub fn config_file_path() -> anyhow::Result<PathBuf> {
    let config = config_file_path_raw();
    if config.exists() {
        tracing::info!("loading config from {}", config.display());
        Ok(config)
    } else {
        tracing::warn!("no config found at {}", config.display());
        bail!("no config at {}", config.display())
    }
}

/// `~/.config/recalld/config.toml` (path only, may not exist yet)
pub fn config_file_path_raw() -> PathBuf {
    config_dir().join("config.toml")
}

/// `~/.local/share/recalld/`
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("could not determine XDG data dir")
        .join("recalld")
}

fn path_with_suffix(path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut out = path.as_os_str().to_os_string();
    out.push(suffix);
    PathBuf::from(out)
}
