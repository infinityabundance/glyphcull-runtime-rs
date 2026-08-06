//! MSDF reconstruction (SPEC.md §2.5, normative; mirrors the JS
//! `src/render/msdf.ts`): the coverage at a sample is the median of the three
//! channels mapped through a smoothstep with screen-space width.
//!
//! ```text
//! median        = max(min(r, g), min(max(r, g), b))
//! texelToPx     = fontSizePx * dpr / texelsPerEm
//! distancePx    = (median - 0.5) * texelToPx
//! coverage      = smoothstep(-0.5, +0.5, distancePx)      (1 device px edge)
//! ```
//!
//! This pure module is the single source of truth: the WGSL shader
//! (`shader.rs`) is its translation (compiled to GLSL/SPIR-V by naga for the
//! wgpu GL backend), and the rendering validation compares the GPU against
//! it.
//!
//! Sampling convention (DESIGN.md D28): atlas page pixels are stored
//! top-row-first (y grows down) and textures are uploaded without flipping.
//! The reference samples at texel centers `(i + 0.5)` in texel space; wgpu
//! normalizes the half-texel phase across backends (Vulkan-style, unlike raw
//! WebGL, where the JS runtime applies a `+0.5/size` UV shift), so the GPU
//! and the reference agree exactly.

/// The median of three values (the MSDF reconstruction).
#[must_use]
pub fn median(r: f64, g: f64, b: f64) -> f64 {
    r.min(g).max((r.max(g)).min(b))
}

/// Hermite smoothstep.
#[must_use]
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The device pixels spanned by one atlas texel at a font size and DPR.
#[must_use]
pub fn texel_to_px(font_size_px: f64, dpr: f64, texels_per_em: f64) -> f64 {
    (font_size_px * dpr) / texels_per_em
}

/// The reconstructed coverage of an MSDF sample.
///
/// - `channels`: the three distance channels (0..1; the edge is at 0.5)
/// - `texel_to_px`: device pixels per texel at the rendered size
/// - `aa_width_px`: the anti-aliasing edge width in device pixels (1)
#[must_use]
pub fn msdf_coverage(channels: [f64; 3], texel_to_px: f64, aa_width_px: f64) -> f64 {
    let distance_px = (median(channels[0], channels[1], channels[2]) - 0.5) * texel_to_px;
    smoothstep(-aa_width_px / 2.0, aa_width_px / 2.0, distance_px)
}

/// Reconstruct a glyph bitmap from an atlas page at an arbitrary output
/// resolution. Output pixel `(ox, oy)` covers the texel footprint
/// `[boxX + ox·texelW/outW, boxX + (ox+1)·texelW/outW] × [boxY + …]`; each
/// pixel is supersampled on a `samples_per_texel²` grid over its footprint
/// and the coverages are averaged. This is the CPU reference for rendering
/// validation: the shader evaluates the same function at fragment centers
/// (supersampling 1), so both agree within the validation tolerance.
///
/// A zero output dimension (defensive; the JS throws a `RangeError`) yields
/// an empty buffer.
///
/// The `out[y * out_w + x]` index is provably in bounds (`x < out_w`,
/// `y < out_h`); the argument count mirrors the JS signature (scoped allows).
#[must_use]
#[allow(clippy::too_many_arguments, clippy::indexing_slicing)]
pub fn reconstruct(
    page: &[u8],
    page_width: usize,
    box_x: usize,
    box_y: usize,
    texel_w: usize,
    texel_h: usize,
    out_w: usize,
    out_h: usize,
    texel_to_px: f64,
    samples_per_texel: usize,
) -> Vec<u8> {
    if out_w == 0 || out_h == 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; out_w * out_h];
    let ss = samples_per_texel.max(1) as f64;
    let step_x = texel_w as f64 / out_w as f64;
    let step_y = texel_h as f64 / out_h as f64;
    for y in 0..out_h {
        for x in 0..out_w {
            let mut acc = 0.0_f64;
            for sy in 0..ss as usize {
                for sx in 0..ss as usize {
                    let tx = box_x as f64 + (x as f64 + (sx as f64 + 0.5) / ss) * step_x;
                    let ty = box_y as f64 + (y as f64 + (sy as f64 + 0.5) / ss) * step_y;
                    acc += coverage_at(page, page_width, tx, ty, texel_to_px);
                }
            }
            out[y * out_w + x] = ((acc / (ss * ss)) * 255.0).round() as u8;
        }
    }
    out
}

/// The reference reconstruction at one output pixel per texel (the glyph box
/// rasterized 1:1 against the atlas). Equivalent to [`reconstruct`] with
/// `out = box` dimensions.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_glyph(
    page: &[u8],
    page_width: usize,
    box_x: usize,
    box_y: usize,
    box_w: usize,
    box_h: usize,
    texel_to_px: f64,
    samples_per_texel: usize,
) -> Vec<u8> {
    reconstruct(
        page,
        page_width,
        box_x,
        box_y,
        box_w,
        box_h,
        box_w,
        box_h,
        texel_to_px,
        samples_per_texel,
    )
}

/// The MSDF coverage at an arbitrary point in texel space — the exact
/// function the shader evaluates per fragment (bilinear per channel, median,
/// smoothstep).
///
/// Edge clamp to the last texel, matching `CLAMP_TO_EDGE` (glyph boxes sit
/// inside the page, but a box flush against a page edge must not read past
/// the buffer). Both taps clamp: when a sample sits exactly in the last texel
/// the second tap reads the same texel, like the GPU's edge clamp.
///
/// The page is RGBA8 (`page.len() == 4 · page_width · page_height`); a page
/// without at least one full row of texels yields coverage 0 (the JS guards
/// the same).
#[must_use]
#[allow(clippy::indexing_slicing)] // indices are clamped to the page bounds, as in the JS
pub fn coverage_at(page: &[u8], page_width: usize, tx: f64, ty: f64, texel_to_px: f64) -> f64 {
    if page_width == 0 || page.len() < page_width * 4 {
        return 0.0;
    }
    let page_height = page.len() as f64 / 4.0 / page_width as f64;
    if page_height < 1.0 {
        return 0.0;
    }
    let cx = tx.clamp(0.0, page_width as f64 - 1.0);
    let cy = ty.clamp(0.0, page_height - 1.0);
    let x0 = cx.floor() as usize;
    let y0 = cy.floor() as usize;
    let x1 = (x0 + 1).min(page_width - 1);
    let y1 = (y0 + 1).min(page_height.ceil() as usize - 1);
    let fx = cx - x0 as f64;
    let fy = cy - y0 as f64;
    let i00 = (y0 * page_width + x0) * 4;
    let i10 = (y0 * page_width + x1) * 4;
    let i01 = (y1 * page_width + x0) * 4;
    let i11 = (y1 * page_width + x1) * 4;
    // Bilinear per channel (the shader's LINEAR sampling), then the median:
    // identical to `median(textureSample(...).rgb)`.
    let r = lerp(
        lerp(page[i00] as f64, page[i10] as f64, fx),
        lerp(page[i01] as f64, page[i11] as f64, fx),
        fy,
    ) / 255.0;
    let g = lerp(
        lerp(page[i00 + 1] as f64, page[i10 + 1] as f64, fx),
        lerp(page[i01 + 1] as f64, page[i11 + 1] as f64, fx),
        fy,
    ) / 255.0;
    let b = lerp(
        lerp(page[i00 + 2] as f64, page[i10 + 2] as f64, fx),
        lerp(page[i01 + 2] as f64, page[i11 + 2] as f64, fx),
        fy,
    ) / 255.0;
    msdf_coverage([r, g, b], texel_to_px, 1.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
