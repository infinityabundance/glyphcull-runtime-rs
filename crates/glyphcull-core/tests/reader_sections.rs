//! Section-decoder strictness beyond the golden fixture.
//!
//! The golden exercises INFO, CHNK (link_target extra), STYL (a subset),
//! CONT (text only), GLYF, and SEAL — but not IMGS, the remaining CHNK extra
//! kinds, CONT image references, or the full STYL property tag set. Those
//! paths are covered here with hand-built payloads, and each decoder's
//! rejection branches are pinned to the same typed errors the JS reader
//! produces.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::error::ErrorKind;
use glyphcull_core::reader::chunk::{ExtraData, ExtraKind};
use glyphcull_core::reader::content::ContentData;
use glyphcull_core::reader::image::ImageFormat;
use glyphcull_core::reader::style::{PropertyTag, PropertyValue};
use glyphcull_core::reader::{parse, SectionKind};

fn package(kind: SectionKind, payload: Vec<u8>) -> Vec<u8> {
    common::build_package(&[common::TestSection {
        kind: kind as u32,
        compression: 0,
        payload,
    }])
}

// ---------------------------------------------------------------------------
// §2.6 IMGS

#[test]
fn imgs_decodes_both_formats() {
    let rgba: Vec<u8> = (0..(2 * 2 * 4)).map(|i| i as u8).collect();
    let rgb: Vec<u8> = (0..9).map(|i| i as u8).collect(); // 1x3 RGB8
    let payload = common::imgs_payload(&[
        common::TestImage {
            width: 2,
            height: 2,
            format: 0,
            data: rgba.clone(),
        },
        common::TestImage {
            width: 1,
            height: 3,
            format: 1,
            data: rgb.clone(),
        },
    ]);
    let pkg = parse(&package(SectionKind::Images, payload)).expect("parses");
    let images = pkg
        .images()
        .expect("images decode")
        .expect("images present");
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].width, 2);
    assert_eq!(images[0].height, 2);
    assert_eq!(images[0].format, ImageFormat::Rgba8);
    assert_eq!(images[0].data, rgba);
    assert_eq!(images[1].width, 1);
    assert_eq!(images[1].height, 3);
    assert_eq!(images[1].format, ImageFormat::Rgb8);
    assert_eq!(images[1].data, rgb);
}

