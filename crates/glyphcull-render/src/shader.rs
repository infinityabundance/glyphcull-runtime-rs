//! The MSDF render program (DESIGN.md D9/D28; the Rust counterpart of the
//! JS `src/render/gl.ts` shaders): one logical WGSL program — the median-of-
//! three reconstruction with a fixed 1-device-px edge, premultiplied alpha,
//! document-y-down coordinates.
//!
//! The shader math is the exact translation of `msdf.rs`:
//!
//! ```text
//! vertex    ndc = (pos * scale + offset) * 2 - 1;  clip = (ndc.x, -ndc.y)
//! fragment  distPx = (median(r, g, b) - 0.5) * pxRange
//!           coverage = clamp(distPx + 0.5)² · (3 - 2 · clamp(distPx + 0.5))
//!           out = (color.rgb · coverage, color.a · coverage)
//! ```
//!
//! wgpu compiles WGSL to the GL backend's GLSL and to SPIR-V via naga, so
//! this single source is the one logical program for every backend (the tests
//! validate the WGSL and its GLSL/SPIR-V translations with naga). The GLSL
//! written by hand in the JS runtime expresses the same math.
//!
//! Sampling phase (DESIGN.md D28, corrected): the fragment samples at
//! `uv·size − 0.5` (GL/Vulkan convention); the glyph UVs are shifted by half a
//! texel at plan flattening (`renderer::flatten_plan`) so the physical sample
//! equals the CPU reference's pixel center — exactly the compensation the JS
//! renderer's vertex shader applies.

/// The combined vertex + fragment shader source (WGSL). One module, two
/// entry points (`vs_main`, `fs_main`).
pub const MSDF_WGSL: &str = r#"
struct ViewUniform {
    scale: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> view: ViewUniform;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) px_range: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) px_range: f32,
};

fn median(r: f32, g: f32, b: f32) -> f32 {
    return max(min(r, g), min(max(r, g), b));
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    // Document y grows down; NDC y grows up, so the projected y is negated.
    let ndc = (in.pos * view.scale + view.offset) * 2.0 - 1.0;
    out.clip = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.px_range = in.px_range;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let msd = textureSample(tex, samp, in.uv).rgb;
    let dist_px = (median(msd.r, msd.g, msd.b) - 0.5) * in.px_range;
    let t = clamp(dist_px + 0.5, 0.0, 1.0);
    let coverage = t * t * (3.0 - 2.0 * t);
    return vec4<f32>(in.color.rgb * coverage, in.color.a * coverage);
}
"#;

/// The vertex buffer layout shared by every draw: pos/uv/color/pxRange —
/// 9 f32 per vertex, matching the JS `stride = 9 * 4` and the attribute
/// offsets of `src/render/gl.ts`.
pub const VERTEX_STRIDE: u64 = 9 * 4;

#[cfg(test)]
mod tests {
    //! The WGSL must parse, validate, and translate to GLSL (the wgpu GL
    //! backend) and SPIR-V (Vulkan) — proving the single logical program
    //! compiles everywhere without a GPU.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::MSDF_WGSL;

    fn validated(source: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(source).expect("WGSL parses");
        assert!(!module.entry_points.is_empty(), "has entry points");
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("WGSL validates");
        (module, info)
    }

    fn glsl_string(
        module: &naga::Module,
        info: &naga::valid::ModuleInfo,
        stage: naga::ShaderStage,
        entry: &str,
    ) -> String {
        let options = naga::back::glsl::Options {
            version: naga::back::glsl::Version::new_gles(300),
            writer_flags: naga::back::glsl::WriterFlags::empty(),
            binding_map: Default::default(),
            zero_initialize_workgroup_memory: true,
        };
        let pipeline_options = naga::back::glsl::PipelineOptions {
            shader_stage: stage,
            entry_point: entry.to_string(),
            multiview: None,
        };
        let mut out: String = String::new();
        let mut writer = naga::back::glsl::Writer::new(
            &mut out,
            module,
            info,
            &options,
            &pipeline_options,
            Default::default(),
        )
        .expect("GLSL writer");
        writer.write().expect("GLSL write");
        out
    }

    #[test]
    fn the_program_parses_validates_and_translates_to_glsl_and_spirv() {
        let (module, info) = validated(MSDF_WGSL);
        for (stage, entry) in [
            (naga::ShaderStage::Vertex, "vs_main"),
            (naga::ShaderStage::Fragment, "fs_main"),
        ] {
            let glsl = glsl_string(&module, &info, stage, entry);
            assert!(!glsl.is_empty(), "GLSL emitted for {entry}");
            let _spv = naga::back::spv::write_vec(
                &module,
                &info,
                &naga::back::spv::Options::default(),
                None,
            )
            .expect("SPIR-V translation succeeds");
        }
    }

    #[test]
    fn the_shader_math_matches_the_cpu_reference() {
        // The fragment evaluates msdfCoverage(channels, pxRange, 1) with the
        // shader's t²(3-2t) smoothstep; the CPU reference is the same
        // function. Spot-check a few channels.
        for (r, g, b) in [
            (0.5, 0.5, 0.5),
            (0.6, 0.4, 0.5),
            (0.9, 0.9, 0.1),
            (0.0, 0.7, 0.0),
        ] {
            let dist_px = (crate::msdf::median(r, g, b) - 0.5) * 16.0;
            let t = (dist_px + 0.5).clamp(0.0, 1.0);
            let coverage = t * t * (3.0 - 2.0 * t);
            let reference = crate::msdf::msdf_coverage([r, g, b], 16.0, 1.0);
            assert!(
                (coverage - reference).abs() < 1e-9,
                "shader math == cpu reference"
            );
        }
    }
}
