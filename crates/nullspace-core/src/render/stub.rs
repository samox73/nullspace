use image::{Rgba, RgbaImage};

pub fn render(latex: &str, px_height: u32) -> Result<RgbaImage, String> {
    if latex.trim().is_empty() {
        return Err("empty".to_string());
    }
    let height = px_height.max(16);
    let glyph_w = (height / 2).max(8);
    let width = (latex.chars().count() as u32 * glyph_w).max(16);
    let mut image = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    for (index, ch) in latex.chars().enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        let x0 = index as u32 * glyph_w + glyph_w / 5;
        let y0 = height / 4;
        let x1 = (x0 + glyph_w / 2).min(width);
        let y1 = (height * 3 / 4).min(height);
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgba([20, 20, 20, 255]));
            }
        }
    }
    Ok(image)
}
