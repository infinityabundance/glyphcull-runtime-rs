//! Property tests for the reader: arbitrary bytes must never panic — every
//! outcome is a successful parse or a typed, precise error — and parsing is
//! deterministic (mirrors the JS `test/format/property.test.ts`).

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use proptest::prelude::*;

use glyphcull_core::error::ErrorKind;
use glyphcull_core::reader::{parse, validate_structure};

/// Arbitrary byte buffers up to 4 KiB.
fn arbitrary_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..4096)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 500, ..ProptestConfig::default() })]

    /// `validate_structure` returns a typed result for any input.
    #[test]
    fn structure_never_panics(bytes in arbitrary_bytes()) {
        match validate_structure(&bytes) {
            Ok(structure) => {
                assert_eq!(structure.version, 1);
            }
            Err(err) => {
                // A precise variant, never an internal defect: untrusted input
                // must not surface reader bugs as `Internal`.
                assert_ne!(err.kind(), ErrorKind::Internal);
            }
        }
    }

    /// `parse` resolves, never panics, for any input.
    #[test]
    fn parse_never_panics(bytes in arbitrary_bytes()) {
        match parse(&bytes) {
            Ok(pkg) => {
                assert_eq!(pkg.version, 1);
            }
            Err(err) => {
                assert_ne!(err.kind(), ErrorKind::Internal);
            }
        }
    }

    /// Mutations of a valid golden package never panic untyped.
    #[test]
    fn golden_mutations_never_panic_untyped(
        position in 0..common::pipeline_golden().len(),
        value in any::<u8>(),
    ) {
        let mut mutated = common::pipeline_golden().to_vec();
        mutated[position] = value;
        if let Err(err) = parse(&mutated) {
            assert_ne!(err.kind(), ErrorKind::Internal);
        }
    }
}

/// Reading is deterministic: identical input yields identical structure.
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
}
