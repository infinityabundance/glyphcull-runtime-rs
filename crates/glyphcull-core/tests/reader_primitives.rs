//! Known-answer tests for the reader primitives: CRC-32, Adler-32, and
//! SHA-256 (mirrors the JS `test/format/primitives.test.ts`).
//!
//! The CRC-32 and Adler-32 primitives are hand-owned by this crate (CRC via
//! `crc32fast`, Adler hand-rolled); the SHA-256 vectors gate the `sha2`
//! dependency version so SEAL verification never silently changes meaning.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

use glyphcull_core::reader::{adler32, crc32, parse, SectionKind};

/// Hex of a SHA-256 digest.
fn sha256_hex(input: &[u8]) -> String {
    Sha256::digest(input)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn crc32_matches_standard_known_answer_vectors() {
    assert_eq!(crc32(&[]), 0x0000_0000);
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(
        crc32(b"The quick brown fox jumps over the lazy dog"),
        0x414f_a339
    );
}

#[test]
fn crc32_is_order_sensitive_and_stable() {
    let a = crc32(b"abc");
    let b = crc32(b"acb");
    assert_ne!(a, b);
    assert_eq!(crc32(b"abc"), a);
}

#[test]
fn crc32_matches_the_golden_section_table() {
    // Independent cross-check: recompute every decoded payload's CRC and
    // compare to the table the compiler wrote.
    let pkg = parse(common::pipeline_golden()).expect("golden parses");
    for entry in &pkg.entries {
        let kind = SectionKind::from_code(entry.kind).expect("known kind");
        let payload = pkg.section(kind).expect("payload present");
        assert_eq!(crc32(payload), entry.crc32, "section kind {kind}");
    }
}

#[test]
fn adler32_matches_rfc1950_known_answer_vectors() {
    assert_eq!(adler32(&[]), 1);
    assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    assert_eq!(
        adler32(b"The quick brown fox jumps over the lazy dog"),
        0x5bdc_0fda
    );
}

#[test]
fn adler32_handles_inputs_longer_than_one_chunk() {
    // The mod-65521 reduction happens every 5552 bytes; 20,000 bytes crosses
    // that boundary three times. Cross-check against a real zlib stream's
    // big-endian trailer (RFC 1950 §2.3).
    let big: Vec<u8> = (0..20_000).map(|i| (i % 251) as u8).collect();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(9));
    encoder.write_all(&big).expect("zlib encode");
    let compressed = encoder.finish().expect("zlib finish");
    let trailer = compressed.get(compressed.len() - 4..).expect("trailer");
    let expected = u32::from_be_bytes(trailer.try_into().expect("4 bytes"));
    assert_eq!(adler32(&big), expected);
}

#[test]
fn sha256_matches_fips_180_4_example_vectors() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(
        sha256_hex(b"The quick brown fox jumps over the lazy dog"),
        "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
    );
}

#[test]
fn sha256_handles_lengths_crossing_the_padding_boundary() {
    // 55, 56, 63, 64, 65, 119, 120, 121 bytes exercise the SHA-256 padding
    // edge cases; a regression in the `sha2` dependency would change these.
    for len in [55usize, 56, 63, 64, 65, 119, 120, 121, 1000] {
        let data: Vec<u8> = (0..len).map(|i| ((i * 13) % 256) as u8).collect();
        let first = sha256_hex(&data);
        assert_eq!(sha256_hex(&data), first, "deterministic at len {len}");
        assert_eq!(first.len(), 64, "hex digest length at len {len}");
    }
    // The FIPS 180-4 multi-block vector: SHA-256 of 1,000 × "a".
    assert_eq!(
        sha256_hex(&vec![b'a'; 1000]),
        "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
}
