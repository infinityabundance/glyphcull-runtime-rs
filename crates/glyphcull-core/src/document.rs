//! The Document model — the trusted runtime view of a `.cull` package
//! (Phase 4.2; mirrors the JS runtime's `src/document/model.ts`).
//!
//! [`build_document`] validates the package's semantic structure at load —
//! chunk-graph invariants (SPEC.md §2.2), style/content/image reference
//! resolution, and the INFO count cross-checks — then hands back a
//! [`DocumentModel`] that layout, visibility, materialization, and selection
//! treat as trusted. **No geometry lives here**: geometry is produced by
//! materialization and owned by layout structures (Architecture.md §3.2).
//!
//! Models are self-contained views — no global state — so any number of
//! Documents coexist (multi-document isolation). The model borrows the
//! [`Package`] it was built from (the reader's decoded payloads), so building
//! copies only the derived data: the resolved style table.

use std::collections::BTreeMap;
use std::fmt;

use crate::reader::chunk::{ChunkExtra, ChunkKind, ChunkRecord};
use crate::reader::content::{ContentData, ContentPayload, PayloadKind};
use crate::reader::glyph::Atlas;
use crate::reader::image::ImageRecord;
use crate::reader::info::Info;
use crate::reader::style::{PropertyTag, PropertyValue, StyleProperty, StyleRecord};
use crate::reader::Package;

/// A load-time document validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentError {
    /// The failure discriminant.
    pub kind: DocumentErrorKind,
    /// A precise human-readable detail.
    pub message: String,
}

/// The discriminant of a document build failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentErrorKind {
    /// A required section (INFO, CHNK) is absent.
    MissingSection,
    /// The chunk graph violates a structural invariant (SPEC.md §2.2).
    InvalidChunkGraph,
    /// A chunk references a style, content payload, or image that does not
    /// exist.
    DanglingReference,
    /// An INFO count disagrees with the decoded section.
    CountMismatch,
    /// A chunk's content is of the wrong kind for its chunk kind.
    InvalidContent,
}

