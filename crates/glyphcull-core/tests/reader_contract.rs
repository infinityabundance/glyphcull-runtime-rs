//! Reader contract tests: the independent `.cull` reader against the
//! committed compiler fixtures, with the compiler's `cull inspect`
//! diagnostics pinned as the expected values (mirrors the JS
//! `test/format/reader.test.ts`).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use std::collections::{BTreeSet, HashMap, HashSet};

use glyphcull_core::error::ErrorKind;
use glyphcull_core::reader::chunk::{ChunkKind, ExtraData, ExtraKind};
use glyphcull_core::reader::content::{ContentData, PayloadKind};
use glyphcull_core::reader::style::{PropertyTag, PropertyValue};
use glyphcull_core::reader::{
    parse, validate_structure, Compression, SectionKind, HEADER_LEN, VERSION,
};

// ---------------------------------------------------------------------------
// Container structure (JS `describe('container structure')`)

#[test]
fn parses_the_v1_minimal_package() {
    let structure = validate_structure(common::v1_minimal()).expect("valid");
    assert_eq!(structure.version, VERSION);
    assert_eq!(structure.entries.len(), 1);
    let entry = &structure.entries[0];
    assert_eq!(entry.kind, SectionKind::Info as u32);
    assert_eq!(entry.compression, Compression::Zlib);
}

#[test]
fn rejects_a_too_short_buffer() {
    for len in [0usize, 1, 4, 15] {
        let err = validate_structure(&vec![0_u8; len]).expect_err("too short");
        assert_eq!(err.kind(), ErrorKind::TooShort, "len {len}");
    }
}

#[test]
fn rejects_bad_magic() {
    let mut bytes = common::v1_minimal().to_vec();
    bytes[0] = b'X';
    let err = validate_structure(&bytes).expect_err("bad magic");
    assert_eq!(err.kind(), ErrorKind::BadMagic);
}

#[test]
fn rejects_unsupported_versions() {
    let mut bytes = common::v1_minimal().to_vec();
    bytes[4] = 2;
    bytes[5] = 0;
    let err = validate_structure(&bytes).expect_err("bad version");
    assert_eq!(err.kind(), ErrorKind::UnsupportedVersion);
}

#[test]
fn rejects_header_crc_mismatch() {
    // Corrupt section_count after the header CRC was computed.
    let mut bytes = common::v1_minimal().to_vec();
    bytes[8] = 99;
    let err = validate_structure(&bytes).expect_err("bad header crc");
    assert_eq!(err.kind(), ErrorKind::HeaderCrcMismatch);
}

#[test]
fn rejects_a_truncated_section_table() {
    let bytes = common::v1_minimal().get(..HEADER_LEN + 8).expect("prefix");
    let err = validate_structure(bytes).expect_err("truncated table");
    assert_eq!(err.kind(), ErrorKind::Truncated);
}

#[test]
fn rejects_zero_and_over_limit_section_counts() {
    let with_count = |count: u32| -> Vec<u8> {
        let mut bytes = common::v1_minimal().to_vec();
        let count_slice = bytes.get_mut(8..12).expect("count bytes");
        count_slice.copy_from_slice(&count.to_le_bytes());
        // Recompute the header CRC over bytes 0..12.
        let crc = glyphcull_core::reader::crc32(bytes.get(..12).expect("prefix"));
        let crc_slice = bytes.get_mut(12..16).expect("crc bytes");
        crc_slice.copy_from_slice(&crc.to_le_bytes());
        bytes
    };
    for count in [0_u32, 65, u32::MAX] {
        let err = validate_structure(&with_count(count)).expect_err("section count");
        let expected = if count == 0 {
            ErrorKind::InvalidValue
        } else {
            ErrorKind::TooManySections
        };
        assert_eq!(err.kind(), expected, "count {count}");
    }
}

// ---------------------------------------------------------------------------
// Full package read (JS `describe('full package read')`)

#[test]
fn parses_the_pipeline_golden_with_pinned_diagnostics() {
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    assert_eq!(pkg.version, 1);

    let kinds: Vec<u32> = pkg.entries.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, common::GOLDEN.section_kinds);

    let info = pkg.info().expect("info decodes").expect("info present");
    assert_eq!(info.document_id, common::GOLDEN.document_id);
    assert_eq!(info.source_digest, common::GOLDEN.source_digest);
    assert_eq!(info.generator, common::GOLDEN.generator);
    assert_eq!(info.chunk_count, common::GOLDEN.chunk_count);
    assert_eq!(info.style_count, common::GOLDEN.style_count);
    assert_eq!(info.content_count, common::GOLDEN.content_count);
    assert_eq!(info.atlas_count, common::GOLDEN.atlas_count);
    assert_eq!(info.image_count, common::GOLDEN.image_count);
}

