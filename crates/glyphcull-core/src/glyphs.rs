//! The glyph cache (Architecture.md §3.6; mirrors the JS
//! `src/glyphs/cache.ts`).
//!
//! Glyph instances are cached per (atlas, codepoint, size, color) quad as
//! prepared stamps: the pixel-space quad + UV rect + metrics needed to emit
//! a draw command, derived from the atlas glyph record and the placement
//! convention (SPEC.md §2.5):
//!
//! ```text
//! scale        = fontSizePx / texelsPerEm
//! inkLeftPx    = penX + bearingX * fontSizePx
//! inkTopPx     = baselineY - bearingY * fontSizePx
//! boxLeftPx    = inkLeftPx - padding * scale      (box = ink + padding)
//! boxTopPx     = inkTopPx - padding * scale
//! quad size    = boxW * scale, boxH * scale
//! uv           = box rect / page size (page texels)
//! ```
//!
//! The cache is budgeted in bytes; when the budget is exceeded the least
//! recently used entries are evicted (deterministically — a monotonic touch
//! counter orders the LRU chain, which is exactly the JS `Map` re-insertion
//! order). Chunks own their stamps: `release_chunk` drops every stamp of an
//! evicted chunk (lifecycle coupling — Evicted ⇒ cache entries released);
//! stamps shared with live chunks survive.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::layout::measure::glyph_for;
use crate::reader::glyph::Atlas;

/// The fixed per-stamp overhead the byte budget accounts for (mirrors the JS
/// `STAMP_OVERHEAD`).
pub const STAMP_OVERHEAD: u64 = 128;
/// The stamp payload estimate (quad + key + metrics; mirrors the JS `+ 64`).
pub const STAMP_PAYLOAD: u64 = 64;

/// The cache key of a prepared glyph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphKey {
    /// The atlas (font) id.
    pub atlas_id: u32,
    /// The codepoint (Unicode scalar value).
    pub codepoint: u32,
    /// The font size the stamp was prepared for (document px).
    pub font_size_px: f32,
    /// The color the stamp was prepared for (RGBA).
    pub color: u32,
}

/// A prepared, size-specific glyph stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphStamp {
    /// The cache key.
    pub key: GlyphKey,
    /// The atlas page this glyph's box lives in.
    pub page_index: u16,
    /// UV rect in texture space: `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    /// The quad's left offset relative to the pen (penX + offsetX).
    pub offset_x: f32,
    /// The quad's top offset relative to the baseline (baselineY − offsetY;
    /// page y grows down, so the box top is `baselineY + offsetY`).
    pub offset_y: f32,
    /// Quad width in pixels.
    pub quad_w: f32,
    /// Quad height in pixels.
    pub quad_h: f32,
    /// Advance in pixels (f64: f32 atlas advance widened, matching the JS
    /// `number`; DESIGN.md D24).
    pub advance_px: f64,
    /// Whether the glyph has no outline (space/combining).
    pub no_outline: bool,
    /// Whether the glyph is a combining mark (advance 0).
    pub combining: bool,
    /// The atlas page texture width (for shader normalization).
    pub page_width: u32,
    /// The atlas page texture height.
    pub page_height: u32,
    /// The atlas density (texels per em).
    pub texels_per_em: f32,
    /// Estimated bytes (quad + key + fixed overhead).
    pub size_bytes: u64,
}

/// Prepare a glyph stamp for a codepoint at a size and color. Returns `None`
/// when the atlas has no record for the codepoint (the layout renders a tofu
/// box instead).
#[must_use]
pub fn prepare_glyph(
    atlas: &Atlas,
    codepoint: u32,
    font_size_px: f32,
    color: u32,
) -> Option<GlyphStamp> {
    let glyph = glyph_for(atlas, codepoint)?;
    let tpe = atlas.texels_per_em();
    let scale = if tpe > 0.0 {
        font_size_px / tpe
    } else {
        font_size_px
    };
    let padding = f32::from(atlas.padding) * scale;
    let ink_left_px = glyph.bearing_x * font_size_px;
    let ink_top_px = glyph.bearing_y * font_size_px;
    // Page y grows down; the box top sits above the baseline, so the offset
    // from the baseline is `+ padding` (mirrors the JS sign convention).
    let offset_x = ink_left_px - padding;
    let offset_y = ink_top_px + padding;
    let quad_w = f32::from(glyph.box_w) * scale;
    let quad_h = f32::from(glyph.box_h) * scale;
    let page_w = atlas.page_width as f32;
    let page_h = atlas.page_height as f32;
    let u0 = f32::from(glyph.box_x) / page_w;
    let v0 = f32::from(glyph.box_y) / page_h;
    let u1 = (f32::from(glyph.box_x) + f32::from(glyph.box_w)) / page_w;
    let v1 = (f32::from(glyph.box_y) + f32::from(glyph.box_h)) / page_h;
    Some(GlyphStamp {
        key: GlyphKey {
            atlas_id: atlas.font_id,
            codepoint,
            font_size_px,
            color,
        },
        page_index: glyph.page_index,
        uv: [u0, v0, u1, v1],
        offset_x,
        offset_y,
        quad_w,
        quad_h,
        advance_px: f64::from(glyph.advance) * f64::from(font_size_px),
        no_outline: glyph.no_outline,
        combining: glyph.combining,
        page_width: atlas.page_width,
        page_height: atlas.page_height,
        texels_per_em: tpe,
        size_bytes: STAMP_OVERHEAD + STAMP_PAYLOAD,
    })
}

