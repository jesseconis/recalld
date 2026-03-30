pub mod events;
pub mod host;
pub mod manifest;

use anyhow::{Context, Result};
use std::path::PathBuf;

use manifest::PluginManifest;

/// Information about a loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    pub enabled: bool,
}

/// Manages discovery, loading, and lifecycle of WASM plugins.
pub struct PluginManager {
    plugins_dir: PathBuf,
    plugins: Vec<PluginInfo>,
    engine: wasmtime::Engine,
}

impl PluginManager {
    /// Create a new plugin manager, scanning the plugins directory.
    pub fn new(plugins_dir: PathBuf, enabled_names: &[String]) -> Result<Self> {
        let engine = host::create_engine()?;

        let mut plugins = Vec::new();

        if plugins_dir.exists() {
            for entry in std::fs::read_dir(&plugins_dir)
                .with_context(|| format!("failed to read plugins dir: {}", plugins_dir.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let manifest_path = path.join("plugin.toml");
                if !manifest_path.exists() {
                    continue;
                }

                match PluginManifest::load(&manifest_path) {
                    Ok(manifest) => {
                        let enabled = enabled_names.contains(&manifest.name);
                        tracing::info!(
                            plugin = %manifest.name,
                            version = %manifest.version,
                            enabled,
                            "discovered plugin"
                        );
                        plugins.push(PluginInfo {
                            manifest,
                            dir: path,
                            enabled,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %manifest_path.display(),
                            error = %e,
                            "failed to load plugin manifest"
                        );
                    }
                }
            }
        }

        Ok(Self {
            plugins_dir,
            plugins,
            engine,
        })
    }

    /// List all discovered plugins.
    pub fn list(&self) -> &[PluginInfo] {
        &self.plugins
    }

    /// Enable a plugin by name.
    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.manifest.name == name) {
            p.enabled = true;
            true
        } else {
            false
        }
    }

    /// Disable a plugin by name.
    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.manifest.name == name) {
            p.enabled = false;
            true
        } else {
            false
        }
    }

    /// Get the wasmtime engine.
    pub fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }

    /// Plugins directory path.
    pub fn plugins_dir(&self) -> &PathBuf {
        &self.plugins_dir
    }
}