impl DocumentError {
    /// Construct a document error.
    #[must_use]
    pub fn new(kind: DocumentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for DocumentError {}

/// The result of a document build.
pub type DocumentResult<'a> = std::result::Result<DocumentModel<'a>, DocumentError>;

/// Chunk-kind classification (SPEC.md §2.2, mirroring the reference): the
/// structural wrappers `document`, `list`, `table`, `table_row`.
#[must_use]
pub fn is_structural_kind(kind: ChunkKind) -> bool {
    kind.is_structural()
}

/// True for inline kinds (nested inside block chunks): `run`, `link`, `br`.
#[must_use]
pub fn is_inline_kind(kind: ChunkKind) -> bool {
    matches!(kind, ChunkKind::Run | ChunkKind::Link | ChunkKind::Br)
}

/// True for block-level renderable kinds (SPEC.md §2.2).
#[must_use]
pub fn is_block_kind(kind: ChunkKind) -> bool {
    matches!(
        kind,
        ChunkKind::Heading1
            | ChunkKind::Heading2
            | ChunkKind::Heading3
            | ChunkKind::Heading4
            | ChunkKind::Heading5
            | ChunkKind::Heading6
            | ChunkKind::Paragraph
            | ChunkKind::Quote
            | ChunkKind::ListItem
            | ChunkKind::CodeBlock
            | ChunkKind::TableCell
            | ChunkKind::Image
            | ChunkKind::Caption
            | ChunkKind::Hr
    )
}

/// A fully resolved style: absent properties take the SPEC §2.3 defaults.
///
/// The enum-like `u8` values (`text_align`, `list_style`, `white_space`) are
/// carried as raw bytes exactly like the JS runtime carries its `const enum`
/// numbers — value semantics are interpreted by the consumers (layout) with
/// the SPEC's tables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedStyle {
    /// Index into the GLYF atlas table.
    pub font_id: u32,
    /// Font size in px.
    pub font_size_px: f32,
    /// Line height as a multiplier of the font size.
    pub line_height: f32,
    /// Font weight (100..=900).
    pub font_weight: u16,
    /// Whether the face is italic.
    pub italic: bool,
    /// Text color as u32 RGBA.
    pub color: u32,
    /// Background color as u32 RGBA.
    pub background_color: u32,
    /// Top margin in px.
    pub margin_top: f32,
    /// Bottom margin in px.
    pub margin_bottom: f32,
    /// Text alignment: 0 start, 1 center, 2 end, 3 justify.
    pub text_align: u8,
    /// First-line indent in px.
    pub text_indent: f32,
    /// List marker style: 0 none … 8 upper_roman.
    pub list_style: u8,
    /// Whether the style is monospace/code.
    pub code: bool,
    /// Whether the style underlines.
    pub underline: bool,
    /// Letter spacing in px.
    pub letter_spacing: f32,
    /// White-space handling: 0 normal, 1 pre, 2 nowrap.
    pub white_space: u8,
}

impl Default for ResolvedStyle {
    /// The SPEC §2.3 defaults (mirrors the JS `STYLE_DEFAULTS`).
    fn default() -> Self {
        Self {
            font_id: 0,
            font_size_px: 16.0,
            line_height: 1.5,
            font_weight: 400,
            italic: false,
            color: 0x0000_00ff,
            background_color: 0x0000_0000,
            margin_top: 0.0,
            margin_bottom: 0.0,
            text_align: 0,
            text_indent: 0.0,
            list_style: 0,
            code: false,
            underline: false,
            letter_spacing: 0.0,
            white_space: 0,
        }
    }
}

/// Resolve a style record against the SPEC §2.3 defaults.
#[must_use]
pub fn resolve_style(record: &StyleRecord) -> ResolvedStyle {
    let mut out = ResolvedStyle::default();
    for property in &record.properties {
        match property.tag {
            PropertyTag::FontId => out.font_id = u32_of(property),
            PropertyTag::FontSizePx => out.font_size_px = f32_of(property),
            PropertyTag::LineHeight => out.line_height = f32_of(property),
            PropertyTag::FontWeight => out.font_weight = u16_of(property),
            PropertyTag::Italic => out.italic = bool_of(property),
            PropertyTag::Color => out.color = u32_of(property),
            PropertyTag::BackgroundColor => out.background_color = u32_of(property),
            PropertyTag::MarginTop => out.margin_top = f32_of(property),
            PropertyTag::MarginBottom => out.margin_bottom = f32_of(property),
            PropertyTag::TextAlign => out.text_align = u8_of(property),
            PropertyTag::TextIndent => out.text_indent = f32_of(property),
            PropertyTag::ListStyle => out.list_style = u8_of(property),
            PropertyTag::Code => out.code = bool_of(property),
            PropertyTag::Underline => out.underline = bool_of(property),
            PropertyTag::LetterSpacing => out.letter_spacing = f32_of(property),
            PropertyTag::WhiteSpace => out.white_space = u8_of(property),
        }
    }
    out
}

// The reader decodes each tag with its fixed SPEC width, so the `PropertyValue`
// variant below always matches its tag; the fallbacks are unreachable and
// exist only to keep the document layer panic-free on impossible input.
fn u32_of(property: &StyleProperty) -> u32 {
    match property.value {
        PropertyValue::U32(v) => v,
        _ => 0,
    }
}

fn f32_of(property: &StyleProperty) -> f32 {
    match property.value {
        PropertyValue::F32(v) => v,
        _ => 0.0,
    }
}

fn u16_of(property: &StyleProperty) -> u16 {
    match property.value {
        PropertyValue::U16(v) => v,
        _ => 0,
    }
}

fn u8_of(property: &StyleProperty) -> u8 {
    match property.value {
        PropertyValue::U8(v) => v,
        _ => 0,
    }
}

fn bool_of(property: &StyleProperty) -> bool {
    u8_of(property) != 0
}

/// The trusted runtime model of a document (mirrors the JS `DocumentModel`).
///
/// All document-order traversals are pre-order walks over the chunk tree.
#[derive(Debug)]
pub struct DocumentModel<'a> {
    package: &'a Package,
    info: Info,
    chunks: &'a [ChunkRecord],
    extras: &'a [ChunkExtra],
    extras_by_chunk: BTreeMap<u32, Vec<usize>>,
    styles: Vec<ResolvedStyle>,
    content: &'a [ContentPayload],
    atlases: &'a [Atlas],
    images: &'a [ImageRecord],
    root: &'a ChunkRecord,
}

