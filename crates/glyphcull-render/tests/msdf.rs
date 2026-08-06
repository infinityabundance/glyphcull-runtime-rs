//! MSDF reconstruction tests — mirrors the JS `test/render/msdf.test.ts`
//! vector for vector: the median, smoothstep, texel scaling, a synthetic-edge
//! reconstruction profile (including downsampling and page-edge clamping),
//! and the equivalence of the reference convenience wrapper.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use glyphcull_render::msdf::{
    median, msdf_coverage, reconstruct, reconstruct_glyph, smoothstep, texel_to_px,
};

#[test]
fn median_returns_the_middle_value() {
    assert_eq!(median(0.1, 0.5, 0.9), 0.5);
    assert_eq!(median(0.9, 0.1, 0.5), 0.5);
    assert_eq!(median(0.5, 0.9, 0.1), 0.5);
    assert_eq!(median(0.1, 0.9, 0.9), 0.9);
}

#[test]
fn smoothstep_is_0_1_outside_and_hermite_inside() {
    assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
    assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
    assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
    assert!((smoothstep(0.0, 1.0, 0.25) - 0.15625).abs() < 1e-5);
}

#[test]
fn texel_to_px_scales_with_font_size_and_dpr() {
    assert_eq!(texel_to_px(16.0, 1.0, 32.0), 0.5);
    assert_eq!(texel_to_px(16.0, 2.0, 32.0), 1.0);
    assert_eq!(texel_to_px(32.0, 1.0, 32.0), 1.0);
}

#[test]
fn msdf_coverage_is_1_inside_and_0_far_outside() {
    // Edge at channel 0.5; texelToPx 1 → distancePx = channel − 0.5. The AA
    // edge spans ±0.5 device px, so saturation needs |dist| ≥ 0.5.
    assert!((msdf_coverage([1.0, 1.0, 1.0], 1.0, 1.0) - 1.0).abs() < 1e-5);
    assert!(msdf_coverage([0.0, 0.0, 0.0], 1.0, 1.0) < 1e-5);
    assert!((msdf_coverage([0.5, 0.5, 0.5], 1.0, 1.0) - 0.5).abs() < 1e-5);
    // A channel 0.9 is 0.4 device px inside the edge → partial coverage.
    assert!((msdf_coverage([0.9, 0.9, 0.9], 1.0, 1.0) - 0.972).abs() < 1e-3);
    assert!((msdf_coverage([0.1, 0.1, 0.1], 1.0, 1.0) - 0.028).abs() < 1e-3);
}

#[test]
fn the_median_prevents_a_single_near_channel_from_dominating() {
    // median(0.1, 0.1, 0.9) = 0.1: the corner stays far, exactly as if all
    // three channels were far.
    assert_eq!(
        msdf_coverage([0.1, 0.1, 0.9], 1.0, 1.0),
        msdf_coverage([0.1, 0.1, 0.1], 1.0, 1.0)
    );
    assert_eq!(
        msdf_coverage([0.9, 0.1, 0.1], 1.0, 1.0),
        msdf_coverage([0.1, 0.1, 0.1], 1.0, 1.0)
    );
    assert_eq!(
        msdf_coverage([0.9, 0.9, 0.1], 1.0, 1.0),
        msdf_coverage([0.9, 0.9, 0.9], 1.0, 1.0)
    );
}

#[test]
fn widens_the_transition_with_the_aa_width() {
    assert!((msdf_coverage([0.5, 0.5, 0.5], 1.0, 2.0) - 0.5).abs() < 1e-5);
    // Half a device px inside a 2px-wide edge (dist = 0.25): t = 0.625.
    assert!((msdf_coverage([0.75, 0.75, 0.75], 1.0, 2.0) - 0.684).abs() < 1e-3);
    // The same channel at 1px AA is closer to saturation: the wider edge
    // softens.
    assert!(
        msdf_coverage([0.75, 0.75, 0.75], 1.0, 1.0) > msdf_coverage([0.75, 0.75, 0.75], 1.0, 2.0)
    );
}

