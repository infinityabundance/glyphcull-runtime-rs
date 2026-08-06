//! STYL section decoder (SPEC.md §2.3): the resolved style table. Each style
//! record carries a dense id and a property blob; properties are `u16 tag`
//! followed by a fixed-size value per tag. Unknown tags are an error (v1 is
//! strict, SPEC.md §4). The reader produces raw records; style resolution
//! (defaults applied) lives in the document layer.

use crate::error::{Error, ErrorKind, Result};
use crate::limits::{MAX_PROPERTIES_PER_STYLE, MAX_STYLE_COUNT};
use crate::reader::Cursor;

/// Style property tags (SPEC.md §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PropertyTag {
    /// `font_id`: u32 (index into GLYF atlases).
    FontId = 1,
    /// `font_size_px`: f32.
    FontSizePx = 2,
    /// `line_height`: f32 (multiplier of font size).
    LineHeight = 3,
    /// `font_weight`: u16 (100..=900).
    FontWeight = 4,
    /// `italic`: u8 0/1.
    Italic = 5,
    /// `color`: u32 RGBA.
    Color = 6,
    /// `background_color`: u32 RGBA.
    BackgroundColor = 7,
    /// `margin_top`: f32 (px).
    MarginTop = 8,
    /// `margin_bottom`: f32 (px).
    MarginBottom = 9,
    /// `text_align`: u8 (0 start, 1 center, 2 end, 3 justify).
    TextAlign = 10,
    /// `text_indent`: f32 (px).
    TextIndent = 11,
    /// `list_style`: u8 (0 none … 8 upper_roman).
    ListStyle = 12,
    /// `code`: u8 0/1.
    Code = 13,
    /// `underline`: u8 0/1.
    Underline = 14,
    /// `letter_spacing`: f32 (px).
    LetterSpacing = 15,
    /// `white_space`: u8 (0 normal, 1 pre, 2 nowrap).
    WhiteSpace = 16,
}

impl PropertyTag {
    /// Interpret a property tag; `None` for out-of-range tags.
    #[must_use]
    pub const fn from_code(code: u16) -> Option<Self> {
        match code {
            1 => Some(Self::FontId),
            2 => Some(Self::FontSizePx),
            3 => Some(Self::LineHeight),
            4 => Some(Self::FontWeight),
            5 => Some(Self::Italic),
            6 => Some(Self::Color),
            7 => Some(Self::BackgroundColor),
            8 => Some(Self::MarginTop),
            9 => Some(Self::MarginBottom),
            10 => Some(Self::TextAlign),
            11 => Some(Self::TextIndent),
            12 => Some(Self::ListStyle),
            13 => Some(Self::Code),
            14 => Some(Self::Underline),
            15 => Some(Self::LetterSpacing),
            16 => Some(Self::WhiteSpace),
            _ => None,
        }
    }
}

/// The typed value of a style property (SPEC.md §2.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropertyValue {
    /// A `u32` value (font_id, color, background_color).
    U32(u32),
    /// An `f32` value (font_size_px, line_height, margins, letter_spacing).
    F32(f32),
    /// A `u16` value (font_weight).
    U16(u16),
    /// A `u8` value (flags, enum-like tags).
    U8(u8),
}

/// One resolved style property (SPEC.md §2.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleProperty {
    /// The property tag.
    pub tag: PropertyTag,
    /// The typed value.
    pub value: PropertyValue,
}

/// A style record: an id plus explicit properties (SPEC.md §2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct StyleRecord {
    /// Dense style id (0 = the implicit document default).
    pub id: u32,
    /// Explicit properties (absent properties take the SPEC defaults).
    pub properties: Vec<StyleProperty>,
}

/// Decode the STYL payload (SPEC.md §2.3).
pub fn decode(payload: &[u8]) -> Result<Vec<StyleRecord>> {
    let mut c = Cursor::new(payload, 0, None);
    let style_count = c.u32("style count")?;
    if u64::from(style_count) > MAX_STYLE_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("style count {style_count} > {MAX_STYLE_COUNT}"),
        ));
    }
    let mut styles = Vec::with_capacity(style_count as usize);
    for i in 0..style_count as usize {
        let id = c.u32("style id")?;
        if id != i as u32 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("style {i}: id {id} != dense id {i}"),
            ));
        }
        let property_count = c.u16("style property count")?;
        let blob_len = c.u16("style blob len")?;
        if u64::from(property_count) > MAX_PROPERTIES_PER_STYLE {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("style {i}: {property_count} properties > {MAX_PROPERTIES_PER_STYLE}"),
            ));
        }
        let blob = c.bytes(blob_len as usize, "style blob")?;
        let mut b = Cursor::new(blob, 0, None);
        let mut properties = Vec::with_capacity(property_count as usize);
        for _p in 0..property_count as usize {
            let tag_value = b.u16("property tag")?;
            let tag = PropertyTag::from_code(tag_value).ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidValue,
                    format!("style {i}: unknown property tag {tag_value}"),
                )
            })?;
            let value = match tag {
                PropertyTag::FontId => PropertyValue::U32(b.u32("property value")?),
                PropertyTag::FontSizePx => PropertyValue::F32(b.f32("property value")?),
                PropertyTag::LineHeight => PropertyValue::F32(b.f32("property value")?),
                PropertyTag::FontWeight => PropertyValue::U16(b.u16("property value")?),
                PropertyTag::Italic => PropertyValue::U8(b.u8("property value")?),
                PropertyTag::Color => PropertyValue::U32(b.u32("property value")?),
                PropertyTag::BackgroundColor => PropertyValue::U32(b.u32("property value")?),
                PropertyTag::MarginTop => PropertyValue::F32(b.f32("property value")?),
                PropertyTag::MarginBottom => PropertyValue::F32(b.f32("property value")?),
                PropertyTag::TextAlign => PropertyValue::U8(b.u8("property value")?),
                PropertyTag::TextIndent => PropertyValue::F32(b.f32("property value")?),
                PropertyTag::ListStyle => PropertyValue::U8(b.u8("property value")?),
                PropertyTag::Code => PropertyValue::U8(b.u8("property value")?),
                PropertyTag::Underline => PropertyValue::U8(b.u8("property value")?),
                PropertyTag::LetterSpacing => PropertyValue::F32(b.f32("property value")?),
                PropertyTag::WhiteSpace => PropertyValue::U8(b.u8("property value")?),
            };
            properties.push(StyleProperty { tag, value });
        }
        b.finish("style blob")?;
        styles.push(StyleRecord { id, properties });
    }
    c.finish("STYL payload")?;
    Ok(styles)
}
