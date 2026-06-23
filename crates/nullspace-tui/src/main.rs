mod action;
mod app;
mod event;
mod render_worker;
mod tui;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use app::AppState;
use crossterm::event as ct_event;

fn main() -> anyhow::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = tui::restore();
        default_hook(info);
    }));

    let db_path = db_path()?;
    let mut app = AppState::open(&db_path)?;
    let mut terminal = tui::init()?;
    let result = run(&mut terminal, &mut app);
    tui::restore()?;
    result
}

fn run(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut AppState,
) -> anyhow::Result<()> {
    while !app.should_quit {
        app.tick_render();
        terminal.draw(|frame| ui::draw(frame, app))?;
        if ct_event::poll(Duration::from_millis(50))? {
            if let ct_event::Event::Key(key) = ct_event::read()? {
                let action = event::map_key(key, &app.mode);
                app.apply(action);
            }
        }
    }
    Ok(())
}

fn db_path() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("NULLSPACE_DB") {
        return Ok(PathBuf::from(path));
    }
    let project_dirs = directories::ProjectDirs::from("dev", "nullspace", "Nullspace")
        .context("could not determine data directory")?;
    let dir = project_dirs.data_dir();
    std::fs::create_dir_all(dir)?;
    Ok(dir.join("nullspace.sqlite3"))
}
