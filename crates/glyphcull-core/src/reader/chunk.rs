//! CHNK section decoder (SPEC.md §2.2): the chunk graph — fixed 44-byte
//! chunk records followed by variable extras. The reader validates the raw
//! record structure (dense ids, kind range, flag bits, depth, structural-flag
//! consistency); the document model (Phase 4.2) validates the graph
//! invariants (tree consistency, reference resolution).

use crate::error::{Error, ErrorKind, Result};
use crate::limits::{MAX_CHUNK_COUNT, MAX_CHUNK_DEPTH, MAX_EXTRA_COUNT};
use crate::reader::{Cursor, CHUNK_RECORD_LEN};

/// The chunk kind codes (SPEC.md §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ChunkKind {
    /// `document` — structural.
    Document = 1,
    /// `heading1`.
    Heading1 = 2,
    /// `heading2`.
    Heading2 = 3,
    /// `heading3`.
    Heading3 = 4,
    /// `heading4`.
    Heading4 = 5,
    /// `heading5`.
    Heading5 = 6,
    /// `heading6`.
    Heading6 = 7,
    /// `paragraph`.
    Paragraph = 8,
    /// `quote`.
    Quote = 9,
    /// `list` — structural.
    List = 10,
    /// `list_item` (renders a marker).
    ListItem = 11,
    /// `code_block`.
    CodeBlock = 12,
    /// `table` — structural.
    Table = 13,
    /// `table_row` — structural.
    TableRow = 14,
    /// `table_cell`.
    TableCell = 15,
    /// `image`.
    Image = 16,
    /// `caption`.
    Caption = 17,
    /// `run` — inline text.
    Run = 18,
    /// `link` — inline.
    Link = 19,
    /// `br` — inline.
    Br = 20,
    /// `hr`.
    Hr = 21,
}

impl ChunkKind {
    /// Interpret a chunk kind code; `None` for out-of-range codes.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Document),
            2 => Some(Self::Heading1),
            3 => Some(Self::Heading2),
            4 => Some(Self::Heading3),
            5 => Some(Self::Heading4),
            6 => Some(Self::Heading5),
            7 => Some(Self::Heading6),
            8 => Some(Self::Paragraph),
            9 => Some(Self::Quote),
            10 => Some(Self::List),
            11 => Some(Self::ListItem),
            12 => Some(Self::CodeBlock),
            13 => Some(Self::Table),
            14 => Some(Self::TableRow),
            15 => Some(Self::TableCell),
            16 => Some(Self::Image),
            17 => Some(Self::Caption),
            18 => Some(Self::Run),
            19 => Some(Self::Link),
            20 => Some(Self::Br),
            21 => Some(Self::Hr),
            _ => None,
        }
    }

    /// Whether the kind is a structural wrapper (SPEC.md §2.2: document,
    /// list, table, row).
    #[must_use]
    pub const fn is_structural(self) -> bool {
        matches!(
            self,
            Self::Document | Self::List | Self::Table | Self::TableRow
        )
    }
}

/// Chunk flag bits (SPEC.md §2.2).
pub mod flags {
    /// Excluded by semantic culling.
    pub const HIDDEN: u8 = 1 << 0;
    /// Layout hint: avoid a break between this chunk and the next.
    pub const KEEP_WITH_NEXT: u8 = 1 << 1;
    /// Layout hint: force a break before this chunk.
    pub const BREAK_BEFORE: u8 = 1 << 2;
    /// Suppress line wrapping (code).
    pub const NO_WRAP: u8 = 1 << 3;
    /// No direct geometry (document/list/table/row).
    pub const STRUCTURAL: u8 = 1 << 4;
    /// All valid flag bits.
    pub const ALL: u8 = 0x1f;
}

/// A fixed 44-byte chunk record (SPEC.md §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRecord {
    /// 1-based, dense in document order; 0 = "none" sentinel in link fields.
    pub id: u32,
    /// The chunk kind.
    pub kind: ChunkKind,
    /// The flag bitmask.
    pub flags: u8,
    /// Style id (0 = document default style).
    pub style_id: u32,
    /// Parent id (0 = none).
    pub parent_id: u32,
    /// Previous sibling id (0 = none).
    pub prev_id: u32,
    /// Next sibling id (0 = none).
    pub next_id: u32,
    /// First child id (0 = none).
    pub first_child_id: u32,
    /// Last child id (0 = none).
    pub last_child_id: u32,
    /// 1-based index into CONT (0 = none).
    pub content_index: u32,
    /// Dense, 0-based, document order.
    pub ordinal: u32,
    /// Depth (root = 0).
    pub depth: u32,
}

/// Extra kinds (SPEC.md §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExtraKind {
    /// `link_target`: `u16 url_len`, UTF-8 URL bytes.
    LinkTarget = 1,
    /// `cell_span`: `u16 colspan`, `u16 rowspan`.
    CellSpan = 2,
    /// `list_item_value`: `u32` explicit ordinal (0 = auto).
    ListItemValue = 3,
    /// `image_alt`: UTF-8 alt text.
    ImageAlt = 4,
}

impl ExtraKind {
    /// Interpret an extra kind code; `None` for out-of-range codes.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::LinkTarget),
            2 => Some(Self::CellSpan),
            3 => Some(Self::ListItemValue),
            4 => Some(Self::ImageAlt),
            _ => None,
        }
    }
}

/// The decoded data of a chunk extra (SPEC.md §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraData {
    /// `link_target`: the link URL.
    LinkTarget {
        /// The link URL.
        url: String,
    },
    /// `cell_span`: column and row spans (each ≥ 1).
    CellSpan {
        /// The column span.
        colspan: u16,
        /// The row span.
        rowspan: u16,
    },
    /// `list_item_value`: the explicit ordinal (0 = auto).
    ListItemValue {
        /// The explicit ordinal (0 = auto).
        value: u32,
    },
    /// `image_alt`: the alt text.
    ImageAlt {
        /// The alt text.
        text: String,
    },
}

