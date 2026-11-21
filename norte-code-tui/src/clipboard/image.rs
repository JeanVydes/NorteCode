//! PNG encoding for clipboard images.

use arboard::ImageData;

use super::error::ClipboardError;

/// Encode RGBA clipboard pixels as a PNG the upload endpoint accepts.
pub(crate) fn encode_png(image: &ImageData<'_>) -> Result<Vec<u8>, ClipboardError> {
    let invalid_dimensions = || ClipboardError::Encoding {
        reason: "clipboard image dimensions do not fit a PNG header".to_owned(),
    };
    let width = u32::try_from(image.width).map_err(|_| invalid_dimensions())?;
    let height = u32::try_from(image.height).map_err(|_| invalid_dimensions())?;
    let mut png_bytes = Vec::new();
    {
        let mut png_encoder = png::Encoder::new(&mut png_bytes, width, height);
        png_encoder.set_color(png::ColorType::Rgba);
        png_encoder.set_depth(png::BitDepth::Eight);
        let mut writer = png_encoder
            .write_header()
            .map_err(|error| ClipboardError::Encoding {
                reason: error.to_string(),
            })?;
        writer
            .write_image_data(&image.bytes)
            .map_err(|error| ClipboardError::Encoding {
                reason: error.to_string(),
            })?;
    }
    Ok(png_bytes)
}
