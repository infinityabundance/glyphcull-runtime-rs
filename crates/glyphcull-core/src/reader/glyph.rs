//! GLYF section decoder (SPEC.md §2.5): MSDF glyph atlases — descriptors,
//! glyph records (32 bytes each), kerning pairs (sorted by (left, right)),
//! and raw RGBA8 page texels. Pages are copied into owned buffers at decode
//! time; the renderer uploads them to the GPU once.

use crate::error::{Error, ErrorKind, Result};
use crate::limits::{
    MAX_ATLAS_COUNT, MAX_FAMILY_LEN, MAX_GLYPH_COUNT, MAX_KERNING_COUNT, MAX_PAGE_DIM,
};
use crate::reader::{Cursor, GLYPH_RECORD_LEN};

/// One glyph record (SPEC.md §2.5).
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRecord {
    /// The codepoint (unique within the atlas).
    pub codepoint: u32,
    /// Advance in em.
    pub advance: f32,
    /// Bearing x in em.
    pub bearing_x: f32,
    /// Bearing y in em.
    pub bearing_y: f32,
    /// Box left in texels (page space).
    pub box_x: u16,
    /// Box top in texels (page space).
    pub box_y: u16,
    /// Box width in texels (≥ 1; includes padding).
    pub box_w: u16,
    /// Box height in texels (≥ 1; includes padding).
    pub box_h: u16,
    /// The page this glyph's box lives in.
    pub page_index: u16,
    /// `no_outline` (space/combining) flag.
    pub no_outline: bool,
    /// `combining` (advance 0) flag.
    pub combining: bool,
}

/// One kerning pair (SPEC.md §2.5), sorted by (left, right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KerningPair {
    /// The left codepoint.
    pub left: u32,
    /// The right codepoint.
    pub right: u32,
    /// The advance adjustment in em.
    pub adjust: f32,
}

/// An MSDF atlas (SPEC.md §2.5).
#[derive(Debug, Clone, PartialEq)]
pub struct Atlas {
    /// The font id (index into the atlas table).
    pub font_id: u32,
    /// Glyph records in codepoint order.
    pub glyphs: Vec<GlyphRecord>,
    /// Page count.
    pub page_count: u16,
    /// The padding in texels around each glyph box.
    pub padding: u16,
    /// Fixed-point texels per em (×1024; 32768 ⇒ 32 texels/em).
    pub texels_per_em_raw: u32,
    /// Typographic ascent in em.
    pub ascent: f32,
    /// Descent in em (positive; below the baseline).
    pub descent: f32,
    /// Line gap in em.
    pub line_gap: f32,
    /// Cap height in em.
    pub cap_height: f32,
    /// X height in em.
    pub x_height: f32,
    /// Font units per em.
    pub units_per_em: f32,
    /// The font family name.
    pub family: String,
    /// The font weight (100..=900).
    pub weight: u16,
    /// Whether the face is italic.
    pub italic: bool,
    /// Page width in texels.
    pub page_width: u32,
    /// Page height in texels.
    pub page_height: u32,
    /// Kerning pairs sorted by (left, right).
    pub kerning: Vec<KerningPair>,
    /// Raw RGBA8 page texels (row-major, top-to-bottom).
    pub pages: Vec<Vec<u8>>,
}

impl Atlas {
    /// The texels per em as a float (SPEC.md §2.5: fixed-point ×1024).
    #[must_use]
    pub fn texels_per_em(&self) -> f32 {
        self.texels_per_em_raw as f32 / 1024.0
    }
}

/// Decode the GLYF payload (SPEC.md §2.5).
pub fn decode(payload: &[u8]) -> Result<Vec<Atlas>> {
    let mut c = Cursor::new(payload, 0, None);
    let atlas_count = c.u32("atlas count")?;
    if u64::from(atlas_count) > MAX_ATLAS_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas count {atlas_count} > {MAX_ATLAS_COUNT}"),
        ));
    }
    let mut atlases = Vec::with_capacity(atlas_count as usize);
    for a in 0..atlas_count as usize {
        atlases.push(decode_atlas(&mut c, a)?);
    }
    c.finish("GLYF payload")?;
    Ok(atlases)
}

