//! Draw list tests (TESTING.md §2 unit/render): deterministic construction
//! (double-build equality), z-order (selection beneath content, backgrounds
//! before glyphs), command geometry, list-marker emission, resilience to
//! missing stamps, and the R5 divergence (nested images/rules render) —
//! mirrors the JS `test/render/drawlist.test.ts`.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use glyphcull_core::document::build_document;
use glyphcull_core::draw_list::{
    DrawCommand, DrawListBuilder, GlyphCommand, ImageCommand, RulerCommand, TextureResolver,
    SELECTION_COLOR,
};
use glyphcull_core::glyphs::{prepare_glyph, GlyphStamp};
use glyphcull_core::layout::layout::{BlockLayout, GlyphInstance, LayoutEngine, LayoutOptions};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::glyph::Atlas;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::SelectionQuad;

/// A fully laid-out golden document: engine + block ids in document order.
fn with_golden<R>(f: impl FnOnce(&LayoutEngine<'_>, Vec<u32>) -> R) -> R {
    let pkg = parse(common::pipeline_golden()).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let visible_ids: Vec<u32> = engine.records_all().keys().copied().collect();
    f(&engine, visible_ids)
}

/// A deterministic texture resolver: page (atlasId, pageIndex) and image ids.
struct TestResolver;

impl TextureResolver for TestResolver {
    fn atlas_page(&self, atlas_id: u32, page_index: u16) -> u32 {
        atlas_id * 100 + u32::from(page_index)
    }
    fn image(&self, image_id: u32) -> u32 {
        2000 + image_id
    }
}

/// The stamps callback: prepare the stamp the cache would own.
fn stamp_for<'a>(
    engine: &'a LayoutEngine<'a>,
) -> impl FnMut(u32, &GlyphInstance) -> Option<GlyphStamp> + 'a {
    let atlases: &'a [Atlas] = engine.document().atlases();
    move |_chunk_id, glyph| {
        let atlas = atlases.get(glyph.atlas_id as usize)?;
        prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
    }
}

fn glyph_commands(commands: &[DrawCommand]) -> Vec<&GlyphCommand> {
    commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Glyph(g) => Some(g),
            _ => None,
        })
        .collect()
}

fn laid_out_outlined_glyphs(engine: &LayoutEngine<'_>) -> usize {
    let mut count = 0;
    for block in engine.records_all().values() {
        for line in &block.lines {
            for instance in &line.glyphs {
                if instance.mark_of.is_none() && instance.has_outline {
                    count += 1;
                }
            }
        }
    }
    count
}

#[test]
fn is_deterministic_double_build_byte_equality() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let a = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        let b = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        assert_eq!(a, b, "identical inputs ⇒ identical draw lists");
        assert!(!a.commands.is_empty());
    });
}

#[test]
fn emits_selection_quads_first_beneath_all_content() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let selection = [
            SelectionQuad {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 16.0,
            },
            SelectionQuad {
                x: 10.0,
                y: 60.0,
                w: 40.0,
                h: 16.0,
            },
        ];
        let list = builder.build(engine, &visible_ids, stamp_for(engine), &selection);
        match &list.commands[0] {
            DrawCommand::Fill(f) => {
                assert_eq!(f.x, 10.0);
                assert_eq!(f.y, 20.0);
                assert_eq!(f.w, 100.0);
                assert_eq!(f.h, 16.0);
                assert_eq!(f.color, SELECTION_COLOR);
            }
            other => panic!("expected fill, got {other:?}"),
        }
        match &list.commands[1] {
            DrawCommand::Fill(f) => {
                assert_eq!(f.x, 10.0);
                assert_eq!(f.y, 60.0);
                assert_eq!(f.w, 40.0);
                assert_eq!(f.h, 16.0);
                assert_eq!(f.color, SELECTION_COLOR);
            }
            other => panic!("expected fill, got {other:?}"),
        }
        // No glyph precedes the selection (selection is beneath the content).
        let first_glyph = list
            .commands
            .iter()
            .position(|c| matches!(c, DrawCommand::Glyph(_)));
        if let Some(first_glyph) = first_glyph {
            assert!(first_glyph > 1);
        }
    });
}