impl<'a> DocumentModel<'a> {
    /// The validated package this model was built from.
    #[must_use]
    pub const fn package(&self) -> &'a Package {
        self.package
    }

    /// The validated INFO metadata.
    #[must_use]
    pub const fn info(&self) -> &Info {
        &self.info
    }

    /// Chunk records indexed by id (`chunks[i]` has id `i + 1`).
    #[must_use]
    pub const fn chunks(&self) -> &'a [ChunkRecord] {
        self.chunks
    }

    /// Resolved styles indexed by style id (defaults applied).
    #[must_use]
    pub fn styles(&self) -> &[ResolvedStyle] {
        &self.styles
    }

    /// Content payloads indexed by payload id.
    #[must_use]
    pub const fn content(&self) -> &'a [ContentPayload] {
        self.content
    }

    /// Atlases indexed by font id.
    #[must_use]
    pub const fn atlases(&self) -> &'a [Atlas] {
        self.atlases
    }

    /// Images indexed by image id.
    #[must_use]
    pub const fn images(&self) -> &'a [ImageRecord] {
        self.images
    }

    /// The document root chunk.
    #[must_use]
    pub const fn root(&self) -> &'a ChunkRecord {
        self.root
    }

    /// The chunk with the given id, or `None`.
    #[must_use]
    pub fn chunk(&self, id: u32) -> Option<&'a ChunkRecord> {
        if id < 1 || id > self.chunks.len() as u32 {
            return None;
        }
        // Validation guarantees dense ids, so `chunks[id - 1].id == id`.
        self.chunks.get((id - 1) as usize)
    }

    /// The child ids of a chunk, in document order (empty for leaves).
    #[must_use]
    pub fn child_ids(&self, id: u32) -> Vec<u32> {
        let Some(chunk) = self.chunk(id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut child = chunk.first_child_id;
        while child != 0 {
            out.push(child);
            if child == chunk.last_child_id {
                break;
            }
            let Some(next) = self.chunk(child) else {
                break;
            };
            child = next.next_id;
        }
        out
    }

    /// All chunk ids in document order (a pre-order walk from the root).
    #[must_use]
    pub fn all_ids(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.chunks.len());
        let mut stack = vec![self.root.id];
        while let Some(id) = stack.pop() {
            out.push(id);
            let children = self.child_ids(id);
            for child in children.iter().rev() {
                stack.push(*child);
            }
        }
        out
    }

    /// The extras attached to a chunk, in file order (empty when none).
    #[must_use]
    pub fn extras_for(&self, id: u32) -> Vec<&'a ChunkExtra> {
        match self.extras_by_chunk.get(&id) {
            Some(indices) => indices.iter().filter_map(|&i| self.extras.get(i)).collect(),
            None => Vec::new(),
        }
    }

    /// The text content of a single chunk's direct payload (no traversal).
    #[must_use]
    pub fn direct_text(&self, id: u32) -> Option<&'a str> {
        let chunk = self.chunk(id)?;
        if chunk.content_index == 0 {
            return None;
        }
        let payload = self.content.get((chunk.content_index - 1) as usize)?;
        match &payload.data {
            ContentData::Text(text) => Some(text.as_str()),
            ContentData::ImageRef(_) => None,
        }
    }

    /// The image id referenced by an image chunk's payload.
    #[must_use]
    pub fn image_ref(&self, id: u32) -> Option<u32> {
        let chunk = self.chunk(id)?;
        if chunk.content_index == 0 {
            return None;
        }
        let payload = self.content.get((chunk.content_index - 1) as usize)?;
        match payload.data {
            ContentData::ImageRef(image_id) => Some(image_id),
            ContentData::Text(_) => None,
        }
    }

    /// The plain text of a subtree in document order (used by selection/copy):
    /// block chunks concatenate their descendants; code blocks contribute
    /// their direct text; `br` chunks contribute a newline.
    ///
    /// Iterative (explicit stack) so arbitrarily deep documents cannot
    /// overflow the native stack; the output order matches the JS
    /// runtime's recursive `plainText` exactly.
    #[must_use]
    pub fn plain_text(&self, id: u32) -> String {
        let mut out = String::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let Some(chunk) = self.chunk(current) else {
                continue;
            };
            if chunk.kind == ChunkKind::Br {
                out.push('\n');
                continue;
            }
            if let Some(text) = self.direct_text(current) {
                out.push_str(text);
            }
            let children = self.child_ids(current);
            for child in children.iter().rev() {
                stack.push(*child);
            }
        }
        out
    }
}

