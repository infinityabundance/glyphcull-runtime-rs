//! Typed reader errors (mirrors `glyphcull-runtime-js` `CullError`).
//!
//! Every failure of the package reader is an [`Error`] with a precise
//! [`ErrorKind`] discriminant and (where useful) the section table index the
//! failure is scoped to. The reader never panics on malformed input: every
//! path returns a `Result`, and the top-level parse entry point wraps any
//! internal defect as [`ErrorKind::Internal`] so hosts can always depend on
//! typed errors.

use std::fmt;

/// The discriminant of a reader failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The input is shorter than the header.
    TooShort,
    /// The magic bytes are not `CULL`.
    BadMagic,
    /// The format version is not supported.
    UnsupportedVersion,
    /// The header CRC-32 does not match.
    HeaderCrcMismatch,
    /// `section_count` exceeds the v1 cap.
    TooManySections,
    /// The section table does not fit in the input.
    Truncated,
    /// A section's `offset + stored_len` exceeds the file.
    OutOfBounds,
    /// Two sections share a kind.
    DuplicateSection,
    /// The compression code is not in `{0, 1}`.
    UnsupportedCompression,
    /// Reserved flags or reserved bits are set.
    InvalidFlags,
    /// A section's `decoded_len` exceeds the v1 cap.
    DecodedLenExceeded,
    /// A decoded stream's length differs from `decoded_len`.
    DecompressMismatch,
    /// The payload CRC-32 does not match the table entry.
    CrcMismatch,
    /// A text field is not valid UTF-8.
    InvalidUtf8,
    /// A structural value is invalid (out of range, unknown kind, trailing bytes).
    InvalidValue,
    /// Arithmetic overflow while validating untrusted offsets or lengths.
    Overflow,
    /// The zlib header bytes are invalid.
    ZlibHeaderInvalid,
    /// The zlib stream's trailing Adler-32 does not match the decoded output.
    ZlibAdlerMismatch,
    /// The SEAL hash tree does not verify.
    SealMismatch,
    /// The SEAL mode or algorithm is not supported.
    UnsupportedAlgorithm,
    /// A reader defect surfaced as a typed error (never a panic across the API).
    Internal,
}

/// A structured reader failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The failure discriminant.
    pub kind: ErrorKind,
    /// The section table index when the failure is section-scoped.
    pub section: Option<usize>,
    /// A precise human-readable detail.
    pub message: String,
}

impl Error {
    /// Construct an unscoped error.
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            section: None,
            message: message.into(),
        }
    }

    /// Construct a section-scoped error (SPEC.md §1.6: precise, per-entry).
    #[must_use]
    pub fn for_section(kind: ErrorKind, section: usize, message: impl Into<String>) -> Self {
        Self {
            kind,
            section: Some(section),
            message: format!("{} (section {section})", message.into()),
        }
    }

    /// The failure discriminant.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(section) = self.section {
            write!(f, "{:?} (section {section}): {}", self.kind, self.message)
        } else {
            write!(f, "{:?}: {}", self.kind, self.message)
        }
    }
}

impl std::error::Error for Error {}

/// The result of a reader operation: a value or a typed [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
