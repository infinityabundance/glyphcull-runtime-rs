//! Document model tests: load-time validation and the trusted model view
//! (mirrors the JS `test/document/model.test.ts`).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::document::{
    build_document, is_block_kind, is_inline_kind, is_structural_kind, resolve_style,
    DocumentErrorKind, ResolvedStyle,
};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::style::{PropertyTag, PropertyValue, StyleProperty, StyleRecord};
use glyphcull_core::reader::{parse, Package};

fn parsed(bytes: &[u8]) -> Package {
    parse(bytes).expect("package parses")
}

/// A minimal valid document package: document root + one paragraph + one run.
fn minimal_package() -> Vec<u8> {
    let chunks = common::chnk_payload(
        &[
            common::TestChunk {
                id: 1,
                kind: 1, // document (structural)
                flags: 0x10,
                first_child_id: 2,
                last_child_id: 2,
                ..Default::default()
            },
            common::TestChunk {
                id: 2,
                kind: 8, // paragraph
                style_id: 0,
                parent_id: 1,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["Hello, world!"], &[]),
        },
    ])
}

#[test]
fn builds_a_valid_model_from_the_pipeline_golden() {
    let pkg = parsed(common::pipeline_golden());
    let doc = build_document(&pkg).expect("builds");
    assert_eq!(doc.info().chunk_count, 22);
    assert_eq!(doc.root().kind, ChunkKind::Document);
    assert_eq!(doc.chunks().len(), 22);
    assert_eq!(doc.styles().len(), 11);
    assert_eq!(doc.content().len(), 12);
    assert_eq!(doc.atlases().len(), 3);
    // The root's children are all reachable exactly once.
    let ids = doc.all_ids();
    assert_eq!(ids.len(), 22);
    let unique: std::collections::HashSet<u32> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 22);
    // Chunk records are indexed by id.
    assert_eq!(doc.chunk(1).expect("root").kind, ChunkKind::Document);
    assert!(doc.chunk(0).is_none());
    assert!(doc.chunk(23).is_none());
}

#[test]
fn plain_text_concatenates_descendant_text_in_document_order() {
    let pkg = parsed(common::pipeline_golden());
    let doc = build_document(&pkg).expect("builds");
    let text = doc.plain_text(1);
    for needle in [
        "Deterministic",
        "golden",
        "one",
        "two",
        "code block",
        "quote",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in {text:?}");
    }
    // Document order: heading, paragraph, list items, code block, quote.
    assert!(text.find("Deterministic").expect("h") < text.find("one").expect("1"));
    assert!(text.find("one").expect("1") < text.find("code block").expect("c"));
    assert!(text.find("code block").expect("c") < text.find("quote").expect("q"));
}

#[test]
fn exposes_extras_per_chunk() {
    let pkg = parsed(common::pipeline_golden());
    let doc = build_document(&pkg).expect("builds");
    let with_extras: Vec<u32> = (1..=22)
        .filter(|&id| !doc.extras_for(id).is_empty())
        .collect();
    assert_eq!(with_extras.len(), 1);
    let extras = doc.extras_for(with_extras[0]);
    assert_eq!(extras.len(), 1);
    assert_eq!(
        extras[0].data,
        glyphcull_core::reader::chunk::ExtraData::LinkTarget {
            url: "https://example.com".to_string()
        }
    );
}

#[test]
fn classifies_chunk_kinds_per_the_spec() {
    assert!(is_structural_kind(ChunkKind::Document));
    assert!(is_structural_kind(ChunkKind::List));
    assert!(is_structural_kind(ChunkKind::Table));
    assert!(is_structural_kind(ChunkKind::TableRow));
    assert!(!is_structural_kind(ChunkKind::Paragraph));
    assert!(is_inline_kind(ChunkKind::Run));
    assert!(is_inline_kind(ChunkKind::Link));
    assert!(is_inline_kind(ChunkKind::Br));
    assert!(!is_inline_kind(ChunkKind::Paragraph));
    assert!(is_block_kind(ChunkKind::Paragraph));
    assert!(is_block_kind(ChunkKind::Heading1));
    assert!(is_block_kind(ChunkKind::Image));
    assert!(is_block_kind(ChunkKind::Hr));
    assert!(!is_block_kind(ChunkKind::Run));
    assert!(!is_block_kind(ChunkKind::Document));
}

