//! Property tests for selection (proptest) — mirrors the JS fast-check
//! properties in `test/selection/selection.test.ts`:
//!
//! - `comparePositions` is antisymmetric over random positions.
//! - `normalizeSelection` always yields a start ≤ end selection.
//! - `copyText` is deterministic for random selections.
//! - `copyText` of a reversed selection equals the normalized copy.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use proptest::prelude::*;

use glyphcull_core::document::build_document;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::{compare_positions, copy_text, normalize_selection, TextPosition};

/// A document-order position over the golden's 22 chunks.
fn gen_position() -> impl Strategy<Value = TextPosition> {
    (1_u32..=22, 0usize..20).prop_map(|(chunk_id, offset)| TextPosition { chunk_id, offset })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Comparison is antisymmetric: `compare(a, b) == -compare(b, a)`.
    #[test]
    fn comparison_is_antisymmetric(a in gen_position(), b in gen_position()) {
        let pkg = parse(common::pipeline_golden()).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let forward = compare_positions(&doc, a, b);
        let backward = compare_positions(&doc, b, a);
        prop_assert_eq!(forward, backward.reverse(), "antisymmetry");
    }

    /// Normalization always orders the anchors.
    #[test]
    fn normalization_orders_the_anchors(a in gen_position(), b in gen_position()) {
        let pkg = parse(common::pipeline_golden()).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let selection = normalize_selection(&doc, a, b);
        prop_assert_ne!(
            compare_positions(&doc, selection.start, selection.end),
            std::cmp::Ordering::Greater,
            "start <= end"
        );
    }

    /// Copying is deterministic for random selections.
    #[test]
    fn copying_is_deterministic(a in gen_position(), b in gen_position()) {
        let pkg = parse(common::pipeline_golden()).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let selection = normalize_selection(&doc, a, b);
        let first = copy_text(&doc, selection);
        let second = copy_text(&doc, selection);
        prop_assert_eq!(first, second);
    }

    /// The covered chunk ids are exactly the `all_ids` slice between the
    /// selection's endpoints, in document order.
    #[test]
    fn covered_ids_are_the_document_order_slice_between_the_endpoints(
        a in gen_position(),
        b in gen_position(),
    ) {
        let pkg = parse(common::pipeline_golden()).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let selection = normalize_selection(&doc, a, b);
        let ids = glyphcull_core::selection::covered_chunk_ids(&doc, selection);
        let all = doc.all_ids();
        if glyphcull_core::selection::is_collapsed(selection) {
            prop_assert!(ids.is_empty(), "a collapsed selection covers nothing");
        } else {
            prop_assert!(
                !ids.is_empty(),
                "a non-collapsed selection covers at least its endpoints"
            );
            let start = all
                .iter()
                .position(|&x| x == selection.start.chunk_id)
                .expect("start present");
            let end = all
                .iter()
                .position(|&x| x == selection.end.chunk_id)
                .expect("end present");
            let expected: Vec<u32> = all[start..=end].to_vec();
            prop_assert_eq!(ids, expected, "contiguous document-order coverage");
        }
    }
}
