//! Knuth–Plass line breaking tests — mirrors the JS
//! `test/layout/kp.test.ts` vector for vector (including its hand-built item
//! lists and the exact expected break indices).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use glyphcull_core::layout::kp::{line_break, KpItem};

/// Build items for a whitespace-separated sentence with equal word widths
/// (mirrors the JS `words` helper).
fn words(count: usize, word_width: f64, space_width: f64) -> Vec<KpItem> {
    let mut items = Vec::new();
    for i in 0..count {
        items.push(KpItem::Box { width: word_width });
        if i < count - 1 {
            items.push(KpItem::Glue {
                width: space_width,
                stretch: 10.0,
                shrink: 5.0,
            });
        }
    }
    // Terminating forced break (the KP contract).
    items.push(KpItem::Penalty {
        width: 0.0,
        penalty: f64::NEG_INFINITY,
    });
    items
}

#[test]
fn returns_one_line_when_everything_fits() {
    let lines = line_break(&words(3, 100.0, 20.0), 400.0, 100.0, 10.0);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].end, 5); // boxes+glues then the forced break
}

#[test]
fn wraps_when_the_content_exceeds_the_line_width() {
    // 5 words × 100 + 4 spaces × 20 = 580 > 200 → multiple lines.
    let lines = line_break(&words(5, 100.0, 20.0), 200.0, 100.0, 10.0);
    assert!(lines.len() > 1);
    // Lines cover the whole item range contiguously.
    for i in 1..lines.len() {
        assert_eq!(lines[i].start, lines[i - 1].end + 1);
    }
    // 5 words → boxes 0,2,4,6,8 + forced break at 9.
    assert_eq!(lines[lines.len() - 1].end, 9);
}

#[test]
fn never_breaks_inside_a_word_boxes_are_unbreakable() {
    // Two huge boxes: the second cannot fit; it overflows on its own line.
    let items = vec![
        KpItem::Box { width: 100.0 },
        KpItem::Glue {
            width: 20.0,
            stretch: 10.0,
            shrink: 5.0,
        },
        KpItem::Box { width: 500.0 },
        KpItem::Penalty {
            width: 0.0,
            penalty: f64::NEG_INFINITY,
        },
    ];
    let lines = line_break(&items, 200.0, 100.0, 10.0);
    assert_eq!(lines.len(), 2);
    // The 500-wide box (index 2) is whole on the last line (overflowing).
    let last = &lines[lines.len() - 1];
    assert!(last.start <= 2);
    assert_eq!(last.end, 3);
    // No box is ever split: line boundaries fall on glue/penalty indices.
    for line in &lines {
        if line.end < 3 {
            assert!([1usize, 3].contains(&line.end), "end {}", line.end);
        }
    }
}

#[test]
fn honors_forbidden_and_forced_penalties() {
    // a b ! c with a forbidden break after '!' (penalty +inf at index 5):
    // the only feasible 2-line split is before '!'.
    let items = vec![
        KpItem::Box { width: 100.0 }, // a
        KpItem::Glue {
            width: 20.0,
            stretch: 10.0,
            shrink: 5.0,
        },
        KpItem::Box { width: 100.0 }, // b
        KpItem::Glue {
            width: 20.0,
            stretch: 10.0,
            shrink: 5.0,
        },
        KpItem::Box { width: 100.0 }, // !
        KpItem::Penalty {
            width: 0.0,
            penalty: f64::INFINITY,
        }, // no break after !
        KpItem::Box { width: 100.0 }, // c
        KpItem::Penalty {
            width: 0.0,
            penalty: f64::NEG_INFINITY,
        },
    ];
    let lines = line_break(&items, 210.0, 100.0, 10.0);
    assert_eq!(lines.len(), 2);
    for line in &lines {
        // A break at the forbidden penalty (index 5) must never be chosen.
        assert_ne!(line.end, 5);
    }
    // '!' and 'c' stay together on the last line.
    let last = &lines[lines.len() - 1];
    assert!(last.start <= 4);
    assert_eq!(last.end, 7);
}

#[test]
fn justified_lines_keep_the_ratio_within_the_tolerance() {
    let lines = line_break(&words(6, 100.0, 20.0), 350.0, 100.0, 10.0);
    for line in &lines {
        assert!(line.ratio >= -1.0, "ratio {}", line.ratio);
        assert!(line.ratio <= 100.0, "ratio {}", line.ratio);
        assert!(line.badness >= 0.0);
    }
}

#[test]
fn fitness_classes_are_in_range() {
    let lines = line_break(&words(20, 60.0, 15.0), 300.0, 100.0, 10.0);
    for line in &lines {
        assert!((0..=3).contains(&line.fitness), "fitness {}", line.fitness);
    }
}

#[test]
fn empty_input_produces_no_lines() {
    let lines = line_break(&[], 100.0, 100.0, 10.0);
    assert!(lines.is_empty());
}

#[test]
fn is_deterministic() {
    let items = words(30, 55.0, 12.0);
    let a = line_break(&items, 220.0, 100.0, 10.0);
    let b = line_break(&items, 220.0, 100.0, 10.0);
    assert_eq!(a, b);
}

#[test]
fn prefers_fewer_better_lines_over_greedy_wrapping() {
    // A classic case where KP differs from greedy: 9 words, width fits 3.5
    // words. Greedy would produce 3 words/line; KP balances demerits.
    let lines = line_break(&words(9, 100.0, 20.0), 320.0, 100.0, 10.0);
    assert!(lines.len() >= 3);
    for line in &lines {
        assert!(line.ratio >= -1.0);
    }
}

