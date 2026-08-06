//! Shared reader-suite support: fixture loaders, the pinned golden
//! diagnostics, and a minimal package builder.
//!
//! The builder is test-only — the production writer is the compiler. It
//! produces structurally valid containers so tests can construct malformed or
//! unusual cases the golden fixtures cannot express (unknown kinds, duplicate
//! sections, absent SEAL, reserved flags), mirroring the JS testkit builder.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
// Each integration-test crate compiles this module independently and uses a
// different subset of the builders; dead_code is expected per consumer.
#![allow(dead_code)]

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

use glyphcull_core::reader::crc32;

pub const HEADER_LEN: usize = 16;
pub const ENTRY_LEN: usize = 32;

/// A section to assemble (SPEC.md §1.2).
pub struct TestSection {
    pub kind: u32,
    /// 0 = none, 1 = zlib (RFC 1950, level 9).
    pub compression: u8,
    pub payload: Vec<u8>,
}

/// Assemble a package: header + section table + payloads. Sections appear in
/// the given order; `decoded_len` and CRC are computed from the payloads.
pub fn build_package(sections: &[TestSection]) -> Vec<u8> {
    let mut stored: Vec<(Vec<u8>, usize)> = Vec::with_capacity(sections.len());
    for section in sections {
        if section.compression == 1 {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(9));
            encoder.write_all(&section.payload).expect("zlib encode");
            let compressed = encoder.finish().expect("zlib finish");
            stored.push((compressed, section.payload.len()));
        } else {
            stored.push((section.payload.clone(), section.payload.len()));
        }
    }

    let table_len = sections.len() * ENTRY_LEN;
    let mut offset = HEADER_LEN + table_len;
    let total: usize = offset + stored.iter().map(|(s, _)| s.len()).sum::<usize>();
    let mut bytes = Vec::with_capacity(total);

    // Header (SPEC.md §1.1).
    bytes.extend_from_slice(b"CULL");
    bytes.extend_from_slice(&1u16.to_le_bytes()); // version
    bytes.extend_from_slice(&0u16.to_le_bytes()); // flags
    bytes.extend_from_slice(&(sections.len() as u32).to_le_bytes()); // section_count
    let header_crc = crc32(&bytes);
    bytes.extend_from_slice(&header_crc.to_le_bytes());

    // Section table (SPEC.md §1.2).
    for (i, section) in sections.iter().enumerate() {
        bytes.extend_from_slice(&section.kind.to_le_bytes());
        bytes.push(section.compression);
        bytes.push(0); // flags
        bytes.extend_from_slice(&0u16.to_le_bytes()); // reserved
        bytes.extend_from_slice(&(offset as u64).to_le_bytes());
        bytes.extend_from_slice(&(stored[i].0.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(stored[i].1 as u32).to_le_bytes()); // decoded_len
        bytes.extend_from_slice(&crc32(&section.payload).to_le_bytes());
        offset += stored[i].0.len();
    }

    // Payloads.
    for (stored_bytes, _) in &stored {
        bytes.extend_from_slice(stored_bytes);
    }
    bytes
}

/// The INFO payload in the deterministic JSON subset (SPEC.md §2.1): keys
/// sorted lexicographically, no whitespace, minimal escaping.
pub fn info_payload() -> Vec<u8> {
    info_payload_counts(0, 0, 0, 0, 0)
}

/// The INFO payload with explicit section counts.
pub fn info_payload_counts(
    chunk_count: u32,
    style_count: u32,
    content_count: u32,
    atlas_count: u32,
    image_count: u32,
) -> Vec<u8> {
    let source_digest = "00".repeat(32);
    let json = format!(
        "{{\"atlas_count\":{atlas_count},\"chunk_count\":{chunk_count},\"content_count\":{content_count},\"document_id\":\"0123456789abcdef0123456789abcdef\",\"format_version\":1,\"generator\":\"test-builder\",\"generator_version\":\"0.0.0\",\"image_count\":{image_count},\"source_digest\":\"{source_digest}\",\"style_count\":{style_count}}}"
    );
    json.into_bytes()
}

/// An empty CHNK payload (0 chunks, 0 extras).
pub fn empty_chnk_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// A chunk record to encode (SPEC.md §2.2).
#[derive(Clone, Debug, Default)]
pub struct TestChunk {
    pub id: u32,
    pub kind: u8,
    pub flags: u8,
    pub style_id: u32,
    pub parent_id: u32,
    pub prev_id: u32,
    pub next_id: u32,
    pub first_child_id: u32,
    pub last_child_id: u32,
    pub content_index: u32,
    pub ordinal: u32,
    pub depth: u32,
}

/// Encode a CHNK payload from chunk records and raw extras (SPEC.md §2.2).
pub fn chnk_payload(chunks: &[TestChunk], extras: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for c in chunks {
        out.extend_from_slice(&c.id.to_le_bytes());
        out.push(c.kind);
        out.push(c.flags);
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&c.style_id.to_le_bytes());
        out.extend_from_slice(&c.parent_id.to_le_bytes());
        out.extend_from_slice(&c.prev_id.to_le_bytes());
        out.extend_from_slice(&c.next_id.to_le_bytes());
        out.extend_from_slice(&c.first_child_id.to_le_bytes());
        out.extend_from_slice(&c.last_child_id.to_le_bytes());
        out.extend_from_slice(&c.content_index.to_le_bytes());
        out.extend_from_slice(&c.ordinal.to_le_bytes());
        out.extend_from_slice(&c.depth.to_le_bytes());
    }
    out.extend_from_slice(&(extras.len() as u32).to_le_bytes());
    for extra in extras {
        out.extend_from_slice(extra);
    }
    out
}

