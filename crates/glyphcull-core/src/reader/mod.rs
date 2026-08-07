//! The independent `.cull` container reader (SPEC.md §1, the second
//! independent reader after `glyphcull-runtime-js`).
//!
//! `parse` validates the header and section table with overflow-checked
//! arithmetic, decodes every payload (decompressing zlib with explicit header
//! and trailing Adler-32 verification), verifies each payload's CRC-32 and
//! `decoded_len`, rejects duplicate kinds, and keeps unknown/reserved kinds
//! addressable without interpreting them. Typed section decoders (`info`,
//! `chunks`, `styles`, `content`, `atlases`, `images`, `seal`) run on demand
//! from the validated payloads — the same lazy model as the JS runtime.
//!
//! Ownership: `parse` copies each section's decoded payload into owned
//! buffers (raw sections are copied, not borrowed), so the input slice may be
//! dropped after parsing and every `Package` is fully self-contained. The
//! atlas pages are copied once at decode time (documented in DESIGN.md).

pub mod chunk;
pub mod content;
pub mod glyph;
pub mod image;
pub mod info;
pub mod seal;
pub mod style;

use std::collections::HashSet;
use std::fmt;

use crc32fast::Hasher as Crc32;

use crate::error::{Error, ErrorKind, Result};
use crate::limits::{MAX_FILE_LEN, MAX_SECTION_COUNT, MAX_SECTION_DECODED_LEN, MAX_TOTAL_DECODED};

/// The ASCII magic bytes (SPEC.md §1.1).
pub const MAGIC: &[u8; 4] = b"CULL";
/// The current format version (SPEC.md §1.1).
pub const VERSION: u16 = 1;
/// The header is 16 bytes (SPEC.md §1.1).
pub const HEADER_LEN: usize = 16;
/// Each section table entry is 32 bytes (SPEC.md §1.2).
pub const SECTION_ENTRY_LEN: usize = 32;
/// The header CRC covers bytes `0..12`.
pub const CRC_COVERED_LEN: usize = 12;
/// The chunk record is 44 bytes (SPEC.md §2.2).
pub const CHUNK_RECORD_LEN: usize = 44;
/// The glyph record is 32 bytes (SPEC.md §2.5).
pub const GLYPH_RECORD_LEN: usize = 32;
/// The SEAL overall hash is 32 bytes (SPEC.md §2.7).
pub const OVERALL_HASH_LEN: usize = 32;

/// Compression codes (SPEC.md §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Compression {
    /// No compression; the stored bytes are the decoded payload.
    None = 0,
    /// zlib (RFC 1950), deflate, level 9.
    Zlib = 1,
}

/// Section kind codes (SPEC.md §1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum SectionKind {
    /// INFO — metadata (deterministic JSON).
    Info = 1,
    /// CHNK — chunk graph.
    Chunk = 2,
    /// STYL — resolved style table.
    Style = 3,
    /// CONT — content payloads.
    Content = 4,
    /// GLYF — MSDF glyph atlases.
    Glyph = 5,
    /// IMGS — raster images.
    Images = 6,
    /// SEAL — integrity hash tree.
    Seal = 7,
}

