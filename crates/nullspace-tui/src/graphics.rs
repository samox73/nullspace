use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::layout::Size;
use ratatui_image::{
    picker::{Capability, Picker, ProtocolType, cap_parser::QueryStdioOptions},
    protocol::StatefulProtocol,
};
use std::time::Duration;

#[derive(Clone)]
pub struct Graphics {
    picker: Picker,
    pub graphics_ok: bool,
    pub cell_size_px: TerminalCellSize,
    palette: Option<TerminalPalette>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCellSize {
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminalPalette {
    foreground: [u8; 3],
    background: [u8; 3],
}

impl Graphics {
    pub fn detect() -> Self {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        Self::from_picker(picker)
    }

    pub fn probe(timeout: Duration) -> Option<Self> {
        let picker = Picker::from_query_stdio_with_options(QueryStdioOptions {
            timeout,
            ..QueryStdioOptions::default()
        })
        .ok()?;
        if !picker.capabilities().iter().any(|capability| {
            matches!(capability, Capability::CellSize(Some((width, height))) if *width > 0 && *height > 0)
        }) {
            return None;
        }
        Some(Self::from_picker(picker))
    }

    fn from_picker(picker: Picker) -> Self {
        let graphics_ok =
            picker.protocol_type() != ProtocolType::Halfblocks || terminal_graphics_detected();
        let font_size = picker.font_size();
        let palette = query_terminal_palette();
        Self {
            picker,
            graphics_ok,
            cell_size_px: TerminalCellSize {
                width: font_size.width,
                height: font_size.height,
            },
            palette,
        }
    }

    #[cfg(test)]
    pub(crate) fn test(cell_size_px: TerminalCellSize) -> Self {
        Self {
            picker: Picker::halfblocks(),
            graphics_ok: false,
            cell_size_px,
            palette: None,
        }
    }

    pub fn recolor(&self, image: RgbaImage) -> RgbaImage {
        if let Some(palette) = self.palette {
            recolor_image(image, palette)
        } else {
            image
        }
    }

    pub fn protocol_from(&self, image: RgbaImage, available: Size) -> StatefulProtocol {
        let image = center_visible_content_in_cell_rows(image, self.cell_size_px, available.height);
        self.picker
            .new_resize_protocol(DynamicImage::ImageRgba8(image))
    }
}

fn center_visible_content_in_cell_rows(
    image: RgbaImage,
    cell_size_px: TerminalCellSize,
    available_rows: u16,
) -> RgbaImage {
    let Some((top, bottom)) = visible_vertical_bounds(&image) else {
        return image;
    };
    let content_height = bottom - top + 1;
    let target_height = centered_cell_row_height_px(content_height, cell_size_px, available_rows);

    let background = image
        .get_pixel_checked(0, 0)
        .copied()
        .unwrap_or(Rgba([255, 255, 255, 255]));
    let top_padding = target_height.saturating_sub(content_height) / 2;
    let mut padded = RgbaImage::from_pixel(image.width(), target_height, background);
    for y in top..=bottom {
        let target_y = top_padding + (y - top);
        for x in 0..image.width() {
            padded.put_pixel(x, target_y, *image.get_pixel(x, y));
        }
    }
    padded
}

fn centered_cell_row_height_px(
    image_height: u32,
    cell_size_px: TerminalCellSize,
    available_rows: u16,
) -> u32 {
    let cell_height = u32::from(cell_size_px.height);
    if image_height == 0 || cell_height == 0 {
        return image_height;
    }

    let mut rows = image_height.div_ceil(cell_height);
    let available_rows = u32::from(available_rows);
    if available_rows >= rows {
        rows = available_rows;
    } else if image_height.is_multiple_of(cell_height) && rows % 2 == 0 {
        rows += 1;
    }
    rows.saturating_mul(cell_height)
}

fn visible_vertical_bounds(image: &RgbaImage) -> Option<(u32, u32)> {
    let background = image.get_pixel_checked(0, 0)?;
    let mut top = None;
    let mut bottom = None;
    for (_, y, pixel) in image.enumerate_pixels() {
        if !matches_background(pixel, background) {
            top = Some(top.map_or(y, |current: u32| current.min(y)));
            bottom = Some(bottom.map_or(y, |current: u32| current.max(y)));
        }
    }
    top.zip(bottom)
}

fn matches_background(pixel: &Rgba<u8>, background: &Rgba<u8>) -> bool {
    pixel
        .0
        .iter()
        .zip(background.0)
        .all(|(pixel, background)| pixel.abs_diff(background) <= BACKGROUND_MATCH_TOLERANCE)
}

const BACKGROUND_MATCH_TOLERANCE: u8 = 4;

fn recolor_image(mut image: RgbaImage, palette: TerminalPalette) -> RgbaImage {
    for pixel in image.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        if alpha == 0 {
            continue;
        }
        let ink = 255 - luminance(red, green, blue);
        let amount = ink as u16;
        let inverse = 255_u16.saturating_sub(amount);
        *pixel = Rgba([
            blend_channel(
                palette.background[0],
                palette.foreground[0],
                amount,
                inverse,
            ),
            blend_channel(
                palette.background[1],
                palette.foreground[1],
                amount,
                inverse,
            ),
            blend_channel(
                palette.background[2],
                palette.foreground[2],
                amount,
                inverse,
            ),
            alpha,
        ]);
    }
    image
}

fn luminance(red: u8, green: u8, blue: u8) -> u8 {
    ((red as u16 * 77 + green as u16 * 150 + blue as u16 * 29) / 256) as u8
}

fn blend_channel(background: u8, foreground: u8, amount: u16, inverse: u16) -> u8 {
    ((background as u16 * inverse + foreground as u16 * amount) / 255) as u8
}

fn query_terminal_palette() -> Option<TerminalPalette> {
    let response = query_osc_colors()?;
    Some(TerminalPalette {
        foreground: parse_osc_color(&response, "10")?,
        background: parse_osc_color(&response, "11")?,
    })
}

#[cfg(unix)]
fn query_osc_colors() -> Option<String> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\").ok()?;
    stdout.flush().ok()?;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let mut input = stdin.lock();
    let started = Instant::now();
    let timeout = Duration::from_millis(180);
    let mut bytes = Vec::new();