/// A chunk extra (SPEC.md §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkExtra {
    /// The chunk the extra is attached to.
    pub chunk_id: u32,
    /// The extra kind.
    pub kind: ExtraKind,
    /// The decoded data.
    pub data: ExtraData,
}

/// The decoded CHNK section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSection {
    /// Chunk records in id order.
    pub chunks: Vec<ChunkRecord>,
    /// Extras in file order.
    pub extras: Vec<ChunkExtra>,
}

/// Decode the CHNK payload (SPEC.md §2.2).
pub fn decode(payload: &[u8]) -> Result<ChunkSection> {
    let mut c = Cursor::new(payload, 0, None);
    let chunk_count = c.u32("chunk count")?;
    if u64::from(chunk_count) > MAX_CHUNK_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("chunk count {chunk_count} > {MAX_CHUNK_COUNT}"),
        ));
    }
    let mut chunks = Vec::with_capacity(chunk_count as usize);
    for i in 0..chunk_count as usize {
        let rec = c.bytes(CHUNK_RECORD_LEN, "chunk record")?;
        let mut r = Cursor::new(rec, 0, None);
        let id = r.u32("chunk id")?;
        if id != i as u32 + 1 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("chunk {i}: id {id} != dense id {}", i + 1),
            ));
        }
        let kind_value = r.u8("chunk kind")?;
        let kind = ChunkKind::from_code(kind_value).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidValue,
                format!("chunk {i}: unknown kind {kind_value}"),
            )
        })?;
        let flags = r.u8("chunk flags")?;
        let reserved = r.u16("chunk reserved")?;
        let style_id = r.u32("chunk style_id")?;
        let parent_id = r.u32("chunk parent_id")?;
        let prev_id = r.u32("chunk prev_id")?;
        let next_id = r.u32("chunk next_id")?;
        let first_child_id = r.u32("chunk first_child_id")?;
        let last_child_id = r.u32("chunk last_child_id")?;
        let content_index = r.u32("chunk content_index")?;
        let ordinal = r.u32("chunk ordinal")?;
        let depth = r.u32("chunk depth")?;
        r.finish("chunk record")?;

        if reserved != 0 {
            return Err(Error::new(
                ErrorKind::InvalidFlags,
                format!("chunk {i}: reserved bits set"),
            ));
        }
        if flags & !flags::ALL != 0 {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("chunk {i}: unknown flag bits {flags:#04x}"),
            ));
        }
        if u64::from(depth) > MAX_CHUNK_DEPTH {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("chunk {i}: depth {depth} > {MAX_CHUNK_DEPTH}"),
            ));
        }
        if kind.is_structural() != ((flags & flags::STRUCTURAL) != 0) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                format!("chunk {i}: structural flag does not match kind {kind_value}"),
            ));
        }
        chunks.push(ChunkRecord {
            id,
            kind,
            flags,
            style_id,
            parent_id,
            prev_id,
            next_id,
            first_child_id,
            last_child_id,
            content_index,
            ordinal,
            depth,
        });
    }

    let extra_count = c.u32("extra count")?;
    if u64::from(extra_count) > MAX_EXTRA_COUNT {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("extra count {extra_count} > {MAX_EXTRA_COUNT}"),
        ));
    }
    let mut extras = Vec::with_capacity(extra_count as usize);
    for i in 0..extra_count as usize {
        let chunk_id = c.u32("extra chunk_id")?;
        let kind_value = c.u8("extra kind")?;
        let kind = ExtraKind::from_code(kind_value).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidValue,
                format!("extra {i}: unknown kind {kind_value}"),
            )
        })?;
        let extra_flags = c.u8("extra flags")?;
        let length = c.u16("extra length")?;
        if extra_flags != 0 {
            return Err(Error::new(
                ErrorKind::InvalidFlags,
                format!("extra {i}: reserved flags must be zero"),
            ));
        }
        let data = match kind {
            ExtraKind::LinkTarget => {
                let url_len = c.u16("link_target url_len")?;
                let url = c.utf8(url_len as usize, "link_target url")?;
                if url_len as usize + 2 != length as usize {
                    return Err(Error::new(
                        ErrorKind::InvalidValue,
                        format!("extra {i}: link_target length {length} != url_len + 2"),
                    ));
                }
                ExtraData::LinkTarget { url }
            }
            ExtraKind::CellSpan => {
                let colspan = c.u16("cell_span colspan")?;
                let rowspan = c.u16("cell_span rowspan")?;
                if colspan == 0 || rowspan == 0 {
                    return Err(Error::new(
                        ErrorKind::InvalidValue,
                        format!("extra {i}: cell_span colspan/rowspan must be ≥ 1"),
                    ));
                }
                if length != 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidValue,
                        format!("extra {i}: cell_span length {length} != 4"),
                    ));
                }
                ExtraData::CellSpan { colspan, rowspan }
            }
            ExtraKind::ListItemValue => {
                let value = c.u32("list_item_value value")?;
                if length != 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidValue,
                        format!("extra {i}: list_item_value length {length} != 4"),
                    ));
                }
                ExtraData::ListItemValue { value }
            }
            ExtraKind::ImageAlt => {
                let text = c.utf8(length as usize, "image_alt text")?;
                ExtraData::ImageAlt { text }
            }
        };
        extras.push(ChunkExtra {
            chunk_id,
            kind,
            data,
        });
    }
    c.finish("CHNK payload")?;
    Ok(ChunkSection { chunks, extras })
}