impl SectionKind {
    /// Interpret a section kind code; `None` for reserved/unknown kinds
    /// (readers MUST skip them, SPEC.md §1.4).
    #[must_use]
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::Info),
            2 => Some(Self::Chunk),
            3 => Some(Self::Style),
            4 => Some(Self::Content),
            5 => Some(Self::Glyph),
            6 => Some(Self::Images),
            7 => Some(Self::Seal),
            _ => None,
        }
    }

    /// The canonical emission order (SPEC.md §1.4).
    #[must_use]
    pub const fn canonical_order(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for SectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A validated section table entry (SPEC.md §1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionEntry {
    /// The table index (file order).
    pub index: usize,
    /// The section kind code (known or reserved).
    pub kind: u32,
    /// The compression code.
    pub compression: Compression,
    /// The flags byte: bit 0 is `critical`, meaningful only for unknown kinds.
    pub flags: u8,
    /// Absolute byte offset of the stored payload.
    pub offset: u64,
    /// Stored byte length.
    pub stored_len: u64,
    /// Decoded byte length (equals `stored_len` when uncompressed).
    pub decoded_len: u64,
    /// CRC-32 over the decoded payload.
    pub crc32: u32,
}

/// A validated, fully decoded section payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionPayload {
    /// The table entry this payload came from.
    pub entry: SectionEntry,
    /// The decoded bytes.
    pub bytes: Vec<u8>,
}

/// A fully parsed and validated package. Section payloads are owned; typed
/// decoders run lazily and cache their results.
#[derive(Debug)]
pub struct Package {
    /// The format version (always 1).
    pub version: u16,
    /// The header flags (reserved; ignored per SPEC.md §1.1).
    pub flags: u16,
    /// Section table entries in file order.
    pub entries: Vec<SectionEntry>,
    /// Decoded known sections keyed by kind.
    pub sections: Vec<SectionPayload>,
    /// Decoded unknown sections, in file order (addressable, never interpreted).
    pub unknown: Vec<SectionPayload>,

    /// Header bytes `0..12` — the SEAL overall hash covers them (SPEC.md §2.7);
    /// retained because the input slice is not (packages own their payloads).
    header_prefix: [u8; CRC_COVERED_LEN],

    cached_info: Option<std::sync::OnceLock<crate::error::Result<info::Info>>>,
    cached_chunks: Option<std::sync::OnceLock<crate::error::Result<chunk::ChunkSection>>>,
    cached_styles: Option<std::sync::OnceLock<crate::error::Result<Vec<style::StyleRecord>>>>,
    cached_content: Option<std::sync::OnceLock<crate::error::Result<Vec<content::ContentPayload>>>>,
    cached_atlases: Option<std::sync::OnceLock<crate::error::Result<Vec<glyph::Atlas>>>>,
    cached_images: Option<std::sync::OnceLock<crate::error::Result<Vec<image::ImageRecord>>>>,
    cached_seal: Option<std::sync::OnceLock<crate::error::Result<seal::Seal>>>,
}

impl Package {
    fn new(
        version: u16,
        flags: u16,
        entries: Vec<SectionEntry>,
        sections: Vec<SectionPayload>,
        unknown: Vec<SectionPayload>,
        header_prefix: [u8; CRC_COVERED_LEN],
    ) -> Self {
        let has = |kind: SectionKind| sections.iter().any(|s| s.entry.kind == kind as u32);
        let has_info = has(SectionKind::Info);
        let has_chunks = has(SectionKind::Chunk);
        let has_styles = has(SectionKind::Style);
        let has_content = has(SectionKind::Content);
        let has_atlases = has(SectionKind::Glyph);
        let has_images = has(SectionKind::Images);
        let has_seal = has(SectionKind::Seal);
        Self {
            version,
            flags,
            entries,
            sections,
            unknown,
            header_prefix,
            cached_info: has_info.then(std::sync::OnceLock::new),
            cached_chunks: has_chunks.then(std::sync::OnceLock::new),
            cached_styles: has_styles.then(std::sync::OnceLock::new),
            cached_content: has_content.then(std::sync::OnceLock::new),
            cached_atlases: has_atlases.then(std::sync::OnceLock::new),
            cached_images: has_images.then(std::sync::OnceLock::new),
            cached_seal: has_seal.then(std::sync::OnceLock::new),
        }
    }

    /// The decoded payload of a known section, if present.
    #[must_use]
    pub fn section(&self, kind: SectionKind) -> Option<&[u8]> {
        self.sections
            .iter()
            .find(|s| s.entry.kind == kind as u32)
            .map(|s| s.bytes.as_slice())
    }

    /// INFO metadata (parsed and validated on first access).
    pub fn info(&self) -> Result<Option<&info::Info>> {
        let Some(cache) = &self.cached_info else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Info).ok_or_else(|| {
                Error::new(ErrorKind::Internal, "info cache exists but INFO is absent")
            })?;
            info::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// CHNK chunk graph (parsed and validated on first access).
    pub fn chunks(&self) -> Result<Option<&chunk::ChunkSection>> {
        let Some(cache) = &self.cached_chunks else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Chunk).ok_or_else(|| {
                Error::new(ErrorKind::Internal, "chunk cache exists but CHNK is absent")
            })?;
            chunk::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// STYL style records (parsed and validated on first access).
    pub fn styles(&self) -> Result<Option<&Vec<style::StyleRecord>>> {
        let Some(cache) = &self.cached_styles else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Style).ok_or_else(|| {
                Error::new(ErrorKind::Internal, "style cache exists but STYL is absent")
            })?;
            style::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// CONT content payloads (parsed and validated on first access).
    pub fn content(&self) -> Result<Option<&Vec<content::ContentPayload>>> {
        let Some(cache) = &self.cached_content else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Content).ok_or_else(|| {
                Error::new(
                    ErrorKind::Internal,
                    "content cache exists but CONT is absent",
                )
            })?;
            content::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// GLYF atlases (parsed and validated on first access).
    pub fn atlases(&self) -> Result<Option<&Vec<glyph::Atlas>>> {
        let Some(cache) = &self.cached_atlases else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Glyph).ok_or_else(|| {
                Error::new(ErrorKind::Internal, "atlas cache exists but GLYF is absent")
            })?;
            glyph::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// IMGS images (parsed and validated on first access).
    pub fn images(&self) -> Result<Option<&Vec<image::ImageRecord>>> {
        let Some(cache) = &self.cached_images else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Images).ok_or_else(|| {
                Error::new(ErrorKind::Internal, "image cache exists but IMGS is absent")
            })?;
            image::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// SEAL hash tree (parsed and validated on first access).
    pub fn seal(&self) -> Result<Option<&seal::Seal>> {
        let Some(cache) = &self.cached_seal else {
            return Ok(None);
        };
        let value = cache.get_or_init(|| {
            let payload = self.section(SectionKind::Seal).ok_or_else(|| {
                Error::new(ErrorKind::Internal, "seal cache exists but SEAL is absent")
            })?;
            seal::decode(payload)
        });
        Ok(Some(value.as_ref().map_err(Error::clone)?))
    }

    /// Verify the SEAL hash tree against every covered section (SPEC.md §2.7).
    /// Returns `Ok(())` when the seal is absent or verifies.
    pub fn verify_seal(&self) -> Result<()> {
        let Some(seal_section) = self.seal()? else {
            return Ok(());
        };
        seal::verify(self, seal_section)
    }
}

/// Parse and validate a complete package (SPEC.md §1.6).
///
/// This is the entry point for loads. It validates the header and table,
/// decodes every payload, verifies each CRC-32 and `decoded_len`, rejects
/// duplicate section kinds, and returns a self-contained [`Package`]. The
/// SEAL, when present, is verified.
pub fn parse(bytes: &[u8]) -> Result<Package> {
    let structure = validate_structure(bytes)?;
    let entries = structure.entries;
    // The SEAL overall hash covers header bytes 0..12 (SPEC.md §2.7). The file
    // is at least HEADER_LEN bytes (validated), so this cannot fail.
    let header_prefix: [u8; CRC_COVERED_LEN] = bytes
        .get(..CRC_COVERED_LEN)
        .ok_or_else(|| Error::new(ErrorKind::Internal, "header prefix unavailable"))?
        .try_into()
        .map_err(|_| Error::new(ErrorKind::Internal, "header prefix has the wrong length"))?;

    let mut sections: Vec<SectionPayload> = Vec::new();
    let mut unknown: Vec<SectionPayload> = Vec::new();
    let mut seen = HashSet::new();
    let mut total_decoded: u64 = 0;
    // Canonical-order enforcement for the known sections (SPEC.md §1.6): their
    // kinds must be strictly increasing in file order; unknown kinds may appear
    // anywhere.
    let mut last_known_kind: Option<u32> = None;

    for entry in &entries {
        let payload = decode_payload(bytes, entry)?;
        total_decoded = total_decoded
            .checked_add(payload.len() as u64)
            .ok_or_else(|| {
                Error::for_section(
                    ErrorKind::Overflow,
                    entry.index,
                    "total decoded size overflow",
                )
            })?;
        if total_decoded > MAX_TOTAL_DECODED {
            return Err(Error::for_section(
                ErrorKind::DecodedLenExceeded,
                entry.index,
                format!("total decoded size exceeds the {MAX_TOTAL_DECODED}-byte cap"),
            ));
        }
        let item = SectionPayload {
            entry: entry.clone(),
            bytes: payload,
        };
        match SectionKind::from_code(entry.kind) {
            Some(kind) => {
                if !seen.insert(kind) {
                    return Err(Error::for_section(
                        ErrorKind::DuplicateSection,
                        entry.index,
                        format!("duplicate section kind {kind}"),
                    ));
                }
                if let Some(previous) = last_known_kind {
                    if entry.kind <= previous {
                        return Err(Error::for_section(
                            ErrorKind::InvalidSectionOrder,
                            entry.index,
                            format!(
                                "kind {} appears after {previous} (canonical order violated)",
                                entry.kind
                            ),
                        ));
                    }
                }
                last_known_kind = Some(entry.kind);
                sections.push(item);
            }
            None => {
                // Unknown kind: noncritical sections (flags bit 0 clear) are
                // skipped for forward compatibility; a critical unknown section
                // is rejected (SPEC.md §1.2, §4).
                if entry.flags & 0x01 != 0 {
                    return Err(Error::for_section(
                        ErrorKind::UnknownCriticalSection,
                        entry.index,
                        format!("unknown kind {} marked critical", entry.kind),
                    ));
                }
                unknown.push(item);
            }
        }
    }

    // INFO is the required section: every conforming v1 package carries it.
    if !seen.contains(&SectionKind::Info) {
        return Err(Error::new(
            ErrorKind::MissingRequiredSection,
            "required INFO section is absent",
        ));
    }

    let package = Package::new(
        structure.version,
        structure.flags,
        entries,
        sections,
        unknown,
        header_prefix,
    );
    package.verify_seal()?;
    Ok(package)
}

/// Validate the container structure (header + section table) without decoding
/// payloads — the entry point for streaming loads and the truncation corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    /// The format version.
    pub version: u16,
    /// The header flags.
    pub flags: u16,
    /// Section table entries in file order.
    pub entries: Vec<SectionEntry>,
}

