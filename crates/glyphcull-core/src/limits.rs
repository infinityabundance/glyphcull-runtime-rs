//! Format limits (SPEC.md §1.3). The reader enforces every cap before
//! allocating or interpreting untrusted lengths.

/// Maximum number of section table entries (SPEC.md §1.3).
pub const MAX_SECTION_COUNT: u64 = 64;
/// Maximum decoded length of a single section (SPEC.md §1.3).
pub const MAX_SECTION_DECODED_LEN: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB
/// Maximum total decoded size (SPEC.md §1.3).
pub const MAX_TOTAL_DECODED: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB
/// Maximum file size (SPEC.md §1.3).
pub const MAX_FILE_LEN: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB
/// Maximum chunk records in a CHNK section (SPEC.md §1.3).
pub const MAX_CHUNK_COUNT: u64 = 1 << 28;
/// Maximum style records in a STYL section (SPEC.md §1.3).
pub const MAX_STYLE_COUNT: u64 = 1 << 24;
/// Maximum content payloads in a CONT section (SPEC.md §1.3).
pub const MAX_CONTENT_COUNT: u64 = 1 << 24;
/// Maximum atlas page dimension in texels (SPEC.md §1.3).
pub const MAX_PAGE_DIM: u64 = 8192;
/// Maximum glyph records per atlas (SPEC.md §1.3).
pub const MAX_GLYPH_COUNT: u64 = 1 << 16;
/// Maximum kerning pairs per atlas (SPEC.md §1.3).
pub const MAX_KERNING_COUNT: u64 = 1 << 24;
/// Maximum images in an IMGS section (SPEC.md §1.3).
pub const MAX_IMAGE_COUNT: u64 = 1 << 20;
/// Maximum image dimension in pixels (SPEC.md §1.3).
pub const MAX_IMAGE_DIM: u64 = 16_384;
/// Maximum INFO JSON bytes (defensive; metadata is small).
pub const MAX_INFO_LEN: u64 = 1 << 20;
/// Maximum chunk extras in a CHNK section.
pub const MAX_EXTRA_COUNT: u64 = 1 << 26;
/// Maximum chunk depth.
pub const MAX_CHUNK_DEPTH: u64 = 1 << 16;
/// Maximum atlases in a GLYF section.
pub const MAX_ATLAS_COUNT: u64 = 1 << 16;
/// Maximum family name bytes.
pub const MAX_FAMILY_LEN: u64 = 1024;
/// Maximum properties per style record.
pub const MAX_PROPERTIES_PER_STYLE: u64 = 64;
/// Maximum covered sections in a SEAL.
pub const MAX_COVERED_SECTIONS: u64 = 64;
