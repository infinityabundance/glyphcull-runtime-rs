//! Rendering validation vs a golden image (the TESTING.md "renderer output vs
//! golden reference images" layer, delivered headless): the whole pipeline —
//! parse → model → layout → draw list → MSDF reconstruction → compositing —
//! is rasterized on the CPU and compared against a committed golden PNG.
//!
//! The rasterizer implements the same math as the WGSL shader and the JS
//! browser harness's CPU reference (DESIGN.md D9 pixel-center sampling,
//! premultiplied-over compositing, MSDF median/smoothstep coverage); the wgpu
//! executor plays the same plan on the GPU (4.9) and is exercised on the
//! desktop host. Everything here is deterministic and headless.
//!
//! Regeneration is deliberate: `GLYPHCULL_REGEN_GOLDEN=1` rewrites the
//! fixture (see `scripts/regen-golden-image.sh`); the diff must be reviewed
//! before committing, like the compiler's golden fixtures.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs::File;
use std::path::Path;

use glyphcull_core::document::build_document;
use glyphcull_core::draw_list::{DrawCommand, DrawListBuilder, TextureResolver};
use glyphcull_core::glyphs::{prepare_glyph, GlyphStamp};
use glyphcull_core::layout::layout::{GlyphInstance, LayoutEngine, LayoutOptions};
use glyphcull_core::reader::glyph::Atlas;
use glyphcull_core::reader::image::ImageRecord;
use glyphcull_core::reader::parse;
use glyphcull_render::msdf::reconstruct;

/// The golden fixture bytes (shared with glyphcull-core's tests).
const GOLDEN: &[u8] = include_bytes!("../../glyphcull-core/tests/fixtures/pipeline-golden.cull");
/// The committed golden rendering fixture (this crate's tests/).
const FIXTURE: &str = "tests/fixtures/golden-document.png";
/// The rendering content width (matches the host default).
const CONTENT_WIDTH: f32 = 800.0;

/// Texture-handle convention shared with the other render tests.
struct TestResolver;

impl TextureResolver for TestResolver {
    fn atlas_page(&self, atlas_id: u32, page_index: u16) -> u32 {
        atlas_id * 100 + u32::from(page_index)
    }
    fn image(&self, image_id: u32) -> u32 {
        2000 + image_id
    }
}

/// The stamps closure (same as `tests/plan.rs`): prepare per laid-out glyph.
fn stamp_for<'a>(
    engine: &'a LayoutEngine<'a>,
) -> impl FnMut(u32, &GlyphInstance) -> Option<GlyphStamp> + 'a {
    let atlases: &'a [Atlas] = engine.document().atlases();
    move |_chunk_id, glyph| {
        let atlas = atlases.get(glyph.atlas_id as usize)?;
        prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
    }
}

/// Straight (non-premultiplied) RGBA components of a u32 color.
fn straight_components(color: u32) -> [f64; 4] {
    [
        ((color >> 24) & 0xff) as f64 / 255.0,
        ((color >> 16) & 0xff) as f64 / 255.0,
        ((color >> 8) & 0xff) as f64 / 255.0,
        (color & 0xff) as f64 / 255.0,
    ]
}