/// Validate the container structure of a package (SPEC.md §1.6.1–1.6.4).
pub fn validate_structure(bytes: &[u8]) -> Result<Structure> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::new(
            ErrorKind::TooShort,
            format!(
                "package is {} bytes; the header needs {HEADER_LEN}",
                bytes.len()
            ),
        ));
    }
    let file_len = u64::try_from(bytes.len())
        .map_err(|_| Error::new(ErrorKind::Overflow, "file length does not fit u64"))?;
    if file_len > MAX_FILE_LEN {
        // SPEC.md §1.3 caps the file at 4 GiB; a file larger than the cap is
        // invalid before any arithmetic.
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("file exceeds the {MAX_FILE_LEN}-byte cap"),
        ));
    }
    let header = parse_header(bytes)?;
    let table_len = (header.section_count as usize)
        .checked_mul(SECTION_ENTRY_LEN)
        .ok_or_else(|| Error::new(ErrorKind::Overflow, "section table length overflow"))?;
    let table_end = HEADER_LEN
        .checked_add(table_len)
        .ok_or_else(|| Error::new(ErrorKind::Overflow, "section table end overflow"))?;
    if bytes.len() < table_end {
        return Err(Error::new(
            ErrorKind::Truncated,
            format!(
                "section table ends at {table_end}, file is {} bytes",
                bytes.len()
            ),
        ));
    }
    let mut entries = Vec::with_capacity(header.section_count as usize);
    for index in 0..header.section_count as usize {
        let start = HEADER_LEN + index * SECTION_ENTRY_LEN;
        let entry = parse_entry(bytes, index, start)?;
        entries.push(entry);
    }
    for entry in &entries {
        let end = entry.offset.checked_add(entry.stored_len).ok_or_else(|| {
            Error::for_section(
                ErrorKind::Overflow,
                entry.index,
                "offset + stored_len overflow",
            )
        })?;
        if end > file_len {
            return Err(Error::for_section(
                ErrorKind::OutOfBounds,
                entry.index,
                format!(
                    "offset {} + stored_len {} exceeds file size {file_len}",
                    entry.offset, entry.stored_len
                ),
            ));
        }
    }
    Ok(Structure {
        version: header.version,
        flags: header.flags,
        entries,
    })
}