#[test]
fn imgs_rejects_unknown_format() {
    let payload = common::imgs_payload(&[common::TestImage {
        width: 1,
        height: 1,
        format: 2,
        data: vec![0; 4],
    }]);
    let pkg = parse(&package(SectionKind::Images, payload)).expect("container parses");
    let err = pkg.images().expect_err("unknown format");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

#[test]
fn imgs_rejects_byte_len_mismatch() {
    // 2x2 RGBA8 needs 16 bytes; supply 12.
    let payload = common::imgs_payload(&[common::TestImage {
        width: 2,
        height: 2,
        format: 0,
        data: vec![0; 12],
    }]);
    let pkg = parse(&package(SectionKind::Images, payload)).expect("container parses");
    let err = pkg.images().expect_err("byte_len mismatch");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

#[test]
fn imgs_rejects_reserved_flags() {
    let mut payload = common::imgs_payload(&[common::TestImage {
        width: 1,
        height: 1,
        format: 0,
        data: vec![0; 4],
    }]);
    // The flags byte of the first record: u32 count (4) + u32 id (4) + u16 w
    // (2) + u16 h (2) + u8 format (1) = offset 13.
    let flags = payload.get_mut(13).expect("flags byte");
    *flags = 1;
    let pkg = parse(&package(SectionKind::Images, payload)).expect("container parses");
    let err = pkg.images().expect_err("reserved flags");
    assert_eq!(err.kind(), ErrorKind::InvalidFlags);
}

// ---------------------------------------------------------------------------
// §2.2 CHNK extras beyond link_target

#[test]
fn chnk_decodes_every_extra_kind() {
    let chunk = common::TestChunk {
        id: 1,
        kind: 1, // document (structural)
        flags: 0x10,
        style_id: 0,
        parent_id: 0,
        prev_id: 0,
        next_id: 0,
        first_child_id: 0,
        last_child_id: 0,
        content_index: 0,
        ordinal: 0,
        depth: 0,
    };
    // cell_span: u16 colspan + u16 rowspan.
    let cell_span = common::extra_bytes(1, 2, &[2, 0, 3, 0]);
    // list_item_value: u32 explicit ordinal.
    let list_value = common::extra_bytes(1, 3, &[5, 0, 0, 0]);
    // image_alt: raw UTF-8.
    let alt = common::extra_bytes(1, 4, b"alt text");
    // link_target: u16 url_len + UTF-8 URL.
    let url = b"https://x.test";
    let mut link_data = Vec::new();
    link_data.extend_from_slice(&(url.len() as u16).to_le_bytes());
    link_data.extend_from_slice(url);
    let link = common::extra_bytes(1, 1, &link_data);

    let payload = common::chnk_payload(&[chunk], &[cell_span, list_value, alt, link]);
    let pkg = parse(&package(SectionKind::Chunk, payload)).expect("parses");
    let chunks = pkg
        .chunks()
        .expect("chunks decode")
        .expect("chunks present");
    assert_eq!(chunks.chunks.len(), 1);
    assert_eq!(
        chunks.chunks[0].kind,
        glyphcull_core::reader::chunk::ChunkKind::Document
    );
    assert_eq!(chunks.extras.len(), 4);
    assert_eq!(chunks.extras[0].kind, ExtraKind::CellSpan);
    assert_eq!(
        chunks.extras[0].data,
        ExtraData::CellSpan {
            colspan: 2,
            rowspan: 3
        }
    );
    assert_eq!(chunks.extras[1].kind, ExtraKind::ListItemValue);
    assert_eq!(chunks.extras[1].data, ExtraData::ListItemValue { value: 5 });
    assert_eq!(chunks.extras[2].kind, ExtraKind::ImageAlt);
    assert_eq!(
        chunks.extras[2].data,
        ExtraData::ImageAlt {
            text: "alt text".to_string()
        }
    );
    assert_eq!(chunks.extras[3].kind, ExtraKind::LinkTarget);
    assert_eq!(
        chunks.extras[3].data,
        ExtraData::LinkTarget {
            url: "https://x.test".to_string()
        }
    );
}

#[test]
fn chnk_rejects_cell_span_with_a_zero_dimension() {
    let chunk = common::TestChunk {
        id: 1,
        kind: 1,
        flags: 0x10,
        style_id: 0,
        parent_id: 0,
        prev_id: 0,
        next_id: 0,
        first_child_id: 0,
        last_child_id: 0,
        content_index: 0,
        ordinal: 0,
        depth: 0,
    };
    let cell_span = common::extra_bytes(1, 2, &[0, 0, 3, 0]); // colspan 0
    let payload = common::chnk_payload(&[chunk], &[cell_span]);
    let pkg = parse(&package(SectionKind::Chunk, payload)).expect("container parses");
    let err = pkg.chunks().expect_err("zero colspan");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

#[test]
fn chnk_rejects_structural_flag_mismatch() {
    // kind 1 (document, structural) without the STRUCTURAL flag.
    let chunk = common::TestChunk {
        id: 1,
        kind: 1,
        flags: 0,
        style_id: 0,
        parent_id: 0,
        prev_id: 0,
        next_id: 0,
        first_child_id: 0,
        last_child_id: 0,
        content_index: 0,
        ordinal: 0,
        depth: 0,
    };
    let payload = common::chnk_payload(&[chunk], &[]);
    let pkg = parse(&package(SectionKind::Chunk, payload)).expect("container parses");
    let err = pkg.chunks().expect_err("flag mismatch");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

// ---------------------------------------------------------------------------
// §2.4 CONT image references

#[test]
fn cont_decodes_text_and_image_refs() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&2u32.to_le_bytes()); // payload count
                                                    // Payload 0: text.
    let text = b"hello";
    payload.extend_from_slice(&0u32.to_le_bytes()); // id
    payload.push(0); // kind: text_utf8
    payload.push(0); // flags
    payload.extend_from_slice(&0u16.to_le_bytes()); // reserved
    payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
    payload.extend_from_slice(text);
    // Payload 1: image ref.
    payload.extend_from_slice(&1u32.to_le_bytes()); // id
    payload.push(1); // kind: image_ref
    payload.push(0); // flags
    payload.extend_from_slice(&0u16.to_le_bytes()); // reserved
    payload.extend_from_slice(&4u32.to_le_bytes()); // byte_len
    payload.extend_from_slice(&7u32.to_le_bytes()); // image id

    let pkg = parse(&package(SectionKind::Content, payload)).expect("parses");
    let content = pkg
        .content()
        .expect("content decode")
        .expect("content present");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0].data, ContentData::Text("hello".to_string()));
    assert_eq!(content[1].data, ContentData::ImageRef(7));
}