/// The internal map key: the [`GlyphKey`] quad with the f32 font size
/// canonicalized to its bits (deterministic `Ord`).
type CacheKey = (u32, u32, u32, u32);

fn key_of(key: GlyphKey) -> CacheKey {
    (
        key.atlas_id,
        key.codepoint,
        key.font_size_px.to_bits(),
        key.color,
    )
}

/// The glyph cache: budgeted, deterministic, and chunk-owning
/// (`release_chunk` frees a chunk's stamps).
///
/// LRU order is a monotonic touch counter: every `get`/`put` takes the next
/// counter value, so the least recently used stamp is the one with the
/// smallest counter — exactly the JS `Map` insertion/re-insertion order. The
/// budget is `u64`, so invalid (negative/NaN) budgets are unrepresentable
/// (the JS rejects them with a `RangeError`; DESIGN.md R4).
pub struct GlyphCache {
    budget_bytes: u64,
    /// key → (stamp, last-touch counter), the authority for `bytes`.
    stamps: BTreeMap<CacheKey, (GlyphStamp, u64)>,
    /// (counter, key) → eviction order, least recently used first.
    order: BTreeMap<(u64, CacheKey), ()>,
    /// key → owning chunk ids (a stamp may be shared by several runs).
    owners: BTreeMap<CacheKey, BTreeSet<u32>>,
    /// The current byte usage.
    bytes: u64,
    /// The next touch counter value.
    next_tick: u64,
}