/// A minimal straight-alpha RGBA canvas with premultiplied-over compositing
/// (the reference rasterizer for the golden image).
struct Raster {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Raster {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height * 4],
        }
    }

    /// Composite one straight-alpha source pixel over the destination.
    fn over(&mut self, x: usize, y: usize, src: [f64; 4]) {
        let index = (y * self.width + x) * 4;
        let dst = [
            self.pixels[index] as f64 / 255.0,
            self.pixels[index + 1] as f64 / 255.0,
            self.pixels[index + 2] as f64 / 255.0,
            self.pixels[index + 3] as f64 / 255.0,
        ];
        let out_alpha = src[3] + dst[3] * (1.0 - src[3]);
        let out = if out_alpha > 0.0 {
            [
                (src[0] * src[3] + dst[0] * dst[3] * (1.0 - src[3])) / out_alpha,
                (src[1] * src[3] + dst[1] * dst[3] * (1.0 - src[3])) / out_alpha,
                (src[2] * src[3] + dst[2] * dst[3] * (1.0 - src[3])) / out_alpha,
                out_alpha,
            ]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
        self.pixels[index] = (out[0] * 255.0).round() as u8;
        self.pixels[index + 1] = (out[1] * 255.0).round() as u8;
        self.pixels[index + 2] = (out[2] * 255.0).round() as u8;
        self.pixels[index + 3] = (out[3] * 255.0).round() as u8;
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let src = straight_components(color);
        let x0 = x.round().max(0.0) as usize;
        let y0 = y.round().max(0.0) as usize;
        let x1 = ((x + w).round() as isize).clamp(0, self.width as isize) as usize;
        let y1 = ((y + h).round() as isize).clamp(0, self.height as isize) as usize;
        for py in y0..y1 {
            for px in x0..x1 {
                self.over(px, py, src);
            }
        }
    }

    fn glyph(
        &mut self,
        cmd: &glyphcull_core::draw_list::GlyphCommand,
        page: &[u8],
        page_width: usize,
        page_height: usize,
    ) {
        // The ink box in texel space comes from the UV rect; the quad (x, y,
        // w, h) is the same box in document pixels at the laid-out size.
        let box_x = (f64::from(cmd.uv[0]) * page_width as f64).round() as usize;
        let box_y = (f64::from(cmd.uv[1]) * page_height as f64).round() as usize;
        let box_w = ((f64::from(cmd.uv[2]) - f64::from(cmd.uv[0])) * page_width as f64)
            .round()
            .max(1.0) as usize;
        let box_h = ((f64::from(cmd.uv[3]) - f64::from(cmd.uv[1])) * page_height as f64)
            .round()
            .max(1.0) as usize;
        let out_w = cmd.w.round() as usize;
        let out_h = cmd.h.round() as usize;
        if out_w == 0 || out_h == 0 {
            return;
        }
        // The CPU MSDF reference (the exact function the shader evaluates).
        let coverage = reconstruct(
            page,
            page_width,
            box_x,
            box_y,
            box_w,
            box_h,
            out_w,
            out_h,
            f64::from(cmd.px_per_texel),
            4,
        );
        let color = straight_components(cmd.color);
        let x0 = cmd.x.round() as isize;
        let y0 = cmd.y.round() as isize;
        for py in 0..out_h {
            for px in 0..out_w {
                let alpha = coverage[py * out_w + px] as f64 / 255.0 * color[3];
                let cx = x0 + px as isize;
                let cy = y0 + py as isize;
                if cx < 0 || cy < 0 || cx >= self.width as isize || cy >= self.height as isize {
                    continue;
                }
                self.over(
                    cx as usize,
                    cy as usize,
                    [color[0], color[1], color[2], alpha],
                );
            }
        }
    }

    fn image(
        &mut self,
        cmd: &glyphcull_core::draw_list::ImageCommand,
        data: &[u8],
        width: usize,
        height: usize,
        bytes_per_pixel: usize,
    ) {
        let x0 = cmd.x.round().max(0.0) as usize;
        let y0 = cmd.y.round().max(0.0) as usize;
        let x1 = ((cmd.x + cmd.w).round() as isize).clamp(0, self.width as isize) as usize;
        let y1 = ((cmd.y + cmd.h).round() as isize).clamp(0, self.height as isize) as usize;
        for py in y0..y1 {
            for px in x0..x1 {
                // Nearest-neighbor sample (the golden has no images; this
                // keeps the rasterizer total for image-bearing documents).
                let sx = ((px - x0) as f32 / cmd.w * width as f32).floor() as usize;
                let sy = ((py - y0) as f32 / cmd.h * height as f32).floor() as usize;
                let i = (sy * width + sx) * bytes_per_pixel;
                let src = [
                    data[i] as f64 / 255.0,
                    data[i + 1] as f64 / 255.0,
                    data[i + 2] as f64 / 255.0,
                    if bytes_per_pixel == 4 {
                        data[i + 3] as f64 / 255.0
                    } else {
                        1.0
                    },
                ];
                self.over(px, py, src);
            }
        }
    }

    /// Play a draw list into the canvas, resolving textures from the package.
    fn draw(
        &mut self,
        list: &glyphcull_core::draw_list::DrawList,
        atlases: &[Atlas],
        images: &[ImageRecord],
    ) {
        for command in &list.commands {
            match command {
                DrawCommand::Fill(fill) => {
                    self.fill_rect(fill.x, fill.y, fill.w, fill.h, fill.color)
                }
                DrawCommand::Ruler(ruler) => {
                    // A 1-device-px rule at the block's ruler position.
                    self.fill_rect(ruler.x, ruler.y - 0.5, ruler.w, 1.0, ruler.color);
                }
                DrawCommand::Glyph(glyph) => {
                    let atlas_id = (glyph.texture / 100) as usize;
                    let page_index = (glyph.texture % 100) as usize;
                    let Some(atlas) = atlases.get(atlas_id) else {
                        continue;
                    };
                    let Some(page) = atlas.pages.get(page_index) else {
                        continue;
                    };
                    self.glyph(
                        glyph,
                        page,
                        atlas.page_width as usize,
                        atlas.page_height as usize,
                    );
                }
                DrawCommand::Image(image) => {
                    let image_id = image.texture.saturating_sub(2000) as usize;
                    let Some(image_record) = images.get(image_id) else {
                        continue;
                    };
                    let bpp = if image_record.format
                        == glyphcull_core::reader::image::ImageFormat::Rgb8
                    {
                        3
                    } else {
                        4
                    };
                    self.image(
                        image,
                        &image_record.data,
                        image_record.width as usize,
                        image_record.height as usize,
                        bpp,
                    );
                }
            }
        }
    }
}