#[test]
fn glyph_commands_carry_the_stamp_quad_uv_color_and_px_range() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        let paragraph = engine
            .records_all()
            .values()
            .map(|rc| rc.as_ref())
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("paragraph");
        let glyph = paragraph
            .lines
            .iter()
            .flat_map(|l| l.glyphs.iter())
            .find(|g| g.mark_of.is_none() && g.has_outline)
            .expect("outlined glyph");
        let atlas = &engine.document().atlases()[glyph.atlas_id as usize];
        let stamp =
            prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color).expect("stamp");
        let commands = glyph_commands(&list.commands);
        let command = commands
            .iter()
            .find(|c| c.uv == stamp.uv)
            .expect("command with matching uv");
        assert!((command.w - stamp.quad_w).abs() < 1e-4);
        assert!((command.h - stamp.quad_h).abs() < 1e-4);
        assert_eq!(command.color, glyph.color);
        // The px-range shader input is document px per texel.
        let expected = glyph.font_size_px / stamp.texels_per_em;
        assert!((command.px_per_texel - expected).abs() < 1e-6);
        // Every command's uv stays inside the page.
        for c in commands {
            let [u0, v0, u1, v1] = c.uv;
            assert!(u0 >= 0.0 && v0 >= 0.0);
            assert!(u1 <= 1.0 && v1 <= 1.0);
            assert!(u1 > u0 && v1 > v0);
        }
    });
}

#[test]
fn emits_exactly_one_command_per_laid_out_outlined_glyph_plus_the_markers() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        let laid_out = laid_out_outlined_glyphs(engine);
        // The golden has two disc list items; each emits its '•' marker glyph.
        assert_eq!(glyph_commands(&list.commands).len(), laid_out + 2);
    });
}

#[test]
fn emits_list_markers_as_glyph_commands_from_the_bullet_stamp() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        let items: Vec<&BlockLayout> = engine
            .records_all()
            .values()
            .map(|rc| rc.as_ref())
            .filter(|r| r.kind == ChunkKind::ListItem)
            .collect();
        assert_eq!(items.len(), 2);
        let glyphs = glyph_commands(&list.commands);
        for item in items {
            // The marker renders from the disc stamp (UV match).
            let atlas = &engine.document().atlases()[item.style.font_id as usize];
            let stamp = prepare_glyph(atlas, 0x2022, item.style.font_size_px, item.style.color)
                .expect("disc stamp");
            assert!(
                glyphs.iter().any(|g| g.uv == stamp.uv),
                "marker disc stamp emitted"
            );
        }
    });
}

#[test]
fn passes_ruler_geometry_through() {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 2,
            last_child_id: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 2,
            kind: 21, // hr
            parent_id: 1,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
    ];
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&chunks, &[]),
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
    ]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let builder = DrawListBuilder::new(TestResolver);
    let list = builder.build(&engine, &[1, 2], stamp_for(&engine), &[]);
    let ruler = list
        .commands
        .iter()
        .find(|c| matches!(c, DrawCommand::Ruler(_)));
    assert!(ruler.is_some(), "ruler command emitted");
    if let DrawCommand::Ruler(r) = ruler.expect("ruler") {
        assert!(r.w > 0.0);
        assert!(r.color > 0);
    }
}

#[test]
fn skips_missing_stamps_without_dropping_the_build() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let none = |_chunk_id: u32, _glyph: &GlyphInstance| -> Option<GlyphStamp> { None };
        let list = builder.build(engine, &visible_ids, none, &[]);
        // No text-run glyph commands; only the two disc markers (block
        // geometry, never gated by the cache) survive.
        assert_eq!(glyph_commands(&list.commands).len(), 2);
        // Partial coverage: only the first block with laid-out text resolves.
        let first = engine
            .records_all()
            .values()
            .map(|rc| rc.as_ref())
            .find(|r| r.lines.iter().any(|l| !l.glyphs.is_empty()))
            .map(|r| r.chunk_id)
            .expect("first text block");
        let mut full = stamp_for(engine);
        let partial = |chunk_id: u32, glyph: &GlyphInstance| -> Option<GlyphStamp> {
            if chunk_id == first {
                full(chunk_id, glyph)
            } else {
                None
            }
        };
        let partial_list = builder.build(engine, &visible_ids, partial, &[]);
        let full_list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        assert!(!glyph_commands(&partial_list.commands).is_empty());
        assert!(
            glyph_commands(&partial_list.commands).len()
                < glyph_commands(&full_list.commands).len()
        );
    });
}

