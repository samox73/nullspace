use image::RgbaImage;
use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_parser::parser::parse;
use ratex_svg::{render_to_svg, SvgOptions};
use ratex_types::{color::Color, math_style::MathStyle};
use resvg::{tiny_skia, usvg};

pub fn validate(latex: &str) -> Result<(), String> {
    let latex = latex.trim();
    if latex.is_empty() {
        return Err("empty".to_string());
    }
    parse(latex)
        .map(|_| ())
        .map_err(|err| format!("parse error: {err}"))
}

pub fn render(latex: &str, px_height: u32) -> Result<RgbaImage, String> {
    let latex = latex.trim();
    if latex.is_empty() {
        return Err("empty".to_string());
    }

    let svg = latex_to_svg(latex)?;
    rasterize_svg(&svg, px_height.max(16))
}

fn latex_to_svg(latex: &str) -> Result<String, String> {
    let ast = parse(latex).map_err(|err| format!("parse error: {err}"))?;
    let layout_opts = LayoutOptions::default()
        .with_style(MathStyle::Display)
        .with_color(Color::BLACK);
    let layout_box = layout(&ast, &layout_opts);
    let display_list = to_display_list(&layout_box);
    let svg_opts = SvgOptions {
        font_size: 40.0,
        padding: 8.0,
        stroke_width: 1.5,
        embed_glyphs: true,
        font_dir: String::new(),
    };
    Ok(render_to_svg(&display_list, &svg_opts))
}

fn rasterize_svg(svg: &str, target_height: u32) -> Result<RgbaImage, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
        .map_err(|err| format!("svg parse error: {err}"))?;
    let size = tree.size();
    let source_width = size.width().max(1.0);
    let source_height = size.height().max(1.0);
    let scale = target_height as f32 / source_height;
    let width = (source_width * scale).ceil().max(1.0) as u32;
    let height = (source_height * scale).ceil().max(1.0) as u32;
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| "pixmap alloc failed".to_string())?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut pixmap_mut = pixmap.as_mut();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap_mut,
    );
    RgbaImage::from_raw(width, height, pixmap.data().to_vec())
        .ok_or_else(|| "invalid raster buffer".to_string())
}
