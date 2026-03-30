use anyhow::{Context, Result};
use std::future::Future;
use std::pin::Pin;

use super::{CaptureBackend, MonitorInfo, Screenshot};

/// Capture backend that shells out to the `grim` CLI.
/// Works on wlroots-based compositors (Sway, Hyprland, etc.).
pub struct GrimBackend {
    _priv: (),
}

impl GrimBackend {
    pub async fn new() -> Result<Self> {
        // Verify grim is installed
        let status = tokio::process::Command::new("which")
            .arg("grim")
            .status()
            .await
            .context("failed to check for grim")?;
        if !status.success() {
            anyhow::bail!("grim not found in PATH");
        }
        Ok(Self { _priv: () })
    }
}

impl CaptureBackend for GrimBackend {
    fn name(&self) -> &str {
        "grim"
    }

    fn capture(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Screenshot>>> + Send + '_>> {
        Box::pin(async {
            // Capture all outputs to PNG on stdout
            let output = tokio::process::Command::new("grim")
                .args(["-t", "png", "-"])
                .output()
                .await
                .context("failed to run grim")?;

            if !output.status.success() {
                anyhow::bail!(
                    "grim failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            let img =
                image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png)
                    .context("failed to decode grim PNG output")?;
            let (w, h) = (img.width(), img.height());

            Ok(vec![Screenshot {
                image: img,
                monitor: MonitorInfo {
                    name: "grim-all".into(),
                    width: w,
                    height: h,
                },
            }])
        })
    }
}
