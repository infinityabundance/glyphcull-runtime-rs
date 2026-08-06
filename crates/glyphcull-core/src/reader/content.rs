//! CONT section decoder (SPEC.md §2.4): content payloads — UTF-8 text and
//! image references. Payload ids are dense `0..payload_count` in emission
//! order; text bytes are preserved verbatim (whitespace policy is the
//! writer's).

use crate::error::{Error, ErrorKind, Result};
use crate::limits::MAX_CONTENT_COUNT;
use crate::reader::Cursor;

/// Payload kind codes (SPEC.md §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadKind {
    /// `text_utf8` — raw UTF-8 (NFC-normalized by the writer).
    TextUtf8 = 0,
    /// `image_ref` — a `u32` image id into IMGS.
    ImageRef = 1,
}

impl PayloadKind {
    /// Interpret a payload kind code; `None` for out-of-range codes.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::TextUtf8),
            1 => Some(Self::ImageRef),
            _ => None,
        }
    }
}

/// The decoded data of a content payload (SPEC.md §2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentData {
    /// A text payload: the raw text bytes as UTF-8.
    Text(String),
    /// An image reference: the image id into IMGS.
    ImageRef(u32),
}

/// A content payload (SPEC.md §2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentPayload {
    /// Dense payload id (`0..payload_count`).
    pub id: u32,
    /// The payload kind.
    pub kind: PayloadKind,
    /// The decoded data.
    pub data: ContentData,
}

/// Decode the CONT payload (SPEC.md §2.4).
pub fn decode(payload: &[u8]) -> Result<Vec<ContentPayload>> {
    let mut c = Cursor::new(payload, 0, None);
    let payload_count = c.u32("payload count")?;
    if u64::from(payload_count) > MAX_CONTENT_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("payload count {payload_count} > {MAX_CONTENT_COUNT}"),
        ));
    }
    let mut out = Vec::with_capacity(payload_count as usize);
    for i in 0..payload_count as usize {
        let id = c.u32("payload id")?;
        if id != i as u32 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("payload {i}: id {id} != dense id {i}"),
            ));
        }
        let kind_value = c.u8("payload kind")?;
        let kind = PayloadKind::from_code(kind_value).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidValue,
                format!("payload {i}: unknown kind {kind_value}"),
            )
        })?;
        let payload_flags = c.u8("payload flags")?;
        let reserved = c.u16("payload reserved")?;
        let byte_len = c.u32("payload byte_len")?;
        if payload_flags != 0 || reserved != 0 {
            return Err(Error::new(
                ErrorKind::InvalidFlags,
                format!("payload {i}: reserved flags/reserved bits must be zero"),
            ));
        }
        let data = match kind {
            PayloadKind::TextUtf8 => {
                let text = c.utf8(byte_len as usize, "payload text")?;
                ContentData::Text(text)
            }
            PayloadKind::ImageRef => {
                if byte_len != 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidValue,
                        format!("payload {i}: image_ref byte_len {byte_len} != 4"),
                    ));
                }
                let image_id = c.u32("payload image id")?;
                ContentData::ImageRef(image_id)
            }
        };
        out.push(ContentPayload { id, kind, data });
    }
    c.finish("CONT payload")?;
    Ok(out)
}