// ---------------------------------------------------------------------------
// Synthetic: nested images and rules (R5 — the JS drops these)

/// A document with a quote containing an image child.
fn quote_image_package() -> Vec<u8> {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 2,
            last_child_id: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 2,
            kind: 9, // quote
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 3,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 16, // image
            parent_id: 2,
            content_index: 1,
            ordinal: 2,
            depth: 2,
            ..Default::default()
        },
    ];
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(3, 1, 1, 0, 1),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&chunks, &[]),
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&[], &[0]),
        },
        common::TestSection {
            kind: 6,
            compression: 1,
            payload: common::imgs_payload(&[common::TestImage {
                width: 40,
                height: 20,
                format: 0,
                data: vec![0u8; 40 * 20 * 4],
            }]),
        },
    ])
}

/// A document with a list item containing an `hr` child.
fn list_hr_package() -> Vec<u8> {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 2,
            last_child_id: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 2,
            kind: 10, // list (structural)
            flags: 0x10,
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 3,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 11, // list_item
            parent_id: 2,
            first_child_id: 4,
            last_child_id: 4,
            ordinal: 2,
            depth: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 4,
            kind: 21, // hr
            parent_id: 3,
            ordinal: 3,
            depth: 3,
            ..Default::default()
        },
    ];
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(4, 1, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&chunks, &[]),
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
    ])
}

fn with_synthetic<R>(bytes: Vec<u8>, f: impl FnOnce(&LayoutEngine<'_>, Vec<u32>) -> R) -> R {
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let visible_ids: Vec<u32> = engine.records_all().keys().copied().collect();
    f(&engine, visible_ids)
}

#[test]
fn nested_image_inside_a_quote_renders_r5_divergence() {
    // The JS `emitBlockLayout` drops the image branch, so a container-nested
    // image emits no quad there; the Rust builder renders it (DESIGN.md R5).
    with_synthetic(quote_image_package(), |engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        let images: Vec<&ImageCommand> = list
            .commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Image(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 1, "the nested image emits exactly one quad");
        assert_eq!(images[0].texture, 2000); // image id 0
        assert_eq!(images[0].w, 40.0);
        assert_eq!(images[0].h, 20.0);
        // The image id also appears in the visible set — no double emission.
        assert!(visible_ids.contains(&3));
    });
}

#[test]
fn nested_hr_inside_a_list_item_renders_r5_divergence() {
    with_synthetic(list_hr_package(), |engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        let rulers: Vec<&RulerCommand> = list
            .commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Ruler(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rulers.len(), 1, "the nested hr emits exactly one ruler");
        assert!(rulers[0].w > 0.0);
    });
}

#[test]
fn repeated_builds_are_stable_and_bounded() {
    with_golden(|engine, visible_ids| {
        let builder = DrawListBuilder::new(TestResolver);
        let reference = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
        for _ in 0..200 {
            let list = builder.build(engine, &visible_ids, stamp_for(engine), &[]);
            assert_eq!(list, reference, "draw list is a pure function");
        }
        // The command count is bounded by the visible content, never by the
        // document size.
        assert!(reference.commands.len() < 10_000);
    });
}

// ---------------------------------------------------------------------------
// Property: determinism over random subsets and selections

#[test]
fn property_same_inputs_yield_identical_serialization_for_random_subsets() {
    use proptest::prelude::*;

    let golden = common::pipeline_golden();
    let pkg = parse(golden).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let builder = DrawListBuilder::new(TestResolver);

    proptest::proptest!(|(subset in proptest::collection::vec(1_u32..=22, 0..=22),
                          quads in proptest::collection::vec(
                              (0_u32..2000, 0_u32..2000, 1_u32..200, 1_u32..50),
                              0..=4,
                          ))| {
        let selection: Vec<SelectionQuad> = quads
            .iter()
            .map(|&(x, y, w, h)| SelectionQuad {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
            })
            .collect();
        let a = builder.build(&engine, &subset, stamp_for(&engine), &selection);
        let b = builder.build(&engine, &subset, stamp_for(&engine), &selection);
        prop_assert_eq!(a, b);
    });
}