/// Encode a chunk extra's raw bytes: `u32 chunk_id`, `u8 kind`, `u8 flags`,
/// `u16 length`, then the kind-specific data (SPEC.md §2.2).
pub fn extra_bytes(chunk_id: u32, kind: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&chunk_id.to_le_bytes());
    out.push(kind);
    out.push(0); // flags
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Encode a text/image-ref CONT payload section (SPEC.md §2.4).
pub fn cont_payload(texts: &[&str], image_refs: &[u32]) -> Vec<u8> {
    let mut payloads: Vec<(u8, Vec<u8>)> = Vec::new(); // (kind, data)
    for text in texts {
        payloads.push((0, text.as_bytes().to_vec()));
    }
    for &image_id in image_refs {
        payloads.push((1, image_id.to_le_bytes().to_vec()));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(payloads.len() as u32).to_le_bytes());
    for (i, (kind, data)) in payloads.iter().enumerate() {
        out.extend_from_slice(&(i as u32).to_le_bytes()); // id
        out.push(*kind);
        out.push(0); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

/// Encode a STYL payload from `(property_count, blob)` records with dense
/// ids `0..records.len()` (SPEC.md §2.3).
pub fn styl_payload(records: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for (i, (count, blob)) in records.iter().enumerate() {
        out.extend_from_slice(&(i as u32).to_le_bytes()); // id
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(blob.len() as u16).to_le_bytes());
        out.extend_from_slice(blob);
    }
    out
}

/// Encode one STYL property: `u16 tag` + fixed-size value bytes.
pub fn style_prop(tag: u16, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(value);
    out
}

/// An image record to encode (SPEC.md §2.6).
pub struct TestImage {
    pub width: u16,
    pub height: u16,
    /// 0 = RGBA8, 1 = RGB8.
    pub format: u8,
    pub data: Vec<u8>,
}

/// Encode an IMGS payload from image records (SPEC.md §2.6).
pub fn imgs_payload(images: &[TestImage]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(images.len() as u32).to_le_bytes());
    for (i, image) in images.iter().enumerate() {
        out.extend_from_slice(&(i as u32).to_le_bytes());
        out.extend_from_slice(&image.width.to_le_bytes());
        out.extend_from_slice(&image.height.to_le_bytes());
        out.push(image.format);
        out.push(0); // flags
        out.extend_from_slice(&(image.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&image.data);
    }
    out
}

// ---------------------------------------------------------------------------
// Contract fixtures (refreshed via `scripts/refresh-fixtures.sh`)

/// The INFO-only minimal package (compiler `v1-minimal.cull`).
pub fn v1_minimal() -> &'static [u8] {
    include_bytes!("../fixtures/v1-minimal.cull")
}

/// The full pipeline golden package (compiler `golden.cull`).
pub fn pipeline_golden() -> &'static [u8] {
    include_bytes!("../fixtures/pipeline-golden.cull")
}

/// The golden source markdown (used by later layout phases).
pub fn golden_markdown() -> &'static str {
    include_str!("../fixtures/golden.md")
}

/// A pinned atlas diagnostic (from `cull inspect`).
pub struct GoldenAtlas {
    pub font_id: u32,
    pub weight: u16,
    pub italic: bool,
    pub glyphs: usize,
    pub kerning: usize,
    pub page_width: u32,
    pub pages: usize,
}

/// Expected diagnostics for `pipeline-golden.cull`, pinned from the compiler's
/// `cull inspect` output. Any drift in the fixture or the reader fails here.
pub struct GoldenExpected {
    pub document_id: &'static str,
    pub source_digest: &'static str,
    pub generator: &'static str,
    pub chunk_count: u32,
    pub style_count: u32,
    pub content_count: u32,
    pub atlas_count: u32,
    pub image_count: u32,
    /// Section kinds in file order.
    pub section_kinds: &'static [u32],
    /// (font_id, weight, italic, glyphs, kerning, page_width, pages) per atlas.
    pub atlases: &'static [GoldenAtlas],
}

/// The pinned golden diagnostics (see `test/fixtures/README.md` for
/// provenance; `scripts/refresh-fixtures.sh` refreshes the bytes).
pub const GOLDEN: GoldenExpected = GoldenExpected {
    document_id: "928da088ece3776622d6f104756a5e35",
    source_digest: "47869ba2d830d7e8599b594a98b1e446f79f85a474f8760f22eb99ba0afc70f9",
    generator: "glyphcull-compiler",
    chunk_count: 22,
    style_count: 11,
    content_count: 12,
    atlas_count: 3,
    image_count: 0,
    section_kinds: &[1, 2, 3, 4, 5, 7],
    atlases: &[
        GoldenAtlas {
            font_id: 0,
            weight: 400,
            italic: false,
            glyphs: 22,
            kerning: 12,
            page_width: 256,
            pages: 2,
        },
        GoldenAtlas {
            font_id: 1,
            weight: 400,
            italic: true,
            glyphs: 6,
            kerning: 0,
            page_width: 128,
            pages: 1,
        },
        GoldenAtlas {
            font_id: 2,
            weight: 700,
            italic: false,
            glyphs: 12,
            kerning: 8,
            page_width: 256,
            pages: 1,
        },
    ],
};
