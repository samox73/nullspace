mod action;
mod app;
mod clipboard;
mod event;
mod graphics;
mod protocol_warm_worker;
mod render_cache;
mod render_worker;
mod tui;
mod ui;
mod warm_worker;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use app::AppState;
use crossterm::event as ct_event;
use nullspace_core::{DuplicatePolicy, Equation, Store};

fn main() -> anyhow::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = tui::restore();
        default_hook(info);
    }));

    let args = Args::parse()?;
    let db_path = db_path()?;
    if let Some(path) = args.export_path {
        export_json(&db_path, &path)?;
        return Ok(());
    }
    if let Some(path) = args.import_path {
        import_json(&db_path, &path, args.duplicate_policy)?;
        return Ok(());
    }

    let mut terminal = tui::init()?;
    let result = (|| {
        let graphics = graphics::Graphics::detect();
        let mut app = AppState::open(&db_path, graphics)?;
        run(&mut terminal, &mut app)
    })();
    tui::restore()?;
    result
}

struct Args {
    export_path: Option<PathBuf>,
    import_path: Option<PathBuf>,
    duplicate_policy: DuplicatePolicy,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut export_path = None;
        let mut import_path = None;
        let mut duplicate_policy = DuplicatePolicy::Skip;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--export" => {
                    export_path = Some(PathBuf::from(
                        args.next().context("--export requires a path")?,
                    ));
                }
                "--import" => {
                    import_path = Some(PathBuf::from(
                        args.next().context("--import requires a path")?,
                    ));
                }
                "--on-duplicate" => {
                    duplicate_policy = parse_duplicate_policy(
                        &args
                            .next()
                            .context("--on-duplicate requires skip or overwrite")?,
                    )?;
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: nullspace [--export PATH] [--import PATH] [--on-duplicate skip|overwrite]"
                    );
                    std::process::exit(0);
                }
                other if other.starts_with("--on-duplicate=") => {
                    duplicate_policy =
                        parse_duplicate_policy(other.trim_start_matches("--on-duplicate="))?;
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }
        if export_path.is_some() && import_path.is_some() {
            anyhow::bail!("--export and --import cannot be used together");
        }
        Ok(Self {
            export_path,
            import_path,
            duplicate_policy,
        })
    }
}

fn parse_duplicate_policy(raw: &str) -> anyhow::Result<DuplicatePolicy> {
    match raw {
        "skip" => Ok(DuplicatePolicy::Skip),
        "overwrite" => Ok(DuplicatePolicy::Overwrite),
        other => anyhow::bail!("unknown duplicate policy: {other}"),
    }
}

fn export_json(db_path: &std::path::Path, output_path: &std::path::Path) -> anyhow::Result<()> {
    let store = Store::open(db_path)?;
    let equations = store.all()?;
    let json = serde_json::to_string_pretty(&equations)?;
    std::fs::write(output_path, json)?;
    println!(
        "exported {} equation(s) to {}",
        equations.len(),
        output_path.display()
    );
    Ok(())
}

fn import_json(
    db_path: &std::path::Path,
    input_path: &std::path::Path,
    policy: DuplicatePolicy,
) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(input_path)?;
    let equations: Vec<Equation> = serde_json::from_str(&raw)?;
    let mut store = Store::open(db_path)?;
    let summary = store.import_equations(&equations, policy)?;
    println!(
        "imported {} new, updated {}, skipped {} duplicate(s) from {}",
        summary.inserted,
        summary.updated,
        summary.skipped,
        input_path.display()
    );
    Ok(())
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