fn decode_atlas(c: &mut Cursor<'_>, a: usize) -> Result<Atlas> {
    let font_id = c.u32("atlas font_id")?;
    let glyph_count = c.u32("atlas glyph count")?;
    if u64::from(glyph_count) > MAX_GLYPH_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: glyph count {glyph_count} > {MAX_GLYPH_COUNT}"),
        ));
    }
    let page_count = c.u16("atlas page count")?;
    let format = c.u8("atlas format")?;
    let flags = c.u8("atlas flags")?;
    let padding = c.u16("atlas padding")?;
    let texels_per_em_raw = c.u32("atlas texels_per_em")?;
    let ascent = c.f32("atlas ascent")?;
    let descent = c.f32("atlas descent")?;
    let line_gap = c.f32("atlas line_gap")?;
    let cap_height = c.f32("atlas cap_height")?;
    let x_height = c.f32("atlas x_height")?;
    let units_per_em = c.f32("atlas units_per_em")?;
    let family_len = c.u16("atlas family_len")?;
    if u64::from(family_len) > MAX_FAMILY_LEN {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: family_len {family_len} > {MAX_FAMILY_LEN}"),
        ));
    }
    let family = c.utf8(family_len as usize, "atlas family")?;
    let weight = c.u16("atlas weight")?;
    if !(100..=900).contains(&weight) {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: weight {weight} not in 100..=900"),
        ));
    }
    let italic = c.u8("atlas italic")?;
    let reserved = c.u8("atlas reserved")?;
    let page_width = c.u32("atlas page_width")?;
    let page_height = c.u32("atlas page_height")?;
    if format != 0 {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: format {format} != 0 (MSDF_RGBA8)"),
        ));
    }
    if flags != 0 || reserved != 0 {
        return Err(Error::new(
            ErrorKind::InvalidFlags,
            format!("atlas {a}: reserved flags/bits must be zero"),
        ));
    }
    if page_count == 0 {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: page_count must be at least 1"),
        ));
    }
    if u64::from(page_width) > MAX_PAGE_DIM || u64::from(page_height) > MAX_PAGE_DIM {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: page {page_width}x{page_height} exceeds {MAX_PAGE_DIM} texels"),
        ));
    }
    if italic > 1 {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: italic flag {italic} not in 0..=1"),
        ));
    }

    let mut glyphs = Vec::with_capacity(glyph_count as usize);
    for g in 0..glyph_count as usize {
        let rec = c.bytes(GLYPH_RECORD_LEN, "glyph record")?;
        let mut r = Cursor::new(rec, 0, None);
        let codepoint = r.u32("glyph codepoint")?;
        let advance = r.f32("glyph advance")?;
        let bearing_x = r.f32("glyph bearing_x")?;
        let bearing_y = r.f32("glyph bearing_y")?;
        let box_x = r.u16("glyph box_x")?;
        let box_y = r.u16("glyph box_y")?;
        let box_w = r.u16("glyph box_w")?;
        let box_h = r.u16("glyph box_h")?;
        let page_index = r.u16("glyph page_index")?;
        let glyph_flags = r.u8("glyph flags")?;
        let reserved1 = r.u8("glyph reserved1")?;
        let reserved4 = r.u32("glyph reserved4")?;
        r.finish("glyph record")?;

        if box_w == 0 || box_h == 0 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("atlas {a}: glyph {g}: box_w/box_h must be ≥ 1"),
            ));
        }
        if glyph_flags & !0x03 != 0 || reserved1 != 0 || reserved4 != 0 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("atlas {a}: glyph {g}: unknown flag bits or reserved bits"),
            ));
        }
        if page_index >= page_count {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!(
                    "atlas {a}: glyph {g}: page_index {page_index} out of range ({page_count})"
                ),
            ));
        }
        let box_end_x = u32::from(box_x) + u32::from(box_w);
        let box_end_y = u32::from(box_y) + u32::from(box_h);
        if box_end_x > page_width || box_end_y > page_height {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("atlas {a}: glyph {g}: box exceeds the page bounds"),
            ));
        }
        glyphs.push(GlyphRecord {
            codepoint,
            advance,
            bearing_x,
            bearing_y,
            box_x,
            box_y,
            box_w,
            box_h,
            page_index,
            no_outline: glyph_flags & 0x01 != 0,
            combining: glyph_flags & 0x02 != 0,
        });
    }

    let kerning_count = c.u32("atlas kerning count")?;
    if u64::from(kerning_count) > MAX_KERNING_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("atlas {a}: kerning count {kerning_count} > {MAX_KERNING_COUNT}"),
        ));
    }
    let mut kerning = Vec::with_capacity(kerning_count as usize);
    let mut prev: Option<(u32, u32)> = None;
    for k in 0..kerning_count as usize {
        let left = c.u32("kerning left")?;
        let right = c.u32("kerning right")?;
        let adjust = c.f32("kerning adjust")?;
        // Pairs must be sorted by (left, right) — determinism (SPEC.md §2.5).
        if let Some(p) = prev {
            if (left, right) <= p {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    format!("atlas {a}: kerning pair {k} is not strictly sorted"),
                ));
            }
        }
        prev = Some((left, right));
        kerning.push(KerningPair {
            left,
            right,
            adjust,
        });
    }

    let mut pages = Vec::with_capacity(page_count as usize);
    let page_bytes = (page_width as usize)
        .checked_mul(page_height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Overflow,
                format!("atlas {a}: page byte count overflow"),
            )
        })?;
    for _p in 0..page_count as usize {
        let raw = c.bytes(page_bytes, "atlas page")?;
        pages.push(raw.to_vec());
    }

    Ok(Atlas {
        font_id,
        glyphs,
        page_count,
        padding,
        texels_per_em_raw,
        ascent,
        descent,
        line_gap,
        cap_height,
        x_height,
        units_per_em,
        family,
        weight,
        italic: italic == 1,
        page_width,
        page_height,
        kerning,
        pages,
    })
}
