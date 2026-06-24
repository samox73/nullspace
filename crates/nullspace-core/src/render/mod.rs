mod ratex;
#[allow(dead_code)]
mod stub;
mod unicode;

use image::RgbaImage;

pub use unicode::to_unicode_approx;

pub fn validate_latex(latex: &str) -> Result<(), String> {
    ratex::validate(latex)
}

pub fn render_image(latex: &str, px_height: u32) -> Result<RgbaImage, String> {
    ratex::render(latex, px_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_renderer_renders_nonempty_image() {
        let image = render_image("E = mc^2", 48).unwrap();
        assert!(image.width() > 0);
        assert!(image.height() > 0);
        assert!(image.pixels().any(|pixel| pixel.0 != [255, 255, 255, 255]));
    }

    #[test]
    fn active_renderer_empty_is_err() {
        assert!(render_image("   ", 48).is_err());
    }

    #[test]
    fn active_renderer_invalid_latex_is_err() {
        assert!(render_image("\\frac{", 48).is_err());
    }

    #[test]
    fn stub_renders_nonempty_image() {
        let image = stub::render("E = mc^2", 48).unwrap();
        assert!(image.width() > 0);
        assert!(image.height() > 0);
    }

    #[test]
    fn unicode_alpha() {
        let rendered = to_unicode_approx("\\alpha + \\beta");
        assert!(rendered.contains('α'));
        assert!(rendered.contains('β'));
    }

    #[test]
    fn unicode_superscript() {
        assert!(to_unicode_approx("x^2").contains('²'));
    }
}
