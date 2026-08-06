//! Glyph cache tests: stamp preparation, budgeting, LRU eviction, and
//! lifecycle coupling (release_chunk) — mirrors the JS
//! `test/glyphs/cache.test.ts` vector for vector.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::glyphs::{prepare_glyph, GlyphCache, GlyphKey, STAMP_OVERHEAD, STAMP_PAYLOAD};
use glyphcull_core::reader::glyph::Atlas;
use glyphcull_core::reader::parse;

fn golden_atlas(font_id: usize) -> Atlas {
    let pkg = parse(common::pipeline_golden()).expect("parses");
    let atlases = pkg
        .atlases()
        .expect("has atlases")
        .expect("atlases present");
    atlases[font_id].clone()
}

/// A codepoint present in the golden's atlas 0 ('Deterministic…').
const D: u32 = 'D' as u32;
const E: u32 = 'e' as u32;

/// A codepoint definitely absent from the golden atlas.
const MISSING: u32 = 0x10ffff;

#[test]
fn produces_a_stamp_consistent_with_the_placement_convention() {
    let atlas = golden_atlas(0);
    let glyph = atlas
        .glyphs
        .binary_search_by(|g| g.codepoint.cmp(&D))
        .ok()
        .and_then(|i| atlas.glyphs.get(i))
        .expect("D glyph");
    let stamp = prepare_glyph(&atlas, D, 16.0, 0x0000_00ff).expect("stamp");
    let tpe = atlas.texels_per_em();
    let scale = 16.0 / tpe;
    assert!((stamp.quad_w - f32::from(glyph.box_w) * scale).abs() < 1e-4);
    assert!((stamp.quad_h - f32::from(glyph.box_h) * scale).abs() < 1e-4);
    let padding = f32::from(atlas.padding) * scale;
    assert!((stamp.offset_x - (glyph.bearing_x * 16.0 - padding)).abs() < 1e-4);
    assert!((stamp.offset_y - (glyph.bearing_y * 16.0 + padding)).abs() < 1e-4);
    assert!((stamp.advance_px - f64::from(glyph.advance) * 16.0).abs() < 1e-9);
    // UVs are within [0, 1].
    let [u0, v0, u1, v1] = stamp.uv;
    assert!(u0 >= 0.0 && v0 >= 0.0);
    assert!(u1 <= 1.0 && v1 <= 1.0);
    assert!(u1 > u0 && v1 > v0);
}

#[test]
fn returns_none_for_missing_codepoints() {
    let atlas = golden_atlas(0);
    assert!(prepare_glyph(&atlas, MISSING, 16.0, 0).is_none());
}

#[test]
fn scales_the_quad_with_the_font_size() {
    let atlas = golden_atlas(0);
    let small = prepare_glyph(&atlas, D, 16.0, 0).expect("small");
    let large = prepare_glyph(&atlas, D, 32.0, 0).expect("large");
    assert!((large.quad_w - small.quad_w * 2.0).abs() < 1e-4);
    assert!((large.advance_px - small.advance_px * 2.0).abs() < 1e-9);
}

#[test]
fn separates_stamps_by_color() {
    let atlas = golden_atlas(0);
    let a = prepare_glyph(&atlas, D, 16.0, 0xff00_00ff).expect("a");
    let b = prepare_glyph(&atlas, D, 16.0, 0x00ff_00ff).expect("b");
    assert_ne!(a.key.color, b.key.color);
}

#[test]
fn stores_and_fetches_stamps_with_lru_touch() {
    let atlas = golden_atlas(0);
    let mut cache = GlyphCache::new(1 << 20);
    let stamp = prepare_glyph(&atlas, D, 16.0, 0).expect("stamp");
    cache.put(stamp.key, stamp, 7);
    assert!(cache.has(stamp.key));
    let fetched = cache.get(stamp.key).expect("fetched");
    assert_eq!(fetched, stamp);
    assert_eq!(cache.size(), 1);
}

#[test]
fn enforces_the_byte_budget_by_evicting_the_least_recently_used() {
    let atlas = golden_atlas(0);
    // Budget fits exactly one stamp (192 bytes).
    let mut cache = GlyphCache::new(200);
    let stamps = [D, E, 't' as u32].map(|cp| prepare_glyph(&atlas, cp, 16.0, 0).expect("stamp"));
    cache.put(stamps[0].key, stamps[0], 1);
    cache.put(stamps[1].key, stamps[1], 2);
    // A and B: the first is evicted when B arrives.
    assert!(!cache.has(stamps[0].key), "A evicted");
    assert!(cache.has(stamps[1].key), "B cached");
    // Touching B, then inserting C evicts B.
    let _ = cache.get(stamps[1].key);
    cache.put(stamps[2].key, stamps[2], 3);
    assert!(!cache.has(stamps[1].key), "B evicted after touch + C");
    assert!(cache.has(stamps[2].key), "C cached");
    assert!(cache.used_bytes() <= cache.budget());
}

#[test]
fn an_unlimited_budget_never_evicts() {
    let atlas = golden_atlas(0);
    let mut cache = GlyphCache::new(u64::MAX);
    let stamps = [D, E, 't' as u32, 'o' as u32, 'n' as u32]
        .map(|cp| prepare_glyph(&atlas, cp, 16.0, 0).expect("stamp"));
    for s in stamps {
        cache.put(s.key, s, 1);
    }
    assert_eq!(cache.size(), 5);
    assert_eq!(cache.used_bytes(), 5 * (STAMP_OVERHEAD + STAMP_PAYLOAD));
}

#[test]
fn release_chunk_frees_chunk_owned_stamps_and_keeps_shared_ones() {
    let atlas = golden_atlas(0);
    let mut cache = GlyphCache::new(1 << 20);
    let stamp_a = prepare_glyph(&atlas, D, 16.0, 0).expect("A");
    let stamp_b = prepare_glyph(&atlas, E, 16.0, 0).expect("B");
    cache.put(stamp_a.key, stamp_a, 10);
    cache.put(stamp_b.key, stamp_b, 20);
    // 'A' shared by chunks 10 and 11.
    cache.put(stamp_a.key, stamp_a, 11);
    let freed = cache.release_chunk(10);
    assert_eq!(freed, 0, "A survives via chunk 11");
    assert!(cache.has(stamp_a.key));
    assert_eq!(cache.owners_of(stamp_a.key), vec![11]);
    let freed_b = cache.release_chunk(20);
    assert_eq!(freed_b, stamp_b.size_bytes);
    assert!(!cache.has(stamp_b.key));
    assert_eq!(cache.size(), 1);
}

#[test]
fn clear_drops_everything() {
    let atlas = golden_atlas(0);
    let mut cache = GlyphCache::new(1 << 20);
    let stamp = prepare_glyph(&atlas, D, 16.0, 0).expect("stamp");
    cache.put(stamp.key, stamp, 1);
    cache.clear();
    assert_eq!(cache.size(), 0);
    assert_eq!(cache.used_bytes(), 0);
}

#[test]
fn key_round_trips_through_get_and_put() {
    let atlas = golden_atlas(0);
    let mut cache = GlyphCache::new(1 << 20);
    let key = GlyphKey {
        atlas_id: 0,
        codepoint: D,
        font_size_px: 16.0,
        color: 0x1234_5678,
    };
    let stamp = prepare_glyph(&atlas, key.codepoint, key.font_size_px, key.color).expect("stamp");
    assert_eq!(stamp.key, key);
    cache.put(key, stamp, 1);
    let fetched = cache.get(key).expect("fetched");
    assert_eq!(fetched.key, key);
}