    while started.elapsed() < timeout {
        let remaining = timeout.saturating_sub(started.elapsed());
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if ready <= 0 || poll_fd.revents & libc::POLLIN == 0 {
            break;
        }

        let mut chunk = [0_u8; 256];
        let read = input.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if contains_osc_terminator(&bytes, b"10") && contains_osc_terminator(&bytes, b"11") {
            break;
        }
    }

    String::from_utf8(bytes).ok()
}

#[cfg(not(unix))]
fn query_osc_colors() -> Option<String> {
    None
}

fn contains_osc_terminator(response: &[u8], code: &[u8]) -> bool {
    let needle = [b"\x1b]".as_slice(), code, b";"].concat();
    response
        .windows(needle.len())
        .position(|window| window == needle)
        .and_then(|start| find_osc_end(response, start + needle.len()))
        .is_some()
}

fn parse_osc_color(response: &str, code: &str) -> Option<[u8; 3]> {
    let prefix = format!("\x1b]{code};");
    let start = response.find(&prefix)? + prefix.len();
    let end = find_osc_end(response.as_bytes(), start)?;
    parse_color_spec(&response[start..end])
}

fn find_osc_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\x07' => return Some(index),
            b'\x1b' if bytes.get(index + 1) == Some(&b'\\') => return Some(index),
            _ => index += 1,
        }
    }
    None
}

fn parse_color_spec(spec: &str) -> Option<[u8; 3]> {
    let rgb = spec
        .strip_prefix("rgb:")
        .or_else(|| spec.strip_prefix("rgba:"))?;
    let mut parts = rgb.split('/');
    Some([
        parse_hex_component(parts.next()?)?,
        parse_hex_component(parts.next()?)?,
        parse_hex_component(parts.next()?)?,
    ])
}

fn parse_hex_component(component: &str) -> Option<u8> {
    if component.is_empty() || component.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let max = (1_u32 << (component.len() * 4)) - 1;
    Some(((value * 255 + max / 2) / max) as u8)
}

