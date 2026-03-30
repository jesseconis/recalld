use anyhow::{Context, Result};
use std::future::Future;
use std::pin::Pin;

use super::{CaptureBackend, MonitorInfo, Screenshot};

/// Portal-based screenshot capture using `org.freedesktop.portal.Screenshot`.
/// Works across all Wayland compositors that implement xdg-desktop-portal.
pub struct PortalBackend {
    _priv: (),
}

impl PortalBackend {
    pub async fn new() -> Result<Self> {
        // Verify portal is reachable by checking for the gnome-screenshot or
        // xdg-desktop-portal-based tool.
        let status = tokio::process::Command::new("which")
            .arg("xdg-desktop-portal")
            .status()
            .await;
        // We don't hard-fail here — the portal might still work at runtime.
        let _ = status;
        Ok(Self { _priv: () })
    }
}

impl CaptureBackend for PortalBackend {
    fn name(&self) -> &str {
        "portal"
    }

    fn capture(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Screenshot>>> + Send + '_>> {
        Box::pin(async {
            // Use gdbus to call the portal Screenshot method.
            // This avoids the complexity of subscribing to D-Bus Response signals
            // directly. The portal writes the screenshot to a temp file and returns its URI.
            let output = tokio::process::Command::new("gdbus")
                .args([
                    "call",
                    "--session",
                    "--dest=org.freedesktop.portal.Desktop",
                    "--object-path=/org/freedesktop/portal/desktop",
                    "--method=org.freedesktop.portal.Screenshot.Screenshot",
                    "",
                    "{'interactive': <false>}",
                ])
                .output()
                .await
                .context("failed to invoke portal screenshot via gdbus")?;

            if !output.status.success() {
                anyhow::bail!(
                    "portal screenshot failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            // Portal returns a file URI — parse it and load the image.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let uri = stdout
                .split('\'')
                .find(|s| s.starts_with("file://"))
                .context("could not parse screenshot URI from portal response")?;

            let path = uri.strip_prefix("file://").unwrap_or(uri);
            let img = image::open(path).context("failed to open portal screenshot")?;
            let (w, h) = (img.width(), img.height());

            // Clean up the temp file
            let _ = std::fs::remove_file(path);

            Ok(vec![Screenshot {
                image: img,
                monitor: MonitorInfo {
                    name: "portal".into(),
                    width: w,
                    height: h,
                },
            }])
        })
    }
}