impl GlyphCache {
    /// Create a cache with the given byte budget.
    #[must_use]
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            stamps: BTreeMap::new(),
            order: BTreeMap::new(),
            owners: BTreeMap::new(),
            bytes: 0,
            next_tick: 0,
        }
    }

    /// The current byte usage.
    #[must_use]
    pub const fn used_bytes(&self) -> u64 {
        self.bytes
    }

    /// The configured budget.
    #[must_use]
    pub const fn budget(&self) -> u64 {
        self.budget_bytes
    }

    /// The number of cached stamps.
    #[must_use]
    pub fn size(&self) -> usize {
        self.stamps.len()
    }

    /// Fetch a stamp (touching its LRU position), or `None`.
    pub fn get(&mut self, key: GlyphKey) -> Option<GlyphStamp> {
        let id = key_of(key);
        let (stamp, counter) = self.stamps.get(&id).copied()?;
        self.order.remove(&(counter, id));
        let tick = self.take_tick();
        self.stamps.insert(id, (stamp, tick));
        self.order.insert((tick, id), ());
        Some(stamp)
    }

    /// Whether a stamp is cached.
    #[must_use]
    pub fn has(&self, key: GlyphKey) -> bool {
        self.stamps.contains_key(&key_of(key))
    }

    /// Store a stamp owned by `owner_chunk_id`. When the budget is exceeded,
    /// the least recently used stamps are evicted (deterministically).
    pub fn put(&mut self, key: GlyphKey, stamp: GlyphStamp, owner_chunk_id: u32) {
        let id = key_of(key);
        if let Some((_, counter)) = self.stamps.get(&id) {
            // Refresh ownership and LRU position.
            let counter = *counter;
            self.order.remove(&(counter, id));
            let tick = self.take_tick();
            self.stamps.insert(id, (stamp, tick));
            self.order.insert((tick, id), ());
            if let Some(set) = self.owners.get_mut(&id) {
                set.insert(owner_chunk_id);
            }
            return;
        }
        let tick = self.take_tick();
        self.stamps.insert(id, (stamp, tick));
        self.order.insert((tick, id), ());
        self.bytes += stamp.size_bytes;
        self.owners.entry(id).or_default().insert(owner_chunk_id);
        self.evict_lru();
    }

    /// Evict least-recently-used stamps until the budget is satisfied.
    fn evict_lru(&mut self) {
        while self.bytes > self.budget_bytes {
            let Some(((_, id), _)) = self.order.pop_first() else {
                break;
            };
            let Some((stamp, _)) = self.stamps.remove(&id) else {
                continue;
            };
            self.owners.remove(&id);
            self.bytes = self.bytes.saturating_sub(stamp.size_bytes);
        }
    }

    /// Release every stamp owned by a chunk (called when the chunk is Evicted
    /// by the lifecycle). Stamps shared with live chunks survive. Returns the
    /// freed bytes.
    pub fn release_chunk(&mut self, chunk_id: u32) -> u64 {
        let mut freed = 0_u64;
        let mut empty_keys: Vec<CacheKey> = Vec::new();
        for (id, owners) in self.owners.iter_mut() {
            owners.remove(&chunk_id);
            if owners.is_empty() {
                empty_keys.push(*id);
            }
        }
        for id in empty_keys {
            let (stamp, counter) = match self.stamps.remove(&id) {
                Some(entry) => entry,
                None => continue,
            };
            self.order.remove(&(counter, id));
            self.owners.remove(&id);
            self.bytes = self.bytes.saturating_sub(stamp.size_bytes);
            freed += stamp.size_bytes;
        }
        freed
    }

    /// The chunk ids owning a given key (for lifecycle coupling tests),
    /// ascending.
    #[must_use]
    pub fn owners_of(&self, key: GlyphKey) -> Vec<u32> {
        self.owners
            .get(&key_of(key))
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Drop everything (destroy).
    pub fn clear(&mut self) {
        self.stamps.clear();
        self.order.clear();
        self.owners.clear();
        self.bytes = 0;
    }

    fn take_tick(&mut self) -> u64 {
        let tick = self.next_tick;
        self.next_tick += 1;
        tick
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the cache against a hand-built atlas (the golden-based
    //! mirror vectors live in `tests/glyph_cache.rs`).
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::reader::glyph::{GlyphRecord, KerningPair};

    fn glyph(codepoint: u32, advance: f32) -> GlyphRecord {
        GlyphRecord {
            codepoint,
            advance,
            bearing_x: 0.05,
            bearing_y: 0.7,
            box_x: 2,
            box_y: 3,
            box_w: 16,
            box_h: 24,
            page_index: 0,
            no_outline: false,
            combining: false,
        }
    }

    fn atlas() -> Atlas {
        Atlas {
            font_id: 0,
            glyphs: vec![glyph(0x41, 0.6)],
            page_count: 1,
            padding: 2,
            texels_per_em_raw: 32768,
            ascent: 0.8,
            descent: 0.2,
            line_gap: 0.0,
            cap_height: 0.7,
            x_height: 0.5,
            units_per_em: 1000.0,
            family: "test".into(),
            weight: 400,
            italic: false,
            page_width: 256,
            page_height: 256,
            kerning: Vec::<KerningPair>::new(),
            pages: vec![vec![0u8; 256 * 256 * 4]],
        }
    }

    #[test]
    fn stamp_follows_the_placement_convention() {
        let atlas = atlas();
        let stamp = prepare_glyph(&atlas, 0x41, 16.0, 0x0000_00ff).expect("stamp");
        let glyph = glyph_for(&atlas, 0x41).expect("glyph");
        let tpe = atlas.texels_per_em();
        let scale = 16.0 / tpe;
        assert!((stamp.quad_w - f32::from(glyph.box_w) * scale).abs() < 1e-4);
        assert!((stamp.quad_h - f32::from(glyph.box_h) * scale).abs() < 1e-4);
        let padding = f32::from(atlas.padding) * scale;
        assert!((stamp.offset_x - (glyph.bearing_x * 16.0 - padding)).abs() < 1e-4);
        assert!((stamp.offset_y - (glyph.bearing_y * 16.0 + padding)).abs() < 1e-4);
        assert!((stamp.advance_px - f64::from(glyph.advance) * 16.0).abs() < 1e-9);
        // UVs are within [0, 1] and non-degenerate.
        let [u0, v0, u1, v1] = stamp.uv;
        assert!(u0 >= 0.0 && v0 >= 0.0 && u1 <= 1.0 && v1 <= 1.0);
        assert!(u1 > u0 && v1 > v0);
        assert_eq!(stamp.size_bytes, STAMP_OVERHEAD + STAMP_PAYLOAD);
    }

    #[test]
    fn missing_codepoints_have_no_stamp() {
        let atlas = atlas();
        assert!(prepare_glyph(&atlas, 0x10ffff, 16.0, 0).is_none());
    }

    #[test]
    fn budget_zero_evicts_everything() {
        let atlas = atlas();
        let mut cache = GlyphCache::new(0);
        let stamp = prepare_glyph(&atlas, 0x41, 16.0, 0).expect("stamp");
        cache.put(stamp.key, stamp, 1);
        assert_eq!(cache.size(), 0, "a zero budget holds nothing");
        assert_eq!(cache.used_bytes(), 0);
    }
}