#[test]
fn rejects_a_package_without_info_or_chnk() {
    // INFO-only package: missing CHNK.
    let bytes = common::build_package(&[common::TestSection {
        kind: 1,
        compression: 1,
        payload: common::info_payload(),
    }]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("missing CHNK");
    assert_eq!(err.kind, DocumentErrorKind::MissingSection);

    // CHNK-only package: missing INFO.
    let bytes = common::build_package(&[common::TestSection {
        kind: 2,
        compression: 1,
        payload: common::empty_chnk_payload(),
    }]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("missing INFO");
    assert_eq!(err.kind, DocumentErrorKind::MissingSection);
}

#[test]
fn rejects_an_empty_chunk_graph() {
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(0, 0, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&[], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("empty graph");
    assert_eq!(err.kind, DocumentErrorKind::InvalidChunkGraph);
}

#[test]
fn rejects_a_dangling_style_reference() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 8,
                parent_id: 1,
                style_id: 7,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["x"], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("dangling style");
    assert_eq!(err.kind, DocumentErrorKind::DanglingReference);
}

#[test]
fn rejects_a_dangling_content_reference() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 8,
                parent_id: 1,
                content_index: 9,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 0, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["x"], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("dangling content");
    assert_eq!(err.kind, DocumentErrorKind::DanglingReference);
}

#[test]
fn rejects_an_image_chunk_referencing_a_text_payload() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 16, // image
                parent_id: 1,
                style_id: 0,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["not an image"], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("text instead of image ref");
    assert_eq!(err.kind, DocumentErrorKind::InvalidContent);
}

#[test]
fn rejects_a_non_image_chunk_referencing_an_image_ref_payload() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 8, // paragraph
                parent_id: 1,
                style_id: 0,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
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
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("image ref on a paragraph");
    assert_eq!(err.kind, DocumentErrorKind::InvalidContent);
}