#[test]
fn decodes_the_chunk_graph_with_exact_counts_and_a_link_target_extra() {
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    let chunks = pkg
        .chunks()
        .expect("chunks decode")
        .expect("chunks present");
    assert_eq!(chunks.chunks.len(), 22);
    assert_eq!(chunks.extras.len(), 1);
    let extra = &chunks.extras[0];
    assert_eq!(extra.kind, ExtraKind::LinkTarget);
    assert_eq!(
        extra.data,
        ExtraData::LinkTarget {
            url: "https://example.com".to_string()
        }
    );

    // Chunk kind census, pinned from `cull inspect`.
    let count = |kind: ChunkKind| chunks.chunks.iter().filter(|c| c.kind == kind).count();
    assert_eq!(count(ChunkKind::Document), 1);
    assert_eq!(count(ChunkKind::Heading1), 1);
    assert_eq!(count(ChunkKind::Paragraph), 4);
    assert_eq!(count(ChunkKind::List), 1);
    assert_eq!(count(ChunkKind::ListItem), 2);
    assert_eq!(count(ChunkKind::CodeBlock), 1);
    assert_eq!(count(ChunkKind::Quote), 1);
    assert_eq!(count(ChunkKind::Run), 11);

    // Tree invariants hold on the parsed records.
    let by_id: HashMap<u32, &glyphcull_core::reader::chunk::ChunkRecord> =
        chunks.chunks.iter().map(|c| (c.id, c)).collect();
    let root = by_id.get(&1).expect("root");
    assert_eq!(root.kind, ChunkKind::Document);
    assert_eq!(root.depth, 0);
    assert_eq!(root.parent_id, 0);
    let mut reachable = 0_usize;
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack = vec![root.id];
    while let Some(id) = stack.pop() {
        assert!(seen.insert(id), "cycle in chunk tree at chunk {id}");
        reachable += 1;
        let chunk = by_id.get(&id).expect("chunk exists");
        // Walk the sibling ring once via next links, stopping at last_child.
        let mut child = chunk.first_child_id;
        while child != 0 {
            stack.push(child);
            if child == chunk.last_child_id {
                break;
            }
            child = by_id.get(&child).expect("child exists").next_id;
        }
    }
    assert_eq!(reachable, chunks.chunks.len());
    for chunk in &chunks.chunks {
        assert_eq!(chunk.ordinal, chunk.id - 1);
        if chunk.id != 1 {
            let parent = by_id.get(&chunk.parent_id).expect("parent exists");
            assert_eq!(chunk.depth, parent.depth + 1);
        }
    }
}

#[test]
fn decodes_styles() {
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    let styles = pkg
        .styles()
        .expect("styles decode")
        .expect("styles present");
    assert_eq!(styles.len(), 11);
    // The golden stylesheet sets `p { color: #336699 }`; the paragraph style
    // carries it as an explicit property.
    let has_color = styles.iter().any(|s| {
        s.properties
            .iter()
            .any(|p| p.tag == PropertyTag::Color && p.value == PropertyValue::U32(0x3366_99ff))
    });
    assert!(has_color, "expected the golden paragraph color");
}

#[test]
fn decodes_content_payloads() {
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    let content = pkg
        .content()
        .expect("content decodes")
        .expect("content present");
    assert_eq!(content.len(), 12);
    assert!(content.iter().all(|p| p.kind == PayloadKind::TextUtf8));
    let texts: Vec<&str> = content
        .iter()
        .map(|p| match &p.data {
            ContentData::Text(text) => text.as_str(),
            ContentData::ImageRef(_) => "",
        })
        .collect();
    assert!(texts.contains(&"one"));
    assert!(texts.contains(&"two"));
    assert!(texts.join("").contains("code block"));
}