/// Every chosen line's accumulated demerits are non-decreasing across the
/// paragraph (a global invariant of the dynamic program's forward pass).
#[test]
fn demerits_are_monotonic() {
    let lines = line_break(&words(12, 80.0, 18.0), 260.0, 100.0, 10.0);
    let mut prev = 0.0_f64;
    for line in &lines {
        assert!(line.demerits >= prev, "demerits must not decrease");
        prev = line.demerits;
    }
}

/// A line's recorded ratio is consistent with its start/end items: the width
/// of the items between the breaks (minus the end glue) reproduces the
/// stretch/shrink ratio within f64 roundoff.
#[test]
fn ratios_are_consistent_with_line_widths() {
    let items = words(8, 90.0, 22.0);
    let line_width = 300.0_f64;
    let lines = line_break(&items, line_width, 100.0, 10.0);
    // Prefix sums over item widths / stretch / shrink, exactly as in `run`.
    let mut width_prefix = vec![0.0_f64; items.len() + 1];
    let mut stretch_prefix = vec![0.0_f64; items.len() + 1];
    let mut shrink_prefix = vec![0.0_f64; items.len() + 1];
    for (i, item) in items.iter().enumerate() {
        width_prefix[i + 1] = width_prefix[i] + item.width();
        match item {
            KpItem::Glue {
                stretch, shrink, ..
            } => {
                stretch_prefix[i + 1] = stretch_prefix[i] + stretch;
                shrink_prefix[i + 1] = shrink_prefix[i] + shrink;
            }
            _ => {
                stretch_prefix[i + 1] = stretch_prefix[i];
                shrink_prefix[i + 1] = shrink_prefix[i];
            }
        }
    }
    for line in &lines {
        // The previous breakpoint index: KpLine.start is prev_break + 1
        // (0 when the line starts at the paragraph head). The DP's line
        // content is sumW[end] - sumW[prev_break] minus the breakpoint
        // item's own width; stretch/shrink are the same span.
        let prev_break = line.start.saturating_sub(1);
        let natural = width_prefix[line.end] - width_prefix[prev_break] - items[line.end].width();
        let stretch = stretch_prefix[line.end] - stretch_prefix[prev_break];
        let shrink = shrink_prefix[line.end] - shrink_prefix[prev_break];
        // Lines ending at the forced penalty use the forced branch: ratio 0
        // when the line fits, the emergency-stretch ratio when it overflows.
        let forced_end = matches!(
            items[line.end],
            KpItem::Penalty { penalty, .. } if penalty == f64::NEG_INFINITY
        );
        let expected = if forced_end {
            if natural <= line_width + 1e-9 {
                0.0
            } else {
                (natural - line_width) / 10.0
            }
        } else if natural <= line_width + 1e-9 {
            if stretch > 0.0 {
                (line_width - natural) / stretch
            } else {
                0.0
            }
        } else if shrink > 0.0 {
            (line_width - natural) / shrink
        } else {
            // An overflowing non-forced line with no shrink is infeasible
            // and is never chosen.
            unreachable!("chosen non-forced line cannot be infeasible")
        };
        assert!(
            (line.ratio - expected).abs() < 1e-9,
            "line {line:?}: ratio {} != expected {expected}",
            line.ratio
        );
    }
}

/// An overflowing unbreakable line pays the emergency-stretch badness
/// (TeX \\emergencystretch): ratio = (width - lineWidth) / 10.
#[test]
fn overflow_lines_carry_the_emergency_stretch_ratio() {
    let items = vec![
        KpItem::Box { width: 500.0 },
        KpItem::Penalty {
            width: 0.0,
            penalty: f64::NEG_INFINITY,
        },
    ];
    let lines = line_break(&items, 200.0, 100.0, 10.0);
    assert_eq!(lines.len(), 1);
    let ratio = (500.0 - 200.0) / 10.0;
    assert!((lines[0].ratio - ratio).abs() < 1e-9);
    assert!((lines[0].badness - 100.0 * ratio.powi(3)).abs() < 1e-9);
}

/// A pathological narrow column (one word per line) exhausts the active
/// list — every line ending at a glue overflows with zero shrink — and the
/// dynamic program falls back to a single line covering the paragraph,
/// exactly like the JS runtime (the fallback `{start: 0, end: n-1,
/// ratio: 0}`). This pins the JS parity for the case where the Knuth–Plass
/// active list dies; the paper's deactivation rule is a documented future
/// improvement (DESIGN.md).
#[test]
fn narrow_columns_fall_back_to_a_single_overflowing_line() {
    // Mirror the layout engine's item stream for "one two three ... twelve"
    // at 16px with no atlas (8px per char): word boxes + space boxes +
    // per-token glues, ending in the forced penalty.
    let words = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve",
    ];
    let mut items = Vec::new();
    for (i, w) in words.iter().enumerate() {
        items.push(KpItem::Box {
            width: w.chars().count() as f64 * 8.0,
        });
        if i < words.len() - 1 {
            items.push(KpItem::Glue {
                width: 0.0,
                stretch: 4.0,
                shrink: 0.0,
            });
            items.push(KpItem::Box { width: 8.0 });
            items.push(KpItem::Glue {
                width: 4.0,
                stretch: 2.0,
                shrink: 1.33,
            });
        }
    }
    items.push(KpItem::Penalty {
        width: 0.0,
        penalty: f64::NEG_INFINITY,
    });
    let lines = line_break(&items, 32.0, 100.0, 10.0);
    assert_eq!(
        lines.len(),
        1,
        "active list exhaustion falls back to one line"
    );
    assert_eq!(lines[0].start, 0);
    assert_eq!(lines[0].end, items.len() - 1);
    assert_eq!(lines[0].ratio, 0.0);
}