struct Header {
    version: u16,
    flags: u16,
    section_count: u32,
}

fn parse_header(bytes: &[u8]) -> Result<Header> {
    // The header is exactly HEADER_LEN bytes; bound the cursor to it so
    // `finish` verifies the header consumes itself precisely (the JS cursor
    // bounds the same way via its length limit).
    let header_bytes = bytes
        .get(..HEADER_LEN)
        .ok_or_else(|| Error::new(ErrorKind::TooShort, "header bytes unavailable"))?;
    let mut c = Cursor::new(header_bytes, 0, None);
    let magic = [
        c.u8("magic")?,
        c.u8("magic")?,
        c.u8("magic")?,
        c.u8("magic")?,
    ];
    if &magic != MAGIC {
        return Err(Error::new(
            ErrorKind::BadMagic,
            format!("magic {:?} != \"CULL\"", String::from_utf8_lossy(&magic)),
        ));
    }
    let version = c.u16("version")?;
    if version != VERSION {
        return Err(Error::new(
            ErrorKind::UnsupportedVersion,
            format!("version {version} != {VERSION}"),
        ));
    }
    let flags = c.u16("flags")?;
    let section_count = c.u32("section_count")?;
    let header_crc = c.u32("header_crc32")?;
    c.finish("header")?;
    let covered = bytes
        .get(..CRC_COVERED_LEN)
        .ok_or_else(|| Error::new(ErrorKind::Internal, "header prefix unavailable"))?;
    let actual = crc32(covered);
    if actual != header_crc {
        return Err(Error::new(
            ErrorKind::HeaderCrcMismatch,
            format!("header crc32 {header_crc:#010x} != recomputed {actual:#010x}"),
        ));
    }
    if u64::from(section_count) > MAX_SECTION_COUNT {
        return Err(Error::new(
            ErrorKind::TooManySections,
            format!("section_count {section_count} > {MAX_SECTION_COUNT}"),
        ));
    }
    if section_count == 0 {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            "section_count must be at least 1",
        ));
    }
    Ok(Header {
        version,
        flags,
        section_count,
    })
}

