// Adapted from https://github.com/sotrh/learn-wgpu
// by Benjamin Hansen, licensed under the MIT license

use super::rect::Rect;
use crate::graphics::colors::CODE_COLOR;
use crate::graphics::style::CODE_FONT_SIZE;
use ab_glyph::{FontArc, Glyph, InvalidFont};
use cgmath::{Vector2, Vector4};
use itertools::Itertools;
use wgpu_glyph::{ab_glyph, GlyphBrush, GlyphBrushBuilder, GlyphCruncher, Section};
use bumpalo::collections::Vec as BumpVec;
use bumpalo::Bump;

#[derive(Debug)]
pub struct Text {
    pub position: Vector2<f32>,
    pub area_bounds: Vector2<f32>,
    pub color: Vector4<f32>,
    pub text: String,
    pub size: f32,
    pub visible: bool,
    pub centered: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0).into(),
            area_bounds: (std::f32::INFINITY, std::f32::INFINITY).into(),
            color: (1.0, 1.0, 1.0, 1.0).into(),
            text: String::new(),
            size: CODE_FONT_SIZE,
            visible: true,
            centered: false,
        }
    }
}

// necessary to get dimensions for caret
pub fn example_code_glyph_rect(glyph_brush: &mut GlyphBrush<()>) -> Rect {
    let code_text = Text {
        position: (30.0, 90.0).into(), //TODO 30.0 90.0 should be an arg
        area_bounds: (std::f32::INFINITY, std::f32::INFINITY).into(),
        color: CODE_COLOR.into(),
        text: "a".to_owned(),
        size: CODE_FONT_SIZE,
        ..Default::default()
    };

    let layout = layout_from_text(&code_text);

    let section = section_from_text(&code_text, layout);

    let mut glyph_section_iter = glyph_brush.glyphs_custom_layout(section, &layout);

    if let Some(glyph) = glyph_section_iter.next() {
        glyph_to_rect(glyph)
    } else {
        unreachable!();
    }
}

fn layout_from_text(text: &Text) -> wgpu_glyph::Layout<wgpu_glyph::BuiltInLineBreaker> {
    wgpu_glyph::Layout::default().h_align(if text.centered {
        wgpu_glyph::HorizontalAlign::Center
    } else {
        wgpu_glyph::HorizontalAlign::Left
    })
}

fn section_from_text(
    text: &Text,
    layout: wgpu_glyph::Layout<wgpu_glyph::BuiltInLineBreaker>,
) -> wgpu_glyph::Section {
    Section {
        screen_position: text.position.into(),
        bounds: text.area_bounds.into(),
        layout,
        ..Section::default()
    }
    .add_text(
        wgpu_glyph::Text::new(&text.text)
            .with_color(text.color)
            .with_scale(text.size),
    )
}

// returns glyphs per line
pub fn queue_text_draw<'a>(text: &Text, glyph_brush: &mut GlyphBrush<()>, arena: &'a Bump, selectable: bool) -> Option<BumpVec<'a, usize>> {
    let layout = layout_from_text(text);

    let section = section_from_text(text, layout);

    glyph_brush.queue(section.clone());

    if selectable {
        let mut glyphs_per_line: BumpVec<usize> = BumpVec::new_in(arena);

        let glyph_section_iter = glyph_brush.glyphs_custom_layout(section, &layout);
    
        let first_glyph_opt = glyph_section_iter.next();
    
        if let Some(first_glyph) = first_glyph_opt {
            let mut line_y_coord = first_glyph.glyph.scale.y;
            let mut glyphs_on_line = 0;
    
            for glyph in glyph_section_iter {
                let curr_y_coord = glyph.glyph.scale.y;
                if curr_y_coord != line_y_coord {
                    line_y_coord = curr_y_coord;
                    glyphs_per_line.push(glyphs_on_line);
                    glyphs_on_line = 0;
                } else {
                    glyphs_on_line += 1;
                }
            }
        }
    
        Some(glyphs_per_line)
    } else {
        None
    }
}

fn glyph_to_rect(glyph: &wgpu_glyph::SectionGlyph) -> Rect {
    let position = glyph.glyph.position;
    let px_scale = glyph.glyph.scale;
    let width = glyph_width(&glyph.glyph);
    let height = px_scale.y;
    let top_y = glyph_top_y(&glyph.glyph);

    Rect {
        top_left_coords: [position.x, top_y].into(),
        width,
        height,
        color: [1.0, 1.0, 1.0],
    }
}

pub fn glyph_top_y(glyph: &Glyph) -> f32 {
    let height = glyph.scale.y;

    glyph.position.y - height * 0.75
}

pub fn glyph_width(glyph: &Glyph) -> f32 {
    glyph.scale.x * 0.5
}

pub fn build_glyph_brush(
    gpu_device: &wgpu::Device,
    render_format: wgpu::TextureFormat,
) -> Result<GlyphBrush<()>, InvalidFont> {
    let inconsolata = FontArc::try_from_slice(include_bytes!("../../../Inconsolata-Regular.ttf"))?;

    Ok(GlyphBrushBuilder::using_font(inconsolata).build(&gpu_device, render_format))
}