#[test]
fn decodes_the_three_atlases_with_pinned_descriptors() {
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    let atlases = pkg
        .atlases()
        .expect("atlases decode")
        .expect("atlases present");
    assert_eq!(atlases.len(), 3);
    for expected in common::GOLDEN.atlases {
        let atlas = atlases
            .get(expected.font_id as usize)
            .expect("atlas present");
        assert_eq!(atlas.weight, expected.weight);
        assert_eq!(atlas.italic, expected.italic);
        assert_eq!(atlas.glyphs.len(), expected.glyphs);
        assert!(atlas.kerning.len() <= expected.kerning);
        assert_eq!(atlas.page_width, expected.page_width);
        assert_eq!(atlas.texels_per_em(), 32.0);
        assert_eq!(atlas.pages.len(), expected.pages);
        let page_bytes = (atlas.page_width as usize) * (atlas.page_height as usize) * 4;
        assert_eq!(atlas.pages[0].len(), page_bytes);
    }
}

#[test]
fn verifies_the_seal_against_every_covered_section() {
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    let seal = pkg.seal().expect("seal decodes").expect("seal present");
    assert_eq!(seal.hashes.len(), 5);
    assert_eq!(seal.mode, 1);
    assert_eq!(seal.algo, 0);
    // `parse` already verified the seal; prove the public path independently.
    pkg.verify_seal().expect("seal verifies");
}

#[test]
fn tampering_with_any_section_is_a_typed_error() {
    let bytes = common::pipeline_golden().to_vec();
    let pkg = parse(&bytes).expect("golden parses");
    for entry in &pkg.entries {
        let mut mutated = bytes.clone();
        let at = entry.offset as usize + (entry.stored_len as usize) / 2;
        mutated[at] ^= 0x55;
        let err = parse(&mutated).expect_err("mutation must fail");
        assert!(
            matches!(
                err.kind(),
                ErrorKind::CrcMismatch
                    | ErrorKind::DecompressMismatch
                    | ErrorKind::ZlibAdlerMismatch
                    | ErrorKind::Truncated
            ),
            "section {}: unexpected kind {:?}",
            entry.index,
            err.kind()
        );
    }
}

#[test]
fn tampering_with_the_seal_overall_hash_fails_verification() {
    let bytes = common::pipeline_golden().to_vec();
    let pkg = parse(&bytes).expect("golden parses");
    let seal_entry = pkg
        .entries
        .iter()
        .find(|e| e.kind == SectionKind::Seal as u32)
        .expect("seal entry");

    let mut mutated = bytes.clone();
    let overall_start = seal_entry.offset as usize + seal_entry.stored_len as usize - 32;
    mutated[overall_start] ^= 0xff;

    // The per-section CRC would catch the tamper first; recompute it so the
    // SEAL verification path itself is exercised.
    let seal_payload = mutated
        .get(
            seal_entry.offset as usize..seal_entry.offset as usize + seal_entry.stored_len as usize,
        )
        .expect("seal payload");
    let crc_offset = HEADER_LEN + seal_entry.index * common::ENTRY_LEN + 28;
    let crc = glyphcull_core::reader::crc32(seal_payload);
    let crc_slice = mutated
        .get_mut(crc_offset..crc_offset + 4)
        .expect("crc bytes");
    crc_slice.copy_from_slice(&crc.to_le_bytes());

    let err = parse(&mutated).expect_err("tampered seal");
    assert_eq!(err.kind(), ErrorKind::SealMismatch);
}

// ---------------------------------------------------------------------------
// Truncation corpus (JS `describe('truncation corpus')`)

#[test]
fn every_proper_prefix_of_v1_minimal_fails_with_a_typed_error() {
    let bytes = common::v1_minimal();
    for len in 0..bytes.len() {
        let prefix = bytes.get(..len).expect("prefix");
        let err = parse(prefix).expect_err("prefix must fail");
        // Precise, never an internal defect: prefixes are untrusted input.
        assert_ne!(err.kind(), ErrorKind::Internal, "prefix length {len}");
    }
}

#[test]
fn structural_truncations_of_the_pipeline_golden_fail() {
    let bytes = common::pipeline_golden();
    let pkg = parse(bytes).expect("golden parses");
    // Truncate at every structural boundary: header, each entry, each section
    // start/end ±1, and section midpoints.
    let mut points: BTreeSet<usize> = BTreeSet::new();
    points.insert(HEADER_LEN - 1);
    for entry in &pkg.entries {
        let offset = entry.offset as usize;
        let stored = entry.stored_len as usize;
        points.insert(offset - 1);
        points.insert(offset);
        points.insert(offset + 1);
        points.insert(offset + stored / 2);
        points.insert(offset + stored - 1);
        points.insert(offset + stored);
        points.insert(offset + stored + 1);
    }
    for point in points {
        if point >= bytes.len() - 1 {
            continue;
        }
        let prefix = bytes.get(..point).expect("prefix");
        let err = parse(prefix).expect_err("truncation must fail");
        assert_ne!(err.kind(), ErrorKind::Internal, "truncation point {point}");
    }
}