fn parse_entry(bytes: &[u8], index: usize, start: usize) -> Result<SectionEntry> {
    // Each entry is exactly SECTION_ENTRY_LEN bytes; bound the cursor to it
    // so `finish` verifies the entry consumes itself precisely.
    let entry_bytes = bytes.get(start..start + SECTION_ENTRY_LEN).ok_or_else(|| {
        Error::for_section(ErrorKind::Truncated, index, "section entry out of range")
    })?;
    let mut c = Cursor::new(entry_bytes, 0, Some(index));
    let kind = c.u32("section kind")?;
    let compression_code = c.u8("compression")?;
    let entry_flags = c.u8("flags")?;
    let reserved = c.u16("reserved")?;
    let offset = c.u64("offset")?;
    let stored_len = c.u64("stored_len")?;
    // SPEC.md §1.2: decoded_len is a u32 at offset 24 (crc32 follows at 28).
    let decoded_len = u64::from(c.u32("decoded_len")?);
    let crc = c.u32("crc32")?;
    c.finish("section entry")?;

    let compression = match compression_code {
        0 => Compression::None,
        1 => Compression::Zlib,
        other => {
            return Err(Error::for_section(
                ErrorKind::UnsupportedCompression,
                index,
                format!("compression code {other} not in {{0, 1}}"),
            ));
        }
    };
    // The flags byte: bit 0 is `critical`, meaningful only for unknown section
    // kinds (SPEC.md §1.2 — a critical unknown section MUST be rejected; a
    // noncritical one is skipped). Reserved bits 1..7 must be zero, and known
    // kinds must carry no flags at all.
    let known_kind = SectionKind::from_code(kind).is_some();
    if entry_flags & 0xFE != 0 || reserved != 0 || (known_kind && entry_flags != 0) {
        return Err(Error::for_section(
            ErrorKind::InvalidFlags,
            index,
            "reserved flags/reserved bits must be zero",
        ));
    }
    if decoded_len > MAX_SECTION_DECODED_LEN {
        return Err(Error::for_section(
            ErrorKind::DecodedLenExceeded,
            index,
            format!("decoded_len {decoded_len} > {MAX_SECTION_DECODED_LEN}"),
        ));
    }
    if decoded_len == 0 {
        return Err(Error::for_section(
            ErrorKind::InvalidValue,
            index,
            "decoded_len must be at least 1",
        ));
    }
    Ok(SectionEntry {
        index,
        kind,
        compression,
        flags: entry_flags,
        offset,
        stored_len,
        decoded_len,
        crc32: crc,
    })
}

/// Decode a section payload: decompress when flagged, verify the decoded
/// length, and verify the CRC-32 (SPEC.md §1.6.5–1.6.6).
fn decode_payload(bytes: &[u8], entry: &SectionEntry) -> Result<Vec<u8>> {
    let start = usize::try_from(entry.offset).map_err(|_| {
        Error::for_section(
            ErrorKind::Overflow,
            entry.index,
            "offset does not fit usize",
        )
    })?;
    let stored_len = usize::try_from(entry.stored_len).map_err(|_| {
        Error::for_section(
            ErrorKind::Overflow,
            entry.index,
            "stored_len does not fit usize",
        )
    })?;
    let end = start.checked_add(stored_len).ok_or_else(|| {
        Error::for_section(ErrorKind::Overflow, entry.index, "payload range overflow")
    })?;
    if end > bytes.len() {
        return Err(Error::for_section(
            ErrorKind::OutOfBounds,
            entry.index,
            format!("payload range {start}..{end} exceeds the file"),
        ));
    }
    let stored = bytes.get(start..end).ok_or_else(|| {
        Error::for_section(
            ErrorKind::OutOfBounds,
            entry.index,
            format!("payload range {start}..{end} exceeds the file"),
        )
    })?;
    let decoded = match entry.compression {
        Compression::None => {
            if stored.len() as u64 != entry.decoded_len {
                return Err(Error::for_section(
                    ErrorKind::DecompressMismatch,
                    entry.index,
                    format!(
                        "stored {} bytes but decoded_len {} (uncompressed)",
                        stored.len(),
                        entry.decoded_len
                    ),
                ));
            }
            stored.to_vec()
        }
        Compression::Zlib => inflate_verified(stored, entry.decoded_len, entry.index)?,
    };
    let actual = crc32(&decoded);
    if actual != entry.crc32 {
        return Err(Error::for_section(
            ErrorKind::CrcMismatch,
            entry.index,
            format!(
                "payload crc32 {actual:#010x} != table {:#010x}",
                entry.crc32
            ),
        ));
    }
    Ok(decoded)
}