/// Build and validate a [`DocumentModel`] from a parsed package.
///
/// Validation order mirrors the JS runtime exactly: required sections, then
/// the chunk-graph invariants (using the INFO counts), then the five INFO
/// count cross-checks.
pub fn build_document(package: &Package) -> DocumentResult<'_> {
    let info = package
        .info()
        .map_err(build_failed)?
        .ok_or_else(|| {
            DocumentError::new(
                DocumentErrorKind::MissingSection,
                "package has no INFO section",
            )
        })?
        .clone();
    let chunk_section = package.chunks().map_err(build_failed)?.ok_or_else(|| {
        DocumentError::new(
            DocumentErrorKind::MissingSection,
            "package has no CHNK section",
        )
    })?;
    let chunks: &[ChunkRecord] = &chunk_section.chunks;
    let extras: &[ChunkExtra] = &chunk_section.extras;

    validate_chunk_graph(chunks, package, &info)?;

    // Resolved styles (STYL optional: empty table → all defaults).
    let style_records: &[StyleRecord] = match package.styles().map_err(build_failed)? {
        Some(records) => records,
        None => &[],
    };
    let styles: Vec<ResolvedStyle> = style_records.iter().map(resolve_style).collect();

    // Content payloads (CONT optional), atlases (GLYF optional), images (IMGS
    // optional).
    let content: &[ContentPayload] = match package.content().map_err(build_failed)? {
        Some(payloads) => payloads,
        None => &[],
    };
    let atlases: &[Atlas] = match package.atlases().map_err(build_failed)? {
        Some(atlas_list) => atlas_list,
        None => &[],
    };
    let images: &[ImageRecord] = match package.images().map_err(build_failed)? {
        Some(image_list) => image_list,
        None => &[],
    };

    // Cross-check the INFO counts against the decoded sections.
    if info.chunk_count as usize != chunks.len() {
        return Err(DocumentError::new(
            DocumentErrorKind::CountMismatch,
            format!(
                "INFO chunk_count {} != CHNK records {}",
                info.chunk_count,
                chunks.len()
            ),
        ));
    }
    if info.style_count as usize != styles.len() {
        return Err(DocumentError::new(
            DocumentErrorKind::CountMismatch,
            format!(
                "INFO style_count {} != STYL records {}",
                info.style_count,
                styles.len()
            ),
        ));
    }
    if info.content_count as usize != content.len() {
        return Err(DocumentError::new(
            DocumentErrorKind::CountMismatch,
            format!(
                "INFO content_count {} != CONT payloads {}",
                info.content_count,
                content.len()
            ),
        ));
    }
    if info.atlas_count as usize != atlases.len() {
        return Err(DocumentError::new(
            DocumentErrorKind::CountMismatch,
            format!(
                "INFO atlas_count {} != GLYF atlases {}",
                info.atlas_count,
                atlases.len()
            ),
        ));
    }
    if info.image_count as usize != images.len() {
        return Err(DocumentError::new(
            DocumentErrorKind::CountMismatch,
            format!(
                "INFO image_count {} != IMGS images {}",
                info.image_count,
                images.len()
            ),
        ));
    }

    let mut extras_by_chunk: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, extra) in extras.iter().enumerate() {
        extras_by_chunk
            .entry(extra.chunk_id)
            .or_default()
            .push(index);
    }

    let root = chunks.first().ok_or_else(|| {
        DocumentError::new(DocumentErrorKind::InvalidChunkGraph, "chunk graph is empty")
    })?;

    Ok(DocumentModel {
        package,
        info,
        chunks,
        extras,
        extras_by_chunk,
        styles,
        content,
        atlases,
        images,
        root,
    })
}

/// Wrap a reader-layer decode failure the way the JS runtime wraps any
/// non-`DocumentError` thrown during `buildDocument`.
fn build_failed<E: fmt::Display>(error: E) -> DocumentError {
    DocumentError::new(
        DocumentErrorKind::InvalidChunkGraph,
        format!("document build failed: {error}"),
    )
}