// ---------------------------------------------------------------------------
// Unknown sections and structural strictness (JS describe of the same name)

#[test]
fn skips_reserved_section_kinds_for_forward_compatibility() {
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload(),
        },
        common::TestSection {
            kind: 99,
            compression: 0,
            payload: b"future data".to_vec(),
        },
    ]);
    let pkg = parse(&bytes).expect("parses");
    assert_eq!(pkg.unknown.len(), 1);
    assert_eq!(pkg.unknown[0].entry.kind, 99);
    assert_eq!(pkg.unknown[0].bytes, b"future data");
    assert!(pkg.info().expect("info decodes").is_some());
}

#[test]
fn rejects_duplicate_section_kinds() {
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload(),
        },
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload(),
        },
    ]);
    let err = parse(&bytes).expect_err("duplicate kind");
    assert_eq!(err.kind(), ErrorKind::DuplicateSection);
}

#[test]
fn rejects_reserved_flags_in_section_entries() {
    let bytes = common::build_package(&[common::TestSection {
        kind: 1,
        compression: 1,
        payload: common::info_payload(),
    }]);
    let mut mutated = bytes.clone();
    mutated[HEADER_LEN + 5] = 1; // entry flags byte
    let err = parse(&mutated).expect_err("reserved flags");
    assert_eq!(err.kind(), ErrorKind::InvalidFlags);
}

#[test]
fn rejects_a_package_whose_info_has_unknown_or_wrong_typed_keys() {
    // INFO decoding is lazy; run it through the typed-result boundary.
    let info_of = |payload: Vec<u8>| -> std::result::Result<
        Option<glyphcull_core::reader::info::Info>,
        glyphcull_core::error::Error,
    > {
        let bytes = common::build_package(&[common::TestSection {
            kind: 1,
            compression: 0,
            payload,
        }]);
        let pkg = parse(&bytes).expect("container parses");
        pkg.info().map(|opt| opt.cloned())
    };

    let required = |extra: &str| -> String {
        format!(
            concat!(
                "{{\"format_version\":1,\"generator\":\"g\",\"generator_version\":\"v\",",
                "\"source_digest\":\"{}\",\"document_id\":\"{}\",{}}}"
            ),
            "00".repeat(32),
            "01".repeat(16),
            extra
        )
    };

    // Wrong type for a required key.
    let wrong_type = required("\"chunk_count\":\"not-a-number\",\"style_count\":0,\"content_count\":0,\"atlas_count\":0,\"image_count\":0").into_bytes();
    let err = info_of(wrong_type).expect_err("wrong type");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);

    // Unknown key.
    let unknown = required("\"chunk_count\":0,\"bogus_key\":1,\"style_count\":0,\"content_count\":0,\"atlas_count\":0,\"image_count\":0").into_bytes();
    let err = info_of(unknown).expect_err("unknown key");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);

    // Trailing data after the object.
    let trailing = format!("{} extra", required("\"chunk_count\":0,\"style_count\":0,\"content_count\":0,\"atlas_count\":0,\"image_count\":0")).into_bytes();
    let err = info_of(trailing).expect_err("trailing data");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);

    // Duplicate key.
    let dup = required("\"chunk_count\":0,\"chunk_count\":1,\"style_count\":0,\"content_count\":0,\"atlas_count\":0,\"image_count\":0").into_bytes();
    let err = info_of(dup).expect_err("duplicate key");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

// ---------------------------------------------------------------------------
// Determinism (JS `property.test.ts` "reading is deterministic")

#[test]
fn parsing_is_deterministic() {
    let golden = common::pipeline_golden();
    let a = parse(golden).expect("first parse");
    let b = parse(golden).expect("second parse");
    assert_eq!(a.entries, b.entries);
    assert_eq!(a.sections, b.sections);
    assert_eq!(a.unknown, b.unknown);
    assert_eq!(a.info().expect("info"), b.info().expect("info"));
    assert_eq!(a.chunks().expect("chunks"), b.chunks().expect("chunks"));
    assert_eq!(a.styles().expect("styles"), b.styles().expect("styles"));
    assert_eq!(a.content().expect("content"), b.content().expect("content"));
    assert_eq!(a.atlases().expect("atlases"), b.atlases().expect("atlases"));
    assert_eq!(a.images().expect("images"), b.images().expect("images"));
}