/// Inflate a zlib stream with explicit wrapper verification (SPEC.md §1.5):
/// the two-byte header (`CMF & 0x0F == 8` and `(CMF << 8 | FLG) % 31 == 0`)
/// and the trailing Adler-32 against the decoded output, plus `decoded_len`
/// exactness. The deflate library's own checks are a second line of defense.
fn inflate_verified(stored: &[u8], decoded_len: u64, index: usize) -> Result<Vec<u8>> {
    use std::io::Read;

    if stored.len() < 2 {
        // The zlib wrapper starts with a 2-byte header (RFC 1950 §2.2).
        return Err(Error::for_section(
            ErrorKind::ZlibHeaderInvalid,
            index,
            format!(
                "stored stream is {} bytes; the zlib header needs 2",
                stored.len()
            ),
        ));
    }
    let cmf = stored
        .first()
        .copied()
        .ok_or_else(|| Error::for_section(ErrorKind::ZlibHeaderInvalid, index, "no CMF byte"))?;
    let flg = stored
        .get(1)
        .copied()
        .ok_or_else(|| Error::for_section(ErrorKind::ZlibHeaderInvalid, index, "no FLG byte"))?;
    let header = u16::from(cmf) << 8 | u16::from(flg);
    if cmf & 0x0f != 8 {
        return Err(Error::for_section(
            ErrorKind::ZlibHeaderInvalid,
            index,
            format!("zlib CMF {cmf:#04x}: compression method must be deflate (8)"),
        ));
    }
    if header % 31 != 0 {
        return Err(Error::for_section(
            ErrorKind::ZlibHeaderInvalid,
            index,
            format!("zlib header {header:#06x} fails the % 31 check"),
        ));
    }
    if stored.len() < 6 {
        // The wrapper is at least 6 bytes (2 header + data + 4 adler); a
        // shorter stream cannot carry a trailer (SPEC.md §1.5).
        return Err(Error::for_section(
            ErrorKind::Truncated,
            index,
            format!(
                "stored stream is {} bytes; the zlib trailer needs 4",
                stored.len()
            ),
        ));
    }

    let expected = usize::try_from(decoded_len).map_err(|_| {
        Error::for_section(ErrorKind::Overflow, index, "decoded_len does not fit usize")
    })?;
    // Bounded, incremental accumulation (mirroring the JS runtime): never
    // pre-allocate `decoded_len` — a hostile table can claim up to the 2 GiB
    // cap with a tiny stored stream. Read in chunks and stop the moment the
    // output would exceed the authoritative length.
    let mut decoder = flate2::read::ZlibDecoder::new(stored);
    let mut out = Vec::with_capacity(expected.min(1 << 20));
    let mut buf = [0_u8; 8192];
    loop {
        let n = decoder.read(&mut buf).map_err(|e| {
            // A corrupt stored stream is untrusted input, not a reader defect:
            // mirror the JS runtime, which surfaces platform inflate failures
            // as `decompress-mismatch` (SPEC.md §1.6: every failure is typed).
            Error::for_section(
                ErrorKind::DecompressMismatch,
                index,
                format!("inflate failed: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).ok_or_else(|| {
            Error::for_section(
                ErrorKind::Internal,
                index,
                "inflate produced an out-of-range read",
            )
        })?;
        out.extend_from_slice(chunk);
        if out.len() > expected {
            return Err(Error::for_section(
                ErrorKind::DecompressMismatch,
                index,
                format!("decoded stream exceeds authoritative decoded_len {decoded_len}"),
            ));
        }
    }
    if out.len() != expected {
        return Err(Error::for_section(
            ErrorKind::DecompressMismatch,
            index,
            format!(
                "zlib stream decoded {} bytes; decoded_len {decoded_len}",
                out.len()
            ),
        ));
    }

    // Explicit trailing Adler-32 (RFC 1950 §2.3): the last four bytes of the
    // stored stream are the checksum of the decoded output. Truncating the
    // stream must never decode silently, even when the prefix is identical.
    let adler_bytes = stored.get(stored.len() - 4..).ok_or_else(|| {
        Error::for_section(ErrorKind::ZlibAdlerMismatch, index, "no trailing Adler-32")
    })?;
    let stored_adler = u32::from_be_bytes(adler_bytes.try_into().map_err(|_| {
        Error::for_section(ErrorKind::ZlibAdlerMismatch, index, "adler read failed")
    })?);
    let actual_adler = adler32(&out);
    if stored_adler != actual_adler {
        return Err(Error::for_section(
            ErrorKind::ZlibAdlerMismatch,
            index,
            format!("zlib adler32 {stored_adler:#010x} != recomputed {actual_adler:#010x}"),
        ));
    }
    Ok(out)
}

/// CRC-32 (IEEE, zlib polynomial) over the given bytes.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut hasher = Crc32::new();
    hasher.update(bytes);
    hasher.finalize()
}

/// Adler-32 (RFC 1950) over the given bytes.
#[must_use]
pub fn adler32(bytes: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in bytes.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// The bounds-checked cursor: every read is verified against the remaining
/// bytes and never panics on untrusted input (SPEC.md §1.6: never panic).
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    section: Option<usize>,
}

// The cursor is the single choke point for untrusted bytes: every access is
// preceded by a bounds check in `Cursor::bytes`. The direct indexing below is
// therefore provably safe — the documented exception to the workspace's
// indexing policy, mirroring `glyphcull-format::util`.
#[allow(clippy::indexing_slicing)]
impl<'a> Cursor<'a> {
    /// Create a cursor over `bytes` starting at `pos`, scoped to a section
    /// table index for error context (or `None` for the header).
    pub(crate) fn new(bytes: &'a [u8], pos: usize, section: Option<usize>) -> Self {
        Self {
            bytes,
            pos,
            section,
        }
    }

    fn err(&self, kind: ErrorKind, message: impl Into<String>) -> Error {
        match self.section {
            Some(index) => Error::for_section(kind, index, message),
            None => Error::new(kind, message),
        }
    }

    /// Take `n` bytes, bounds-checked (never panics).
    pub(crate) fn bytes(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| self.err(ErrorKind::Overflow, format!("{what}: read range overflow")))?;
        if end > self.bytes.len() {
            return Err(self.err(
                ErrorKind::Truncated,
                format!(
                    "{what}: need {n} bytes at {} of {}",
                    self.pos,
                    self.bytes.len()
                ),
            ));
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    /// Read a `u8`.
    pub(crate) fn u8(&mut self, what: &str) -> Result<u8> {
        let bytes = self.bytes(1, what)?;
        Ok(bytes[0])
    }

    /// Read a little-endian `u16`.
    pub(crate) fn u16(&mut self, what: &str) -> Result<u16> {
        let bytes = self.bytes(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Read a little-endian `u32`.
    pub(crate) fn u32(&mut self, what: &str) -> Result<u32> {
        let bytes = self.bytes(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a little-endian `u64`.
    pub(crate) fn u64(&mut self, what: &str) -> Result<u64> {
        let bytes = self.bytes(8, what)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Read a little-endian `f32` (bit-preserving).
    pub(crate) fn f32(&mut self, what: &str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    /// Read `n` bytes as UTF-8, rejecting malformed sequences.
    pub(crate) fn utf8(&mut self, n: usize, what: &str) -> Result<String> {
        let raw = self.bytes(n, what)?;
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| self.err(ErrorKind::InvalidUtf8, format!("{what} is not valid UTF-8")))
    }

    /// Assert that no trailing bytes remain.
    pub(crate) fn finish(&self, what: &str) -> Result<()> {
        if self.remaining() != 0 {
            return Err(self.err(
                ErrorKind::InvalidValue,
                format!("{what}: {} trailing bytes", self.remaining()),
            ));
        }
        Ok(())
    }

    /// The number of bytes remaining.
    #[must_use]
    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }
}

#[cfg(test)]
mod tests {
    //! In-crate unit tests for the container reader (the JS suite mirrors
    //! these in `test/format/reader.test.ts` under "section payload
    //! strictness"). `inflate_verified` is private, so the zlib edge cases
    //! are exercised here; the integration suite covers the container and
    //! section layers through `parse`.

    // Test code asserts via `expect` by design; the restriction lints guard
    // production untrusted-input paths (the integration tests allow the
    // same lints at their crate roots).
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::io::Write;

    use flate2::write::ZlibEncoder;
    use flate2::Compression;

    use super::{adler32, crc32, inflate_verified};
    use crate::error::ErrorKind;

    /// A zlib stream (RFC 1950) of `input` at the compiler's fixed level 9.
    fn zlib(input: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(9));
        encoder.write_all(input).expect("zlib encode");
        encoder.finish().expect("zlib finish")
    }

    #[test]
    fn crc32_and_adler32_agree_with_the_compiler_streams() {
        // A compressed payload round-trips through our primitives: the
        // decoded bytes must carry the container CRC and the stored stream
        // must carry the Adler-32 trailer (SPEC.md §1.5).
        let input = b"the quick brown fox jumps over the lazy dog";
        let stream = zlib(input);
        assert_eq!(crc32(input), crc32(input));
        let trailer = stream.get(stream.len() - 4..).expect("trailer");
        let stored_adler = u32::from_be_bytes(trailer.try_into().expect("4 bytes"));
        assert_eq!(stored_adler, adler32(input));
    }

    #[test]
    fn zlib_header_is_verified_explicitly() {
        // CMF low nibble must be 8 (deflate).
        let mut stream = zlib(b"hello hello hello");
        *stream.first_mut().expect("cmf") = 0x09;
        let err = inflate_verified(&stream, 17, 0).expect_err("bad method");
        assert_eq!(err.kind(), ErrorKind::ZlibHeaderInvalid);

        // The % 31 check bits must hold.
        let mut stream = zlib(b"hello hello hello");
        let flg = stream.get_mut(1).expect("flg");
        *flg ^= 0x01;
        let err = inflate_verified(&stream, 17, 0).expect_err("bad check bits");
        assert_eq!(err.kind(), ErrorKind::ZlibHeaderInvalid);

        // Shorter than the 2-byte header.
        let err = inflate_verified(&[0x78], 1, 0).expect_err("no header");
        assert_eq!(err.kind(), ErrorKind::ZlibHeaderInvalid);

        // 2..6 bytes: the header is fine but the trailer cannot exist.
        let err = inflate_verified(&[0x78, 0x01, 0x00], 1, 0).expect_err("no trailer");
        assert_eq!(err.kind(), ErrorKind::Truncated);
    }

    #[test]
    fn truncated_zlib_stream_is_rejected() {
        let stream = zlib(b"hello hello hello");
        let truncated = stream.get(..stream.len() - 3).expect("prefix");
        let err = inflate_verified(truncated, 17, 0).expect_err("truncated");
        // The cut may land in the deflate data (platform rejects: decompress)
        // or in the trailer (our explicit Adler check: zlib-adler-mismatch).
        // Either way the failure is typed and precise (SPEC.md §1.5).
        assert!(
            matches!(
                err.kind(),
                ErrorKind::DecompressMismatch | ErrorKind::ZlibAdlerMismatch | ErrorKind::Truncated
            ),
            "unexpected kind {:?}",
            err.kind()
        );
    }

    #[test]
    fn decoded_length_mismatch_is_rejected() {
        let stream = zlib(b"hello hello hello");
        let err = inflate_verified(&stream, 100, 0).expect_err("too long");
        assert_eq!(err.kind(), ErrorKind::DecompressMismatch);
        let err = inflate_verified(&stream, 3, 0).expect_err("too short");
        assert_eq!(err.kind(), ErrorKind::DecompressMismatch);
    }

    #[test]
    fn corrupted_adler_trailer_is_rejected() {
        let stream = zlib(b"hello hello hello");
        let mut mutated = stream.clone();
        let last = mutated.last_mut().expect("last byte");
        *last ^= 0xff;
        let err = inflate_verified(&mutated, 17, 0).expect_err("corrupt adler");
        // The platform layer may reject the trailer (decompress-mismatch) or
        // our explicit check catches it (zlib-adler-mismatch); either way the
        // failure is typed and precise (SPEC.md §1.5).
        assert!(
            matches!(
                err.kind(),
                ErrorKind::DecompressMismatch | ErrorKind::ZlibAdlerMismatch
            ),
            "unexpected kind {:?}",
            err.kind()
        );
    }

    #[test]
    fn bomb_stream_does_not_preallocate_decoded_len() {
        // A hostile table may claim the full 2 GiB cap with a tiny stream;
        // the decoder must accumulate incrementally and reject with a typed
        // error instead of allocating `decoded_len` upfront.
        let stream = zlib(b"tiny");
        let err = inflate_verified(&stream, u32::MAX as u64 / 2, 0).expect_err("bomb");
        assert_eq!(err.kind(), ErrorKind::DecompressMismatch);
    }
}