#[test]
fn rejects_an_image_reference_out_of_range() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 16, // image
                parent_id: 1,
                style_id: 0,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 1, 0, 1),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&[], &[5]),
        },
        common::TestSection {
            kind: 6,
            compression: 0,
            payload: common::imgs_payload(&[common::TestImage {
                width: 1,
                height: 1,
                format: 0,
                data: vec![0; 4],
            }]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("image id out of range");
    assert_eq!(err.kind, DocumentErrorKind::DanglingReference);
}

#[test]
fn rejects_info_chnk_count_mismatches() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 8,
                parent_id: 1,
                style_id: 0,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(99, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["x"], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("count mismatch");
    assert_eq!(err.kind, DocumentErrorKind::CountMismatch);
}

#[test]
fn rejects_a_graph_with_a_cycle() {
    // Root's first_child is itself: an immediate cycle.
    let chunks = common::chnk_payload(
        &[common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 1,
            last_child_id: 1,
            ..Default::default()
        }],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(1, 0, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("cycle");
    assert_eq!(err.kind, DocumentErrorKind::InvalidChunkGraph);
}

#[test]
fn rejects_a_graph_with_an_unreachable_chunk() {
    // Two siblings at the root with the same parent but the second is not
    // linked in the first's ring — it becomes unreachable.
    let chunks = common::chnk_payload(
        &[
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
                kind: 8,
                parent_id: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
            common::TestChunk {
                id: 3,
                kind: 8,
                parent_id: 1,
                ordinal: 2,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(3, 0, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("unreachable chunk");
    assert_eq!(err.kind, DocumentErrorKind::InvalidChunkGraph);
}

#[test]
fn rejects_a_sibling_ring_that_does_not_reach_last_child() {
    // Root declares first_child 2 and last_child 3, but chunk 2's next_id is
    // 0 — the ring terminates early.
    let chunks = common::chnk_payload(
        &[
            common::TestChunk {
                id: 1,
                kind: 1,
                flags: 0x10,
                first_child_id: 2,
                last_child_id: 3,
                ..Default::default()
            },
            common::TestChunk {
                id: 2,
                kind: 8,
                parent_id: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
            common::TestChunk {
                id: 3,
                kind: 8,
                parent_id: 1,
                ordinal: 2,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(3, 0, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("ring does not terminate");
    assert_eq!(err.kind, DocumentErrorKind::InvalidChunkGraph);
}

#[test]
fn rejects_a_font_id_beyond_the_atlas_table() {
    let chunks = common::chnk_payload(
        &[
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
                kind: 8,
                parent_id: 1,
                style_id: 1,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
        ],
        &[],
    );
    // Style 1 sets font_id=5 but there are no atlases.
    let styl =
        common::styl_payload(&[(0, vec![]), (1, common::style_prop(1, &5u32.to_le_bytes()))]);
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 2, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: styl,
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["x"], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("font_id out of range");
    assert_eq!(err.kind, DocumentErrorKind::DanglingReference);
}

#[test]
fn resolves_styles_with_spec_defaults() {
    // An empty record resolves to the SPEC §2.3 defaults.
    let defaults = resolve_style(&StyleRecord {
        id: 0,
        properties: Vec::new(),
    });
    assert_eq!(defaults.font_id, 0);
    assert_eq!(defaults.font_size_px, 16.0);
    assert_eq!(defaults.line_height, 1.5);
    assert_eq!(defaults.font_weight, 400);
    assert!(!defaults.italic);
    assert_eq!(defaults.color, 0x0000_00ff);
    assert_eq!(defaults.background_color, 0x0000_0000);
    assert_eq!(defaults.margin_top, 0.0);
    assert_eq!(defaults.margin_bottom, 0.0);
    assert_eq!(defaults.text_align, 0);
    assert_eq!(defaults.text_indent, 0.0);
    assert_eq!(defaults.list_style, 0);
    assert!(!defaults.code);
    assert!(!defaults.underline);
    assert_eq!(defaults.letter_spacing, 0.0);
    assert_eq!(defaults.white_space, 0);

    // Every tag overrides its default.
    let record = StyleRecord {
        id: 0,
        properties: vec![
            StyleProperty {
                tag: PropertyTag::FontId,
                value: PropertyValue::U32(2),
            },
            StyleProperty {
                tag: PropertyTag::FontSizePx,
                value: PropertyValue::F32(24.0),
            },
            StyleProperty {
                tag: PropertyTag::LineHeight,
                value: PropertyValue::F32(1.2),
            },
            StyleProperty {
                tag: PropertyTag::FontWeight,
                value: PropertyValue::U16(700),
            },
            StyleProperty {
                tag: PropertyTag::Italic,
                value: PropertyValue::U8(1),
            },
            StyleProperty {
                tag: PropertyTag::Color,
                value: PropertyValue::U32(0x1122_3344),
            },
            StyleProperty {
                tag: PropertyTag::BackgroundColor,
                value: PropertyValue::U32(0xff00_0000),
            },
            StyleProperty {
                tag: PropertyTag::MarginTop,
                value: PropertyValue::F32(4.0),
            },
            StyleProperty {
                tag: PropertyTag::MarginBottom,
                value: PropertyValue::F32(8.0),
            },
            StyleProperty {
                tag: PropertyTag::TextAlign,
                value: PropertyValue::U8(3),
            },
            StyleProperty {
                tag: PropertyTag::TextIndent,
                value: PropertyValue::F32(24.0),
            },
            StyleProperty {
                tag: PropertyTag::ListStyle,
                value: PropertyValue::U8(4),
            },
            StyleProperty {
                tag: PropertyTag::Code,
                value: PropertyValue::U8(1),
            },
            StyleProperty {
                tag: PropertyTag::Underline,
                value: PropertyValue::U8(1),
            },
            StyleProperty {
                tag: PropertyTag::LetterSpacing,
                value: PropertyValue::F32(1.0),
            },
            StyleProperty {
                tag: PropertyTag::WhiteSpace,
                value: PropertyValue::U8(1),
            },
        ],
    };
    let resolved = resolve_style(&record);
    assert_eq!(resolved.font_id, 2);
    assert_eq!(resolved.font_size_px, 24.0);
    assert_eq!(resolved.line_height, 1.2);
    assert_eq!(resolved.font_weight, 700);
    assert!(resolved.italic);
    assert_eq!(resolved.color, 0x1122_3344);
    assert_eq!(resolved.background_color, 0xff00_0000);
    assert_eq!(resolved.margin_top, 4.0);
    assert_eq!(resolved.margin_bottom, 8.0);
    assert_eq!(resolved.text_align, 3);
    assert_eq!(resolved.text_indent, 24.0);
    assert_eq!(resolved.list_style, 4);
    assert!(resolved.code);
    assert!(resolved.underline);
    assert_eq!(resolved.letter_spacing, 1.0);
    assert_eq!(resolved.white_space, 1);
}

#[test]
fn golden_styles_resolve_with_defaults_applied() {
    let pkg = parsed(common::pipeline_golden());
    let doc = build_document(&pkg).expect("builds");
    // The golden stylesheet sets `p { color: #336699 }`.
    let has_color = doc
        .styles()
        .iter()
        .any(|s: &ResolvedStyle| s.color == 0x3366_99ff);
    assert!(has_color, "expected the golden paragraph color");
    // Defaults for unspecified properties on the document default style.
    let default_style = &doc.styles()[0];
    assert_eq!(default_style.font_size_px, 16.0);
    assert_eq!(default_style.line_height, 1.5);
    assert_eq!(default_style.font_weight, 400);
    assert_eq!(default_style.color, 0x0000_00ff);
}

#[test]
fn direct_text_and_image_ref_resolution() {
    // A paragraph with text and an image chunk with an image_ref.
    let chunks = common::chnk_payload(
        &[
            common::TestChunk {
                id: 1,
                kind: 1,
                flags: 0x10,
                first_child_id: 2,
                last_child_id: 3,
                ..Default::default()
            },
            common::TestChunk {
                id: 2,
                kind: 8, // paragraph
                parent_id: 1,
                style_id: 0,
                content_index: 1,
                ordinal: 1,
                depth: 1,
                next_id: 3,
                ..Default::default()
            },
            common::TestChunk {
                id: 3,
                kind: 16, // image
                parent_id: 1,
                style_id: 0,
                content_index: 2,
                ordinal: 2,
                depth: 1,
                next_id: 0,
                prev_id: 2,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(3, 1, 2, 0, 1),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["caption text"], &[0]),
        },
        common::TestSection {
            kind: 6,
            compression: 0,
            payload: common::imgs_payload(&[common::TestImage {
                width: 2,
                height: 1,
                format: 0,
                data: vec![0; 8],
            }]),
        },
    ]);
    let pkg = parsed(&bytes);
    let doc = build_document(&pkg).expect("builds");
    assert_eq!(doc.direct_text(2), Some("caption text"));
    assert_eq!(doc.image_ref(3), Some(0));
    assert_eq!(doc.direct_text(3), None);
    assert_eq!(doc.image_ref(2), None);
    assert_eq!(doc.direct_text(99), None);
    assert_eq!(doc.images().len(), 1);
    assert_eq!(doc.images()[0].width, 2);
}

#[test]
fn plain_text_handles_br_and_code_blocks() {
    // root → paragraph → [run "one", br, run "two"]; root → code_block "code".
    let chunks = common::chnk_payload(
        &[
            common::TestChunk {
                id: 1,
                kind: 1,
                flags: 0x10,
                first_child_id: 2,
                last_child_id: 4,
                ..Default::default()
            },
            common::TestChunk {
                id: 2,
                kind: 8, // paragraph
                parent_id: 1,
                style_id: 0,
                ordinal: 1,
                depth: 1,
                first_child_id: 5,
                last_child_id: 7,
                next_id: 3,
                ..Default::default()
            },
            common::TestChunk {
                id: 3,
                kind: 12, // code_block (direct text, no children)
                parent_id: 1,
                style_id: 0,
                content_index: 3,
                ordinal: 2,
                depth: 1,
                prev_id: 2,
                next_id: 4,
                ..Default::default()
            },
            common::TestChunk {
                id: 4,
                kind: 21, // hr (renders nothing)
                parent_id: 1,
                style_id: 0,
                ordinal: 3,
                depth: 1,
                prev_id: 3,
                ..Default::default()
            },
            common::TestChunk {
                id: 5,
                kind: 18, // run "one"
                parent_id: 2,
                style_id: 0,
                content_index: 1,
                ordinal: 4,
                depth: 2,
                next_id: 6,
                ..Default::default()
            },
            common::TestChunk {
                id: 6,
                kind: 20, // br
                parent_id: 2,
                style_id: 0,
                ordinal: 5,
                depth: 2,
                prev_id: 5,
                next_id: 7,
                ..Default::default()
            },
            common::TestChunk {
                id: 7,
                kind: 18, // run "two"
                parent_id: 2,
                style_id: 0,
                content_index: 2,
                ordinal: 6,
                depth: 2,
                prev_id: 6,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(7, 1, 3, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&["one", "two", "code"], &[]),
        },
    ]);
    let pkg = parsed(&bytes);
    let doc = build_document(&pkg).expect("builds");
    // Paragraph: "one" + br newline + "two"; code block contributes "code";
    // hr contributes nothing.
    assert_eq!(doc.plain_text(2), "one\ntwo");
    assert_eq!(doc.plain_text(3), "code");
    assert_eq!(doc.plain_text(4), "");
    assert_eq!(doc.plain_text(1), "one\ntwocode");
}

#[test]
fn documents_are_isolated() {
    let bytes = minimal_package();
    let pkg_a = parsed(&bytes);
    let doc_a = build_document(&pkg_a).expect("builds a");
    let pkg_b = parsed(&bytes);
    let doc_b = build_document(&pkg_b).expect("builds b");
    // No shared mutable state: independent builds produce equal, independent
    // models.
    assert_eq!(doc_a.plain_text(1), doc_b.plain_text(1));
    assert_eq!(doc_a.chunks(), doc_b.chunks());
    assert_ne!(doc_a.styles().as_ptr(), doc_b.styles().as_ptr());
}

#[test]
fn invalid_chunk_graph_is_a_typed_document_error_not_a_reader_error() {
    // A reader-level decode failure inside a document build surfaces as the
    // JS runtime's wrapped 'invalid-chunk-graph' (build failed), not a panic.
    let mut chunks = common::chnk_payload(
        &[common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            ..Default::default()
        }],
        &[],
    );
    chunks.truncate(3); // corrupt the payload mid-record
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(1, 0, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
    ]);
    let pkg = parsed(&bytes);
    let err = build_document(&pkg).expect_err("wrapped decode failure");
    assert_eq!(err.kind, DocumentErrorKind::InvalidChunkGraph);
}