#[test]
fn cont_rejects_image_ref_with_a_non_four_byte_len() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes()); // id
    payload.push(1); // kind: image_ref
    payload.push(0);
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&8u32.to_le_bytes()); // wrong byte_len
    payload.extend_from_slice(&[0, 0, 0, 7, 0, 0, 0, 0]);
    let pkg = parse(&package(SectionKind::Content, payload)).expect("container parses");
    let err = pkg.content().expect_err("bad image_ref len");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

// ---------------------------------------------------------------------------
// §2.3 STYL full property tag set

/// Append `tag` + a `u32` value to a STYL property blob.
fn tag_u32(blob: &mut Vec<u8>, tag: u16, value: u32) {
    blob.extend_from_slice(&tag.to_le_bytes());
    blob.extend_from_slice(&value.to_le_bytes());
}

/// Append `tag` + an `f32` value to a STYL property blob.
fn tag_f32(blob: &mut Vec<u8>, tag: u16, value: f32) {
    blob.extend_from_slice(&tag.to_le_bytes());
    blob.extend_from_slice(&value.to_bits().to_le_bytes());
}

/// Append `tag` + a `u16` value to a STYL property blob.
fn tag_u16(blob: &mut Vec<u8>, tag: u16, value: u16) {
    blob.extend_from_slice(&tag.to_le_bytes());
    blob.extend_from_slice(&value.to_le_bytes());
}

/// Append `tag` + a `u8` value to a STYL property blob.
fn tag_u8(blob: &mut Vec<u8>, tag: u16, value: u8) {
    blob.extend_from_slice(&tag.to_le_bytes());
    blob.push(value);
}

#[test]
fn styl_decodes_every_property_tag() {
    let mut blob = Vec::new();
    tag_u32(&mut blob, 1, 3); // font_id
    tag_f32(&mut blob, 2, 16.0); // font_size_px
    tag_f32(&mut blob, 3, 1.5); // line_height
    tag_u16(&mut blob, 4, 700); // font_weight
    tag_u8(&mut blob, 5, 1); // italic
    tag_u32(&mut blob, 6, 0x1122_3344); // color
    tag_u32(&mut blob, 7, 0x0000_0000); // background_color
    tag_f32(&mut blob, 8, 4.0); // margin_top
    tag_f32(&mut blob, 9, 8.0); // margin_bottom
    tag_u8(&mut blob, 10, 2); // text_align (end)
    tag_f32(&mut blob, 11, 24.0); // text_indent
    tag_u8(&mut blob, 12, 4); // list_style (decimal)
    tag_u8(&mut blob, 13, 1); // code
    tag_u8(&mut blob, 14, 1); // underline
    tag_f32(&mut blob, 15, 1.0); // letter_spacing
    tag_u8(&mut blob, 16, 1); // white_space (pre)

    let mut payload = Vec::new();
    payload.extend_from_slice(&1u32.to_le_bytes()); // 1 style
    payload.extend_from_slice(&0u32.to_le_bytes()); // id 0
    payload.extend_from_slice(&16u16.to_le_bytes()); // property count
    payload.extend_from_slice(&(blob.len() as u16).to_le_bytes());
    payload.extend_from_slice(&blob);

    let pkg = parse(&package(SectionKind::Style, payload)).expect("parses");
    let styles = pkg
        .styles()
        .expect("styles decode")
        .expect("styles present");
    assert_eq!(styles.len(), 1);
    let properties = &styles[0].properties;
    assert_eq!(properties.len(), 16);
    let value_of = |tag: PropertyTag| {
        properties
            .iter()
            .find(|p| p.tag == tag)
            .expect("tag present")
            .value
    };
    assert_eq!(value_of(PropertyTag::FontId), PropertyValue::U32(3));
    assert_eq!(value_of(PropertyTag::FontSizePx), PropertyValue::F32(16.0));
    assert_eq!(value_of(PropertyTag::LineHeight), PropertyValue::F32(1.5));
    assert_eq!(value_of(PropertyTag::FontWeight), PropertyValue::U16(700));
    assert_eq!(value_of(PropertyTag::Italic), PropertyValue::U8(1));
    assert_eq!(
        value_of(PropertyTag::Color),
        PropertyValue::U32(0x1122_3344)
    );
    assert_eq!(
        value_of(PropertyTag::BackgroundColor),
        PropertyValue::U32(0)
    );
    assert_eq!(value_of(PropertyTag::MarginTop), PropertyValue::F32(4.0));
    assert_eq!(value_of(PropertyTag::MarginBottom), PropertyValue::F32(8.0));
    assert_eq!(value_of(PropertyTag::TextAlign), PropertyValue::U8(2));
    assert_eq!(value_of(PropertyTag::TextIndent), PropertyValue::F32(24.0));
    assert_eq!(value_of(PropertyTag::ListStyle), PropertyValue::U8(4));
    assert_eq!(value_of(PropertyTag::Code), PropertyValue::U8(1));
    assert_eq!(value_of(PropertyTag::Underline), PropertyValue::U8(1));
    assert_eq!(
        value_of(PropertyTag::LetterSpacing),
        PropertyValue::F32(1.0)
    );
    assert_eq!(value_of(PropertyTag::WhiteSpace), PropertyValue::U8(1));
}

