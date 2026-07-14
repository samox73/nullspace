use std::io::{self, Stdout};

use crossterm::{
    event::{DisableFocusChange, EnableFocusChange},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub fn init() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableFocusChange)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

pub fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, DisableFocusChange, LeaveAlternateScreen)?;
    disable_raw_mode()
}
