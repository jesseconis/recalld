use serde::{Deserialize, Serialize};
use std::process::Command;

/// Structured metadata attached to a capture.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextSnapshot {
    pub source: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub window_id: Option<String>,
    pub workspace: Option<String>,
    pub output: Option<String>,
    pub focused: bool,
    pub visible_on_outputs: Vec<String>,
    pub timestamp: i64,
    pub confidence: f32,
    pub capabilities: Vec<String>,
}

impl ContextSnapshot {
    pub fn display_app(&self, fallback: &str) -> String {
        self.app_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.title.as_deref().filter(|s| !s.trim().is_empty()))
            .unwrap_or(fallback)
            .to_string()
    }

    pub fn display_title(&self, fallback: &str) -> String {
        self.title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.app_id.as_deref().filter(|s| !s.trim().is_empty()))
            .unwrap_or(fallback)
            .to_string()
    }
}

pub trait MetadataProvider: Send + Sync {
    fn snapshot_for_monitor(&self, monitor_name: &str, timestamp: i64) -> Option<ContextSnapshot>;
}

pub struct ProviderStack {
    providers: Vec<Box<dyn MetadataProvider>>,
}

impl ProviderStack {
    pub fn new(providers: Vec<Box<dyn MetadataProvider>>) -> Self {
        Self { providers }
    }
}

impl MetadataProvider for ProviderStack {
    fn snapshot_for_monitor(&self, monitor_name: &str, timestamp: i64) -> Option<ContextSnapshot> {
        for provider in &self.providers {
            if let Some(snapshot) = provider.snapshot_for_monitor(monitor_name, timestamp) {
                return Some(snapshot);
            }
        }
        None
    }
}

pub struct NoopMetadataProvider;

impl MetadataProvider for NoopMetadataProvider {
    fn snapshot_for_monitor(&self, _monitor_name: &str, _timestamp: i64) -> Option<ContextSnapshot> {
        None
    }
}

/// Lightweight Hyprland backend using `hyprctl -j activewindow` when available.
/// This gives immediate value on Hyprland and gracefully falls back elsewhere.
pub struct HyprlandMetadataProvider;

impl MetadataProvider for HyprlandMetadataProvider {
    fn snapshot_for_monitor(&self, monitor_name: &str, timestamp: i64) -> Option<ContextSnapshot> {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            return None;
        }

        let output = Command::new("hyprctl")
            .arg("-j")
            .arg("activewindow")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        let app_id = value
            .get("class")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let title = value
            .get("title")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let window_id = value
            .get("address")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let workspace = value
            .get("workspace")
            .and_then(|w| w.get("name"))
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        Some(ContextSnapshot {
            source: "hyprland_ipc".to_string(),
            app_id,
            title,
            window_id,
            workspace,
            output: Some(monitor_name.to_string()),
            focused: true,
            visible_on_outputs: vec![monitor_name.to_string()],
            timestamp,
            confidence: 0.9,
            capabilities: vec![
                "active_window".to_string(),
                "window_id".to_string(),
                "workspace".to_string(),
            ],
        })
    }
}

pub fn build_provider_stack() -> ProviderStack {
    ProviderStack::new(vec![
        Box::new(HyprlandMetadataProvider),
        Box::new(NoopMetadataProvider),
    ])
}