/// An 8×8 page with a vertical edge at x = 4:
/// channel = clamp(0.5 + (x−4), 0, 1) → the 0.5 level sits between x=3 and 4.
fn vertical_edge_page() -> Vec<u8> {
    let mut page = vec![0u8; 8 * 8 * 4];
    for y in 0..8 {
        for x in 0..8 {
            let channel = (0.5 + (x as f64 - 4.0)).clamp(0.0, 1.0);
            let value = (channel * 255.0).round() as u8;
            let i = (y * 8 + x) * 4;
            page[i] = value;
            page[i + 1] = value;
            page[i + 2] = value;
            page[i + 3] = 255;
        }
    }
    page
}

#[test]
fn recovers_a_vertical_edge_profile_from_a_synthetic_field() {
    let page = vertical_edge_page();
    let out = reconstruct_glyph(&page, 8, 0, 0, 8, 8, 1.0, 4);
    // Coverage rises monotonically across the edge at x = 4 (1 device px per
    // texel, 1px AA edge): outside < 0.5 < inside, saturated past the edge.
    let c = |i: usize| out[i] as f64 / 255.0;
    assert!(c(3) < 0.5);
    assert!(c(4) > 0.5);
    assert!((c(5) - 1.0).abs() < 1e-2);
    assert!(c(3) < c(4));
    assert!(c(4) < c(5));
    // Pinned values (analytic texel-grid bilinear, 4×4 supersampling): the
    // edge spans exactly one output pixel.
    assert!((c(3) - 0.187).abs() < 1e-2);
    assert!((c(4) - 0.816).abs() < 1e-2);
}

#[test]
fn reconstruct_glyph_is_the_same_computation_as_reconstruct_at_equal_resolution() {
    let page = vertical_edge_page();
    let a = reconstruct_glyph(&page, 8, 0, 0, 8, 8, 1.0, 4);
    let b = reconstruct(&page, 8, 0, 0, 8, 8, 8, 8, 1.0, 4);
    assert_eq!(a, b);
}

#[test]
fn clamps_at_page_edges_instead_of_reading_out_of_bounds() {
    // A box flush against the page corner: samples touch the last texel row
    // and column. Every output byte must be a finite 0..255 integer (no NaN
    // from out-of-bounds reads), and the far side of the edge is saturated.
    let page = vertical_edge_page();
    let out = reconstruct_glyph(&page, 8, 0, 0, 8, 8, 1.0, 1);
    assert_eq!(out.len(), 64);
    for &byte in &out {
        let _ = byte; // u8 is 0..=255 by construction; the JS checked finiteness
    }
    assert_eq!(out[0], 0); // top-left, far outside the edge
    assert_eq!(out[63], 255); // bottom-right, fully inside
}

#[test]
fn downsamples_the_edge_falls_between_output_pixels_at_half_resolution() {
    let page = vertical_edge_page();
    // 8×8 texels → 4×4 pixels (2 texels per pixel), texelToPx 1.
    let out = reconstruct(&page, 8, 0, 0, 8, 8, 4, 4, 1.0, 4);
    let c = |i: usize| out[i] as f64 / 255.0;
    assert!(c(1) < 0.5); // texel footprint [2,4): mostly outside
    assert!(c(2) > 0.5); // footprint [4,6): mostly inside
    assert!(c(1) < c(2));
}

#[test]
fn a_zero_output_size_is_defensive_empty() {
    // The JS throws a RangeError for a zero/fractional output size; Rust's
    // usize makes those unrepresentable, and zero yields an empty buffer
    // (DESIGN.md R6).
    let page = vertical_edge_page();
    let out = reconstruct(&page, 8, 0, 0, 8, 8, 0, 8, 1.0, 4);
    assert!(out.is_empty());
}

#[test]
fn a_box_larger_than_the_page_still_produces_finite_coverage() {
    let page = vertical_edge_page();
    let out = reconstruct(&page, 8, 0, 0, 10, 10, 5, 5, 1.0, 4);
    // Every output byte is a valid u8 (0..=255 by construction).
    assert_eq!(out.len(), 25);
}