#[test]
fn styl_rejects_unknown_property_tags() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes()); // id
    payload.extend_from_slice(&1u16.to_le_bytes()); // 1 property
    payload.extend_from_slice(&4u16.to_le_bytes()); // blob len 4
    payload.extend_from_slice(&200u16.to_le_bytes()); // tag 200 (reserved)
    payload.extend_from_slice(&1u32.to_le_bytes());
    let pkg = parse(&package(SectionKind::Style, payload)).expect("container parses");
    let err = pkg.styles().expect_err("unknown tag");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}

// ---------------------------------------------------------------------------
// §2.5 GLYF structural validation

#[test]
fn glyf_rejects_a_glyph_box_outside_its_page() {
    // Minimal atlas: 1 glyph, page 8x8, glyph box placed out of bounds.
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u32.to_le_bytes()); // atlas count
    payload.extend_from_slice(&0u32.to_le_bytes()); // font_id
    payload.extend_from_slice(&1u32.to_le_bytes()); // glyph count
    payload.extend_from_slice(&1u16.to_le_bytes()); // page count
    payload.push(0); // format (MSDF_RGBA8)
    payload.push(0); // flags
    payload.extend_from_slice(&0u16.to_le_bytes()); // padding
    payload.extend_from_slice(&(32 * 1024u32).to_le_bytes()); // texels_per_em
    for value in [1.0f32, -0.2, 0.8, 0.7, 0.5, 1000.0] {
        payload.extend_from_slice(&value.to_bits().to_le_bytes()); // ascent..units_per_em
    }
    payload.extend_from_slice(&4u16.to_le_bytes()); // family_len
    payload.extend_from_slice(b"Ahem");
    payload.extend_from_slice(&400u16.to_le_bytes()); // weight
    payload.push(0); // italic
    payload.push(0); // reserved
    payload.extend_from_slice(&8u32.to_le_bytes()); // page_width
    payload.extend_from_slice(&8u32.to_le_bytes()); // page_height
                                                    // Glyph record (32 bytes): codepoint 97, box 4,4 8x8 (exceeds the page).
    payload.extend_from_slice(&97u32.to_le_bytes());
    payload.extend_from_slice(&1.0f32.to_bits().to_le_bytes()); // advance
    payload.extend_from_slice(&0.0f32.to_bits().to_le_bytes()); // bearing_x
    payload.extend_from_slice(&0.0f32.to_bits().to_le_bytes()); // bearing_y
    payload.extend_from_slice(&4u16.to_le_bytes()); // box_x
    payload.extend_from_slice(&4u16.to_le_bytes()); // box_y
    payload.extend_from_slice(&8u16.to_le_bytes()); // box_w
    payload.extend_from_slice(&8u16.to_le_bytes()); // box_h
    payload.extend_from_slice(&0u16.to_le_bytes()); // page_index
    payload.push(0); // glyph flags
    payload.push(0); // reserved1
    payload.extend_from_slice(&0u32.to_le_bytes()); // reserved4
    payload.extend_from_slice(&0u32.to_le_bytes()); // kerning count
    payload.extend_from_slice(&vec![0_u8; 8 * 8 * 4]); // page texels

    let pkg = parse(&package(SectionKind::Glyph, payload)).expect("container parses");
    let err = pkg.atlases().expect_err("box out of bounds");
    assert_eq!(err.kind(), ErrorKind::InvalidValue);
}
