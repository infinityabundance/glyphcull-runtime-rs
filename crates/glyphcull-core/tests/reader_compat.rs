//! Container compatibility rules (SPEC.md §1.6, §4): canonical known-section
//! order, the critical bit on unknown sections, and the required INFO section.
//! The JS reader suite mirrors these ("unknown sections and structural
//! strictness"); the compiler reference reader covers them in
//! `glyphcull-format`'s reader tests.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::error::ErrorKind;
use glyphcull_core::reader::parse;

/// An unknown section kind is skipped when noncritical and rejected when the
/// critical bit (flags bit 0) is set.
#[test]
fn unknown_sections_noncritical_skipped_critical_rejected() {
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
    let pkg = parse(&bytes).expect("noncritical unknown skipped");
    assert_eq!(pkg.unknown.len(), 1);
    assert_eq!(pkg.unknown[0].entry.kind, 99);

    // Set the critical bit on the second entry's flags byte
    // (table entry 1, offset 16 + 32 + 5).
    let mut critical = bytes;
    critical[16 + 32 + 5] = 0x01;
    let err = parse(&critical).expect_err("critical unknown rejected");
    assert_eq!(err.kind, ErrorKind::UnknownCriticalSection);
}

/// The known sections must appear in canonical relative order; an out-of-order
/// package is rejected even when every other layer is valid.
#[test]
fn out_of_order_known_sections_rejected() {
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 4, // CONT before INFO
            compression: 1,
            payload: b"c".to_vec(),
        },
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload(),
        },
    ]);
    let err = parse(&bytes).expect_err("out-of-order rejected");
    assert_eq!(err.kind, ErrorKind::InvalidSectionOrder);
}

/// INFO is the required section: a package without it is rejected by the
/// container reader (the document build additionally requires CHNK).
#[test]
fn missing_info_rejected_at_the_container() {
    let bytes = common::build_package(&[common::TestSection {
        kind: 4,
        compression: 1,
        payload: b"c".to_vec(),
    }]);
    let err = parse(&bytes).expect_err("missing INFO rejected");
    assert_eq!(err.kind, ErrorKind::MissingRequiredSection);
}