fn terminal_graphics_detected() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
        || std::env::var("TERM")
            .map(|term| {
                term.contains("kitty") || term.contains("wezterm") || term.contains("xterm-kitty")
            })
            .unwrap_or(false)
        || std::env::var("TERM_PROGRAM")
            .map(|program| program.contains("iTerm") || program.contains("Ghostty"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xterm_rgb_color_response() {
        let response = "\x1b]10;rgb:ffff/eeee/dddd\x1b\\\x1b]11;rgb:1111/2222/3333\x07";
        assert_eq!(parse_osc_color(response, "10"), Some([255, 238, 221]));
        assert_eq!(parse_osc_color(response, "11"), Some([17, 34, 51]));
    }

    #[test]
    fn recolor_maps_white_to_background_and_black_to_foreground() {
        let palette = TerminalPalette {
            foreground: [230, 231, 232],
            background: [10, 11, 12],
        };
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        image.put_pixel(1, 0, Rgba([0, 0, 0, 255]));

        let recolored = recolor_image(image, palette);

        assert_eq!(recolored.get_pixel(0, 0).0, [10, 11, 12, 255]);
        assert_eq!(recolored.get_pixel(1, 0).0, [230, 231, 232, 255]);
    }

    #[test]
    fn fractional_cell_height_gets_balanced_padding() {
        let mut image = RgbaImage::from_pixel(3, 100, Rgba([255, 255, 255, 255]));
        for y in 40..74 {
            image.put_pixel(1, y, Rgba([0, 0, 0, 255]));
        }

        let padded = center_visible_content_in_cell_rows(
            image,
            TerminalCellSize {
                width: 10,
                height: 20,
            },
            0,
        );

        assert_eq!(padded.height(), 40);
        assert_eq!(padded.get_pixel(1, 3).0, [0, 0, 0, 255]);
        assert_eq!(padded.get_pixel(1, 36).0, [0, 0, 0, 255]);
    }

    #[test]
    fn exact_even_cell_height_promotes_to_odd_cell_count() {
        assert_eq!(
            centered_cell_row_height_px(
                40,
                TerminalCellSize {
                    width: 10,
                    height: 20,
                },
                0
            ),
            60
        );
        assert_eq!(
            centered_cell_row_height_px(
                80,
                TerminalCellSize {
                    width: 10,
                    height: 20,
                },
                0
            ),
            100
        );
    }

    #[test]
    fn exact_even_cell_content_is_centered_in_promoted_cell_count() {
        let mut image = RgbaImage::from_pixel(3, 100, Rgba([255, 255, 255, 255]));
        for y in 0..80 {
            image.put_pixel(1, y, Rgba([0, 0, 0, 255]));
        }

        let padded = center_visible_content_in_cell_rows(
            image,
            TerminalCellSize {
                width: 10,
                height: 20,
            },
            0,
        );

        assert_eq!(padded.height(), 100);
        assert_eq!(padded.get_pixel(1, 9).0, [255, 255, 255, 255]);
        assert_eq!(padded.get_pixel(1, 10).0, [0, 0, 0, 255]);
        assert_eq!(padded.get_pixel(1, 89).0, [0, 0, 0, 255]);
        assert_eq!(padded.get_pixel(1, 90).0, [255, 255, 255, 255]);
    }

    #[test]
    fn exact_odd_cell_height_stays_on_its_middle_row() {
        assert_eq!(
            centered_cell_row_height_px(
                60,
                TerminalCellSize {
                    width: 10,
                    height: 20,
                },
                0
            ),
            60
        );
    }

    #[test]
    fn content_is_centered_in_available_rows() {
        let mut image = RgbaImage::from_pixel(3, 100, Rgba([255, 255, 255, 255]));
        for y in 0..73 {
            image.put_pixel(1, y, Rgba([0, 0, 0, 255]));
        }

        let padded = center_visible_content_in_cell_rows(
            image,
            TerminalCellSize {
                width: 10,
                height: 20,
            },
            5,
        );

        assert_eq!(padded.height(), 100);
        assert_eq!(padded.get_pixel(1, 12).0, [255, 255, 255, 255]);
        assert_eq!(padded.get_pixel(1, 13).0, [0, 0, 0, 255]);
        assert_eq!(padded.get_pixel(1, 85).0, [0, 0, 0, 255]);
        assert_eq!(padded.get_pixel(1, 86).0, [255, 255, 255, 255]);
    }
}
