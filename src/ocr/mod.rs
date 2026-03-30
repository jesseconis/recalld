use anyhow::{Context, Result};
use image::DynamicImage;
use leptess::LepTess;
use std::cell::RefCell;

thread_local! {
    /// Reuse a single Tesseract instance per thread to avoid the cost of
    /// re-loading trained data on every call.
    static TESS: RefCell<Option<LepTess>> = const { RefCell::new(None) };
}

/// Extract text from an image using Tesseract OCR.
///
/// Downscales images larger than 1280px wide to keep CPU usage low.
/// Reuses a thread-local Tesseract instance to avoid repeated init overhead.
pub fn extract_text(image: &DynamicImage) -> Result<String> {
    // Downscale aggressively — Tesseract works fine at 1280px for screen text
    // and this cuts pixel count roughly in half vs 1920.
    let scaled;
    let img = if image.width() > 1280 {
        scaled = image.resize(1280, 1280, image::imageops::FilterType::Triangle);
        &scaled
    } else {
        image
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
