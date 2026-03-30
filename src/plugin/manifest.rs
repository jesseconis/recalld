use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Plugin manifest (`plugin.toml`) that describes a WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    /// Path to the `.wasm` file, relative to the plugin directory.
    pub wasm: String,
    /// Events this plugin subscribes to.
    #[serde(default)]
    pub events: Vec<String>,
    /// Arbitrary plugin-specific configuration.
    #[serde(default)]
    pub config: toml::Table,
}

impl PluginManifest {
    /// Load a manifest from a `plugin.toml` file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read plugin manifest: {}", path.display()))?;
        let manifest: PluginManifest = toml::from_str(&text)
            .with_context(|| format!("failed to parse plugin manifest: {}", path.display()))?;
        Ok(manifest)
    }
}
