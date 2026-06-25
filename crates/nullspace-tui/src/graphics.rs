use image::{DynamicImage, Rgba, RgbaImage};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};

pub struct Graphics {
    picker: Picker,
    pub graphics_ok: bool,
    pub cell_size_px: TerminalCellSize,
    palette: Option<TerminalPalette>,
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalCellSize {
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
        let graphics_ok =
            picker.protocol_type() != ProtocolType::Halfblocks || terminal_graphics_detected();
        let font_size = picker.font_size();
        let palette = query_terminal_palette();
        Self {
            picker,
            graphics_ok,
            cell_size_px: TerminalCellSize {
                height: font_size.height,
            },
            palette,
        }
    }

    pub fn recolor(&self, image: RgbaImage) -> RgbaImage {
        if let Some(palette) = self.palette {
            recolor_image(image, palette)
        } else {
            image
        }
    }

    pub fn protocol_from(&self, image: RgbaImage) -> StatefulProtocol {
        self.picker
            .new_resize_protocol(DynamicImage::ImageRgba8(image))
    }
}

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
}
