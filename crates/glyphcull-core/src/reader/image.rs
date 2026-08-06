//! IMGS section decoder (SPEC.md §2.6): raster images — decoded raw pixels
//! (RGBA8 or RGB8), row-major, top-to-bottom, no padding. Runtimes upload
//! raw pixels and never decode image formats.

use crate::error::{Error, ErrorKind, Result};
use crate::limits::{MAX_IMAGE_COUNT, MAX_IMAGE_DIM};
use crate::reader::Cursor;

/// Image pixel formats (SPEC.md §2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ImageFormat {
    /// RGBA8 — 4 bytes per pixel.
    Rgba8 = 0,
    /// RGB8 — 3 bytes per pixel.
    Rgb8 = 1,
}

impl ImageFormat {
    /// The bytes per pixel.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
        }
    }
}

/// A raster image (SPEC.md §2.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRecord {
    /// Dense image id.
    pub id: u32,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// The pixel format.
    pub format: ImageFormat,
    /// Raw pixels, row-major, top-to-bottom.
    pub data: Vec<u8>,
}

/// Decode the IMGS payload (SPEC.md §2.6).
pub fn decode(payload: &[u8]) -> Result<Vec<ImageRecord>> {
    let mut c = Cursor::new(payload, 0, None);
    let image_count = c.u32("image count")?;
    if u64::from(image_count) > MAX_IMAGE_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("image count {image_count} > {MAX_IMAGE_COUNT}"),
        ));
    }
    let mut images = Vec::with_capacity(image_count as usize);
    for i in 0..image_count as usize {
        let id = c.u32("image id")?;
        if id != i as u32 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("image {i}: id {id} != dense id {i}"),
            ));
        }
        let width = c.u16("image width")?;
        let height = c.u16("image height")?;
        let format_value = c.u8("image format")?;
        let flags = c.u8("image flags")?;
        let byte_len = c.u32("image byte_len")?;
        if flags != 0 {
            return Err(Error::new(
                ErrorKind::InvalidFlags,
                format!("image {i}: reserved flags must be zero"),
            ));
        }
        let format = match format_value {
            0 => ImageFormat::Rgba8,
            1 => ImageFormat::Rgb8,
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    format!("image {i}: unknown format {other}"),
                ));
            }
        };
        if u32::from(width) > MAX_IMAGE_DIM as u32 || u32::from(height) > MAX_IMAGE_DIM as u32 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("image {i}: {width}x{height} exceeds the {MAX_IMAGE_DIM} px cap"),
            ));
        }
        let expected = (width as u32)
            .checked_mul(height as u32)
            .and_then(|n| n.checked_mul(format.bytes_per_pixel() as u32))
            .ok_or_else(|| {
                Error::new(ErrorKind::Overflow, format!("image {i}: byte_len overflow"))
            })?;
        if byte_len != expected {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("image {i}: byte_len {byte_len} != width × height × bpp ({expected})"),
            ));
        }
        let data = c.bytes(byte_len as usize, "image data")?.to_vec();
        images.push(ImageRecord {
            id,
            width,
            height,
            format,
            data,
        });
    }
    c.finish("IMGS payload")?;
    Ok(images)
}
