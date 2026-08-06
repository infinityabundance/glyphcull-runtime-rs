//! Stress tests for the reader: the maximum-size container parses within
//! bounds, repeated parsing is stable, and large decoded sections stream
//! through the bounded inflate path (mirrors the compiler's stress
//! discipline; the JS suite covers the same via its test pyramid).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::limits::{MAX_SECTION_COUNT, MAX_TOTAL_DECODED};
use glyphcull_core::reader::{parse, SectionKind};

#[test]
fn a_sixty_four_section_container_parses() {
    // The v1 cap is 64 sections (SPEC.md §1.3); every section here is a
    // distinct kind-addressable payload so the duplicate check never fires.
    let mut sections = Vec::new();
    for i in 0..MAX_SECTION_COUNT {
        // Kinds 1..7 are known; reserve kinds (e.g. 100 + i) are addressable.
        let kind: u32 = 100 + i as u32;
        let payload = format!("section {i}: {}", "x".repeat(128)).into_bytes();
        sections.push(common::TestSection {
            kind,
            compression: 1,
            payload,
        });
    }
    let bytes = common::build_package(&sections);
    let pkg = parse(&bytes).expect("max-count package parses");
    assert_eq!(pkg.unknown.len(), MAX_SECTION_COUNT as usize);
    assert_eq!(pkg.entries.len(), MAX_SECTION_COUNT as usize);
    assert!(pkg.sections.is_empty());
}

#[test]
fn a_total_decoded_cap_claim_is_rejected_by_the_bounded_path() {
    // Two zlib sections each claiming near-2 GiB decoded_len with tiny
    // streams cannot be honored: the bounded inflate path rejects the
    // impossible length (decoded != claimed) without ever allocating the
    // claimed size — the same outcome the JS runtime produces. The total
    // decoded cap is defense-in-depth against streams that genuinely decode
    // near the per-section cap (SPEC.md §1.3).
    let tiny = b"tiny payload".to_vec();
    let sections = [
        common::TestSection {
            kind: 101,
            compression: 1,
            payload: tiny.clone(),
        },
        common::TestSection {
            kind: 102,
            compression: 1,
            payload: tiny.clone(),
        },
    ];
    // Patch the table's decoded_len fields to just under the per-section cap.
    let mut bytes = common::build_package(&sections);
    let claim = (MAX_TOTAL_DECODED / 2) as u32;
    for i in 0..2 {
        let decoded_len_offset = common::HEADER_LEN + i * common::ENTRY_LEN + 24;
        let slice = bytes
            .get_mut(decoded_len_offset..decoded_len_offset + 4)
            .expect("field");
        slice.copy_from_slice(&claim.to_le_bytes());
    }
    let err = parse(&bytes).expect_err("claimed length rejected");
    assert_eq!(
        err.kind(),
        glyphcull_core::error::ErrorKind::DecompressMismatch
    );
}

#[test]
fn large_decoded_sections_stream_through_the_bounded_path() {
    // A ~1 MiB decoded section (incompressible data) parses exactly.
    let big: Vec<u8> = (0..(1 << 20)).map(|i| ((i * 7) % 251) as u8).collect();
    let sections = [
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload(),
        },
        common::TestSection {
            kind: 101,
            compression: 1,
            payload: big.clone(),
        },
    ];
    let bytes = common::build_package(&sections);
    let pkg = parse(&bytes).expect("large package parses");
    let payload = &pkg.unknown[0].bytes;
    assert_eq!(payload.len(), big.len());
    assert_eq!(payload, &big);
    assert!(pkg.info().expect("info decodes").is_some());
}

#[test]
fn golden_parses_repeatedly_with_stable_results() {
    // 200 parse iterations over the 855 KiB golden: allocation and timing
    // stability regression; results must be byte-identical every time.
    let golden = common::pipeline_golden();
    let reference = parse(golden).expect("reference parse");
    let reference_chunks = reference
        .chunks()
        .expect("chunks")
        .expect("present")
        .clone();
    for _ in 0..200 {
        let pkg = parse(golden).expect("parse");
        assert_eq!(pkg.entries, reference.entries);
        let chunks = pkg.chunks().expect("chunks").expect("present");
        assert_eq!(chunks.chunks, reference_chunks.chunks);
        assert_eq!(chunks.extras, reference_chunks.extras);
    }
}

#[test]
fn all_known_section_kinds_round_trip_together() {
    // A package carrying every known kind (INFO, CHNK, STYL, CONT, GLYF,
    // IMGS) plus an unknown kind — the maximum structural diversity the
    // reader must handle in one container. Empty-but-valid STYL/CONT/GLYF/
    // IMGS payloads carry a zero count; decoded_len is ≥ 1, so the container
    // is well-formed.
    let empty = vec![0_u8; 4]; // count = 0
    let sections = [
        common::TestSection {
            kind: SectionKind::Info as u32,
            compression: 1,
            payload: common::info_payload(),
        },
        common::TestSection {
            kind: SectionKind::Chunk as u32,
            compression: 1,
            payload: common::empty_chnk_payload(),
        },
        common::TestSection {
            kind: SectionKind::Style as u32,
            compression: 1,
            payload: empty.clone(),
        },
        common::TestSection {
            kind: SectionKind::Content as u32,
            compression: 1,
            payload: empty.clone(),
        },
        common::TestSection {
            kind: SectionKind::Glyph as u32,
            compression: 0,
            payload: empty.clone(),
        },
        common::TestSection {
            kind: SectionKind::Images as u32,
            compression: 0,
            payload: empty.clone(),
        },
        common::TestSection {
            kind: 200,
            compression: 0,
            payload: b"future".to_vec(),
        },
    ];
    let bytes = common::build_package(&sections);
    let pkg = parse(&bytes).expect("diverse package parses");
    assert_eq!(pkg.sections.len(), 6);
    assert_eq!(pkg.unknown.len(), 1);
    assert_eq!(pkg.unknown[0].entry.kind, 200);
}
