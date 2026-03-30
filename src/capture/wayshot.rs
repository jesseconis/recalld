use anyhow::{Context, Result};
use std::future::Future;
use std::pin::Pin;

use super::{CaptureBackend, MonitorInfo, Screenshot};

/// Native Wayland screenshot capture using libwayshot (wlr-screencopy protocol).
/// Best performance on wlroots compositors (Sway, Hyprland, river, etc.).
pub struct WayshotBackend {
    connection: libwayshot::WayshotConnection,
}

impl WayshotBackend {
    pub fn new() -> Result<Self> {
        let connection = libwayshot::WayshotConnection::new()
            .context("failed to connect via wlr-screencopy — is a wlroots compositor running?")?;
        Ok(Self { connection })
    }
}

impl CaptureBackend for WayshotBackend {
    fn name(&self) -> &str {
        "wayshot"
    }

    fn capture(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Screenshot>>> + Send + '_>> {
        Box::pin(async {
            let outputs = self.connection.get_all_outputs();
            if outputs.is_empty() {
                anyhow::bail!("wayshot: no outputs found");
            }

            let mut screenshots = Vec::with_capacity(outputs.len());

            for output in outputs {
                let img = self
                    .connection
                    .screenshot_single_output(output, false)
                    .context("wayshot: failed to capture output")?;

                let (w, h) = (img.width(), img.height());
                screenshots.push(Screenshot {
                    image: img,
                    monitor: MonitorInfo {
                        name: output.name.clone(),
                        width: w,
                        height: h,
                    },
                });
            }

            Ok(screenshots)
        })
    }
}