/// Validate the chunk-graph invariants (SPEC.md §2.2) and resolve every
/// reference, mirroring the JS `validateChunkGraph` check-for-check.
fn validate_chunk_graph(
    chunks: &[ChunkRecord],
    package: &Package,
    info: &Info,
) -> Result<(), DocumentError> {
    if chunks.is_empty() {
        return Err(DocumentError::new(
            DocumentErrorKind::InvalidChunkGraph,
            "chunk graph is empty",
        ));
    }
    let mut by_id: BTreeMap<u32, &ChunkRecord> = BTreeMap::new();
    for chunk in chunks {
        if chunk.id != chunk.ordinal + 1 {
            return Err(DocumentError::new(
                DocumentErrorKind::InvalidChunkGraph,
                format!(
                    "chunk {}: id does not match ordinal {}",
                    chunk.id, chunk.ordinal
                ),
            ));
        }
        if by_id.insert(chunk.id, chunk).is_some() {
            return Err(DocumentError::new(
                DocumentErrorKind::InvalidChunkGraph,
                format!("duplicate chunk id {}", chunk.id),
            ));
        }
    }

    let root = by_id.get(&1).ok_or_else(|| {
        DocumentError::new(
            DocumentErrorKind::InvalidChunkGraph,
            "chunk 1 must be the document root",
        )
    })?;
    if root.kind != ChunkKind::Document {
        return Err(DocumentError::new(
            DocumentErrorKind::InvalidChunkGraph,
            "chunk 1 must be the document root",
        ));
    }
    if root.depth != 0 || root.parent_id != 0 {
        return Err(DocumentError::new(
            DocumentErrorKind::InvalidChunkGraph,
            "root must have depth 0 and parent 0",
        ));
    }

    // Reachability + depth + ring consistency from the root (iterative).
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![root.id];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            return Err(DocumentError::new(
                DocumentErrorKind::InvalidChunkGraph,
                format!("cycle or duplicate visit at chunk {id}"),
            ));
        }
        let chunk = match by_id.get(&id).copied() {
            // Unreachable: every id pushed onto the stack was validated
            // present during the ring walk. Kept typed so the model never
            // panics on impossible input.
            Some(chunk) => chunk,
            None => {
                return Err(DocumentError::new(
                    DocumentErrorKind::InvalidChunkGraph,
                    format!("chunk {id} missing"),
                ));
            }
        };
        if chunk.id != 1 {
            let parent_depth = by_id
                .get(&chunk.parent_id)
                .map_or(-1_i64, |p| i64::from(p.depth));
            if i64::from(chunk.depth) != parent_depth + 1 {
                return Err(DocumentError::new(
                    DocumentErrorKind::InvalidChunkGraph,
                    format!(
                        "chunk {}: depth {} != parent.depth + 1",
                        chunk.id, chunk.depth
                    ),
                ));
            }
        }
        // Ring consistency: first → next-chain → last in exactly child_count
        // steps (SPEC.md §2.2 sibling rings).
        let mut child = chunk.first_child_id;
        let mut count = 0_usize;
        let mut last_seen = 0_u32;
        while child != 0 {
            if seen.contains(&child) {
                return Err(DocumentError::new(
                    DocumentErrorKind::InvalidChunkGraph,
                    format!("child {child} visited twice"),
                ));
            }
            let Some(child_chunk) = by_id.get(&child).copied() else {
                return Err(DocumentError::new(
                    DocumentErrorKind::DanglingReference,
                    format!("chunk {}: child {child} missing", chunk.id),
                ));
            };
            if child_chunk.parent_id != chunk.id {
                return Err(DocumentError::new(
                    DocumentErrorKind::InvalidChunkGraph,
                    format!(
                        "chunk {child}: parent {} != {}",
                        child_chunk.parent_id, chunk.id
                    ),
                ));
            }
            if count > chunks.len() {
                return Err(DocumentError::new(
                    DocumentErrorKind::InvalidChunkGraph,
                    format!("sibling ring does not terminate at {}", chunk.id),
                ));
            }
            last_seen = child;
            stack.push(child);
            count += 1;
            if child == chunk.last_child_id {
                break;
            }
            child = child_chunk.next_id;
        }
        if count > 0 && last_seen != chunk.last_child_id {
            return Err(DocumentError::new(
                DocumentErrorKind::InvalidChunkGraph,
                format!(
                    "chunk {}: next-chain from first_child does not reach last_child",
                    chunk.id
                ),
            ));
        }
        if count == 0 && chunk.last_child_id != 0 {
            return Err(DocumentError::new(
                DocumentErrorKind::InvalidChunkGraph,
                format!("chunk {}: last_child set but no first_child", chunk.id),
            ));
        }
    }
    if seen.len() != chunks.len() {
        return Err(DocumentError::new(
            DocumentErrorKind::InvalidChunkGraph,
            format!(
                "chunk graph has {} records but only {} reachable from the root",
                chunks.len(),
                seen.len()
            ),
        ));
    }

    // Reference resolution: style ids and content indices.
    let style_records: &[StyleRecord] = match package.styles().map_err(build_failed)? {
        Some(records) => records,
        None => &[],
    };
    let content_payloads: &[ContentPayload] = match package.content().map_err(build_failed)? {
        Some(payloads) => payloads,
        None => &[],
    };
    let images: &[ImageRecord] = match package.images().map_err(build_failed)? {
        Some(image_list) => image_list,
        None => &[],
    };
    let atlases: &[Atlas] = match package.atlases().map_err(build_failed)? {
        Some(atlas_list) => atlas_list,
        None => &[],
    };
    let atlas_count = atlases.len();
    let style_count = info.style_count;
    let content_count = info.content_count;

    for chunk in chunks {
        if chunk.style_id >= style_count {
            return Err(DocumentError::new(
                DocumentErrorKind::DanglingReference,
                format!(
                    "chunk {}: style_id {} out of range ({} styles)",
                    chunk.id, chunk.style_id, style_count
                ),
            ));
        }
        if chunk.content_index != 0 {
            if chunk.content_index > content_count {
                return Err(DocumentError::new(
                    DocumentErrorKind::DanglingReference,
                    format!(
                        "chunk {}: content_index {} out of range ({} payloads)",
                        chunk.id, chunk.content_index, content_count
                    ),
                ));
            }
            let payload = content_payloads.get((chunk.content_index - 1) as usize);
            if chunk.kind == ChunkKind::Image {
                let Some(payload) = payload else {
                    return Err(DocumentError::new(
                        DocumentErrorKind::InvalidContent,
                        format!(
                            "chunk {}: image chunk must reference an image_ref payload",
                            chunk.id
                        ),
                    ));
                };
                if payload.kind != PayloadKind::ImageRef {
                    return Err(DocumentError::new(
                        DocumentErrorKind::InvalidContent,
                        format!(
                            "chunk {}: image chunk must reference an image_ref payload",
                            chunk.id
                        ),
                    ));
                }
                let image_id = match &payload.data {
                    ContentData::ImageRef(image_id) => *image_id,
                    // The kind check above makes this arm unreachable; a
                    // typed error keeps the model panic-free on impossible
                    // input.
                    ContentData::Text(_) => {
                        return Err(DocumentError::new(
                            DocumentErrorKind::InvalidContent,
                            format!(
                                "chunk {}: image chunk must reference an image_ref payload",
                                chunk.id
                            ),
                        ));
                    }
                };
                if image_id as usize >= images.len() {
                    return Err(DocumentError::new(
                        DocumentErrorKind::DanglingReference,
                        format!("chunk {}: image ref {image_id} out of range", chunk.id),
                    ));
                }
            } else if let Some(payload) = payload {
                if payload.kind != PayloadKind::TextUtf8 {
                    return Err(DocumentError::new(
                        DocumentErrorKind::InvalidContent,
                        format!(
                            "chunk {}: non-image chunk must reference a text payload",
                            chunk.id
                        ),
                    ));
                }
            }
        }
        // Style font_id must resolve against the atlas table when an atlas
        // exists (and against the empty table when GLYF is absent).
        let style = match style_records.get(chunk.style_id as usize) {
            Some(record) => resolve_style(record),
            None => resolve_style(&StyleRecord {
                id: chunk.style_id,
                properties: Vec::new(),
            }),
        };
        if style.font_id >= atlas_count.max(1) as u32 {
            return Err(DocumentError::new(
                DocumentErrorKind::DanglingReference,
                format!(
                    "chunk {}: style font_id {} out of range ({} atlases)",
                    chunk.id, style.font_id, atlas_count
                ),
            ));
        }
    }
    Ok(())
}
