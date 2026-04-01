pub mod benchmark;

use anyhow::{Context, Result};
use image::DynamicImage;
use leptess::LepTess;
use std::cell::RefCell;

/// Runtime OCR options shared by the daemon and benchmark tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrOptions {
    /// Downscale images wider than this many pixels before OCR.
    /// `None` keeps the original resolution.
    pub max_width: Option<u32>,
}

impl OcrOptions {
    pub fn from_config_width(max_width: u32) -> Self {
        Self {
            max_width: (max_width > 0).then_some(max_width),
        }
    }
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            max_width: Some(1280),
        }
    }
}

thread_local! {
    /// Reuse a single Tesseract instance per thread to avoid the cost of
    /// re-loading trained data on every call.
    static TESS: RefCell<Option<LepTess>> = const { RefCell::new(None) };
}

/// Extract text from an image using configurable OCR options.
pub fn extract_text_with_options(image: &DynamicImage, options: OcrOptions) -> Result<String> {
    let scaled;
    let img = match options.max_width {
        Some(max_width) if image.width() > max_width => {
            // Use a configurable downscale so OCR variants can be benchmarked
            // without duplicating the Tesseract call path.
            scaled = image.resize(max_width, max_width, image::imageops::FilterType::Triangle);
            &scaled
        }
        _ => image,
    };

    // Encode image as PNG in memory — leptess needs a file-format buffer, not raw pixels.
    let mut png_buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png_buf, image::ImageFormat::Png)
        .context("failed to encode image as PNG for OCR")?;
    let png_bytes = png_buf.into_inner();

    TESS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let tess = match slot.as_mut() {
            Some(t) => t,
            None => {
                *slot = Some(
                    LepTess::new(None, "eng")
                        .context("failed to initialise Tesseract")?,
                );
                slot.as_mut().unwrap()
            }
        };

        tess.set_image_from_mem(&png_bytes)
            .context("failed to set image for OCR")?;

        tess.get_utf8_text()
            .context("Tesseract OCR failed to extract text")
    })
}

#[cfg(test)]
mod tests {
    use super::OcrOptions;

    #[test]
    fn zero_width_disables_downscale() {
        assert_eq!(OcrOptions::from_config_width(0).max_width, None);
    }

    #[test]
    fn positive_width_enables_downscale() {
        assert_eq!(OcrOptions::from_config_width(1600).max_width, Some(1600));
    }

    #[test]
    fn default_width_matches_runtime_default() {
        assert_eq!(OcrOptions::default().max_width, Some(1280));
    }
}
