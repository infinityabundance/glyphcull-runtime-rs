//! SEAL section decoder and verifier (SPEC.md §2.7): the integrity hash tree.
//!
//! The SEAL section carries a per-section SHA-256 over every covered section's
//! decoded payload plus an `overall_hash` that binds header identity, section
//! kinds, decoded lengths, and content. The definition is deliberately
//! non-circular: the SEAL section does not cover itself, and the overall hash
//! covers header bytes `0..12` instead of the raw table (whose offsets depend
//! on the SEAL section's own size).
//!
//! [`decode`] validates the payload structure; [`verify`] recomputes both the
//! per-section hashes and the overall hash against the package's decoded
//! sections, mirroring the JS runtime's `verifySeal` exactly (mismatch ⇒
//! [`ErrorKind::SealMismatch`]).

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::error::{Error, ErrorKind, Result};
use crate::limits::MAX_COVERED_SECTIONS;
use crate::reader::{Cursor, Package, SectionKind, OVERALL_HASH_LEN};

/// The hash tree mode (SPEC.md §2.7).
pub const SEAL_MODE_HASH_TREE: u8 = 1;
/// The hash algorithm code for SHA-256 (SPEC.md §2.7).
pub const SEAL_ALGO_SHA256: u8 = 0;

/// One covered section's hash (SPEC.md §2.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionHash {
    /// The covered section kind.
    pub kind: u32,
    /// SHA-256 of the section's decoded payload.
    pub hash: [u8; OVERALL_HASH_LEN],
}

/// A decoded SEAL section (SPEC.md §2.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seal {
    /// The hash tree mode (must be [`SEAL_MODE_HASH_TREE`]).
    pub mode: u8,
    /// The hash algorithm (must be [`SEAL_ALGO_SHA256`]).
    pub algo: u8,
    /// Per-section hashes in file order.
    pub hashes: Vec<SectionHash>,
    /// The overall content hash.
    pub overall: [u8; OVERALL_HASH_LEN],
}

/// Decode and structurally validate the SEAL payload (SPEC.md §2.7).
pub fn decode(payload: &[u8]) -> Result<Seal> {
    let mut c = Cursor::new(payload, 0, None);
    let mode = c.u8("SEAL mode")?;
    let algo = c.u8("SEAL algo")?;
    let flags = c.u8("SEAL flags")?;
    let reserved = c.u8("SEAL reserved")?;
    let count = c.u32("SEAL count")?;
    if mode != SEAL_MODE_HASH_TREE {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("SEAL mode {mode} != 1 (hash tree)"),
        ));
    }
    if algo != SEAL_ALGO_SHA256 {
        return Err(Error::new(
            ErrorKind::UnsupportedAlgorithm,
            format!("SEAL algo {algo} != 0 (SHA-256)"),
        ));
    }
    if flags != 0 || reserved != 0 {
        return Err(Error::new(
            ErrorKind::InvalidFlags,
            "SEAL flags/reserved bits must be zero",
        ));
    }
    if u64::from(count) > MAX_COVERED_SECTIONS {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            format!("SEAL count {count} > {MAX_COVERED_SECTIONS}"),
        ));
    }
    let mut hashes = Vec::with_capacity(count as usize);
    for _ in 0..count as usize {
        let kind = c.u32("SEAL section kind")?;
        let hash_bytes = c.bytes(OVERALL_HASH_LEN, "SEAL digest")?;
        let hash = hash_bytes
            .try_into()
            .map_err(|_| Error::new(ErrorKind::Internal, "SEAL digest has the wrong length"))?;
        hashes.push(SectionHash { kind, hash });
    }
    let overall_bytes = c.bytes(OVERALL_HASH_LEN, "SEAL overall")?;
    let overall = overall_bytes.try_into().map_err(|_| {
        Error::new(
            ErrorKind::Internal,
            "SEAL overall hash has the wrong length",
        )
    })?;
    c.finish("SEAL payload")?;
    Ok(Seal {
        mode,
        algo,
        hashes,
        overall,
    })
}

/// Verify the SEAL hash tree against the package (SPEC.md §2.7).
///
/// The covered sections are every known non-SEAL section, exactly as in the JS
/// runtime: per-section hashes must match the decoded payloads (one entry per
/// covered section, each entry's kind covered), and the overall hash is
/// recomputed over header bytes `0..12` then every covered section in
/// canonical kind order as `u32 kind` (LE) + `u32 decoded_len` (LE) + decoded
/// payload.
pub fn verify(package: &Package, seal: &Seal) -> Result<()> {
    let covered: Vec<&crate::reader::SectionPayload> = package
        .sections
        .iter()
        .filter(|s| s.entry.kind != SectionKind::Seal as u32)
        .collect();

    // Per-section hashes: recompute SHA-256 over each decoded payload.
    let mut expected: HashMap<u32, [u8; OVERALL_HASH_LEN]> = HashMap::new();
    for section in &covered {
        let digest = Sha256::digest(&section.bytes);
        let mut hash = [0_u8; OVERALL_HASH_LEN];
        hash.copy_from_slice(&digest);
        expected.insert(section.entry.kind, hash);
    }
    if seal.hashes.len() != covered.len() {
        return Err(Error::new(
            ErrorKind::SealMismatch,
            format!(
                "SEAL covers {} sections, package has {}",
                seal.hashes.len(),
                covered.len()
            ),
        ));
    }
    for entry in &seal.hashes {
        let actual = expected.get(&entry.kind).ok_or_else(|| {
            Error::new(
                ErrorKind::SealMismatch,
                format!("SEAL covers kind {} which the package lacks", entry.kind),
            )
        })?;
        if actual.as_slice() != entry.hash {
            return Err(Error::new(
                ErrorKind::SealMismatch,
                format!("SEAL section hash mismatch for kind {}", entry.kind),
            ));
        }
    }

    // Overall hash: header bytes 0..12, then every covered section in
    // canonical kind order (SPEC.md §2.7).
    let mut canonical: Vec<&crate::reader::SectionPayload> = covered;
    canonical.sort_by_key(|s| s.entry.kind);
    let mut hasher = Sha256::new();
    hasher.update(package.header_prefix);
    for section in &canonical {
        hasher.update(section.entry.kind.to_le_bytes());
        let decoded_len = u32::try_from(section.bytes.len())
            .map_err(|_| Error::new(ErrorKind::Internal, "decoded payload exceeds u32 length"))?;
        hasher.update(decoded_len.to_le_bytes());
        hasher.update(&section.bytes);
    }
    let overall: [u8; OVERALL_HASH_LEN] = hasher.finalize().into();
    if overall != seal.overall {
        return Err(Error::new(
            ErrorKind::SealMismatch,
            "SEAL overall hash mismatch",
        ));
    }
    Ok(())
}
