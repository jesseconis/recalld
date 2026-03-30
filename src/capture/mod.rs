use anyhow::Result;
use image::DynamicImage;
use std::future::Future;
use std::pin::Pin;

pub mod grim;
pub mod portal;
pub mod wayshot;

/// Metadata about the source monitor.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// A captured screenshot with associated monitor info.
#[derive(Debug, Clone)]
pub struct Screenshot {
    pub image: DynamicImage,
    pub monitor: MonitorInfo,
}

/// Trait that all capture backends implement.
pub trait CaptureBackend: Send + Sync {
    /// Human-readable name for this backend.
    fn name(&self) -> &str;

    /// Capture screenshots from all available outputs.
    fn capture(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Screenshot>>> + Send + '_>>;
}

/// Select a backend based on the config string.
/// `"auto"` tries wayshot → grim → portal in order.
pub async fn select_backend(backend: &str) -> Result<Box<dyn CaptureBackend>> {
    match backend {
        "portal" => Ok(Box::new(portal::PortalBackend::new().await?)),
        "wayshot" => Ok(Box::new(wayshot::WayshotBackend::new()?)),
        "grim" => Ok(Box::new(grim::GrimBackend::new().await?)),
        "auto" | _ => auto_detect().await,
    }
}

async fn auto_detect() -> Result<Box<dyn CaptureBackend>> {
    // Try wayshot first (native, best for wlroots compositors)
    if let Ok(ws) = wayshot::WayshotBackend::new() {
        tracing::info!("auto-detected capture backend: wayshot");
        return Ok(Box::new(ws));
    }

    // Try grim next (simple CLI, wlroots)
    if let Ok(g) = grim::GrimBackend::new().await {
        tracing::info!("auto-detected capture backend: grim");
        return Ok(Box::new(g));
    }

    // Fall back to portal (broadest support, may prompt user)
    if let Ok(p) = portal::PortalBackend::new().await {
        tracing::info!("auto-detected capture backend: portal");
        return Ok(Box::new(p));
    }

    anyhow::bail!("no usable capture backend found — install grim or ensure a Wayland compositor with wlr-screencopy or xdg-desktop-portal is running");
}