fn encode_png(path: &Path, width: u32, height: u32, pixels: &[u8]) {
    let file = File::create(path).expect("create fixture");
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(pixels).expect("png data");
}

fn decode_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = File::open(path).expect("open fixture");
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().expect("png info");
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).expect("png frame");
    (
        info.width,
        info.height,
        buffer[..info.buffer_size()].to_vec(),
    )
}

/// The full document rasterized by the CPU reference (canvas = content width
/// × document height). Everything lives in one scope: engine borrows the
/// model borrows the package.
fn rasterize_golden() -> Raster {
    let pkg = parse(GOLDEN).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: CONTENT_WIDTH,
        },
    );
    engine.extend_to(f64::INFINITY);
    let visible_ids: Vec<u32> = engine.records_all().keys().copied().collect();
    let builder = DrawListBuilder::new(TestResolver);
    let list = builder.build(&engine, &visible_ids, stamp_for(&engine), &[]);

    let height = engine
        .records_all()
        .values()
        .map(|record| (record.y + record.h).ceil() as usize)
        .max()
        .unwrap_or(1)
        .max(1);
    let width = CONTENT_WIDTH.round().max(1.0) as usize;
    let mut raster = Raster::new(width, height);
    raster.draw(
        &list,
        engine.document().atlases(),
        engine.document().images(),
    );
    raster
}

#[test]
fn the_full_document_rasterizes_to_the_golden_image() {
    let raster = rasterize_golden();
    let pixels = raster.pixels.clone();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);

    if std::env::var_os("GLYPHCULL_REGEN_GOLDEN").is_some() {
        encode_png(&path, raster.width as u32, raster.height as u32, &pixels);
        eprintln!("regenerated {FIXTURE} ({}×{})", raster.width, raster.height);
        return;
    }

    let (golden_width, golden_height, golden) = decode_png(&path);
    assert_eq!(
        (golden_width as usize, golden_height as usize),
        (raster.width, raster.height),
        "golden fixture dimensions must match the rendered canvas"
    );
    assert_eq!(golden.len(), pixels.len(), "golden fixture size must match");

    // The rasterizer is deterministic, so the diff is exact; the tolerances
    // guard against fixture staleness (a regeneration diff must be reviewed).
    let mut sum = 0.0_f64;
    let mut max_diff = 0u8;
    let mut opaque = 0usize;
    for (index, &pixel) in pixels.iter().enumerate() {
        let diff = pixel.abs_diff(golden[index]);
        sum += f64::from(diff);
        max_diff = max_diff.max(diff);
        if index % 4 == 3 && pixel > 0 {
            opaque += 1;
        }
    }
    let mean = sum / pixels.len() as f64;
    let coverage = opaque as f64 / (raster.width * raster.height) as f64;
    eprintln!(
        "golden image: {}×{}, mean-abs-error {mean:.4}, max {max_diff}, coverage {coverage:.3}",
        raster.width, raster.height
    );
    assert!(mean <= 0.5, "mean abs error {mean} exceeds tolerance");
    assert!(max_diff <= 8, "max pixel diff {max_diff} exceeds tolerance");
    assert!(
        coverage > 0.01,
        "the render must be non-blank (coverage {coverage})"
    );
}
