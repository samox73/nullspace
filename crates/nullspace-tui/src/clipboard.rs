use std::io::{self, Write};

use base64::Engine;

pub fn copy_text(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(osc52_sequence(text).as_bytes())?;
    stdout.flush()
}

fn osc52_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_sequence_encodes_text() {
        assert_eq!(osc52_sequence("E = mc^2"), "\x1b]52;c;RSA9IG1jXjI=\x07");
    }
}
