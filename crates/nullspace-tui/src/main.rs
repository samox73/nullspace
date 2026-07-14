mod action;
mod app;
mod clipboard;
mod event;
mod graphics;
mod protocol_warm_worker;
mod render_cache;
mod render_queue;
mod tui;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use app::{AppState, ScanAgent};
use crossterm::event as ct_event;
use nullspace_core::{DuplicatePolicy, Equation, Quantity, Store};
use schemars::JsonSchema;

fn main() -> anyhow::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = tui::restore();
        default_hook(info);
    }));

    let args = Args::parse()?;
    if let Some(path) = args.export_schema_path {
        export_schema(&path)?;
        return Ok(());
    }

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
        app.scan_agent = args.scan_agent;
        if args.scan {
            app.start_scan(args.scan_agent);
        }
        run(&mut terminal, &mut app)
    })();
    tui::restore()?;
    result
}

struct Args {
    scan: bool,
    scan_agent: ScanAgent,
    export_path: Option<PathBuf>,
    import_path: Option<PathBuf>,
    export_schema_path: Option<PathBuf>,
    duplicate_policy: DuplicatePolicy,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut scan = false;
        let mut scan_agent = ScanAgent::Claude;
        let mut export_path = None;
        let mut import_path = None;
        let mut export_schema_path = None;
        let mut duplicate_policy = DuplicatePolicy::Skip;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "scan" => {
                    scan = true;
                }
                "--agent" => {
                    scan_agent = parse_scan_agent(
                        &args.next().context("--agent requires claude or codex")?,
                    )?;
                }
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
                "--export-schema" => {
                    export_schema_path = Some(PathBuf::from(
                        args.next().context("--export-schema requires a path")?,
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
                        "Usage: nullspace [scan [--agent claude|codex]] [--export PATH] [--import PATH] [--export-schema PATH] [--on-duplicate skip|overwrite]"
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
        let mode_count = export_path.is_some() as u8
            + import_path.is_some() as u8
            + export_schema_path.is_some() as u8
            + scan as u8;
        if mode_count > 1 {
            anyhow::bail!("scan, --export, --import, and --export-schema cannot be combined");
        }
        Ok(Self {
            scan,
            scan_agent,
            export_path,
            import_path,
            export_schema_path,
            duplicate_policy,
        })
    }
}

fn parse_scan_agent(raw: &str) -> anyhow::Result<ScanAgent> {
    match raw {
        "claude" => Ok(ScanAgent::Claude),
        "codex" => Ok(ScanAgent::Codex),
        other => anyhow::bail!("unknown scan agent: {other}"),
    }
}

fn parse_duplicate_policy(raw: &str) -> anyhow::Result<DuplicatePolicy> {
    match raw {
        "skip" => Ok(DuplicatePolicy::Skip),
        "overwrite" => Ok(DuplicatePolicy::Overwrite),
        other => anyhow::bail!("unknown duplicate policy: {other}"),
    }
}

#[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
struct ExportFile {
    #[serde(default)]
    quantities: Vec<Quantity>,
    equations: Vec<Equation>,
}

fn export_schema(output_path: &std::path::Path) -> anyhow::Result<()> {
    let schema = export_file_schema();
    let json = serde_json::to_string_pretty(&schema)?;
    std::fs::write(output_path, json)?;
    println!("exported schema to {}", output_path.display());
    Ok(())
}

pub(crate) fn export_file_schema() -> schemars::Schema {
    schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<ExportFile>()
}

pub(crate) fn parse_import(raw: &str) -> anyhow::Result<(Vec<Quantity>, Vec<Equation>)> {
    if let Ok(file) = serde_json::from_str::<ExportFile>(raw) {
        return Ok((file.quantities, file.equations));
    }
    let equations: Vec<Equation> = serde_json::from_str(raw)?;
    Ok((Vec::new(), equations))
}

fn export_json(db_path: &std::path::Path, output_path: &std::path::Path) -> anyhow::Result<()> {
    let store = Store::open(db_path)?;
    let quantities = store
        .quantities()?
        .into_iter()
        .map(|(quantity, _)| quantity)
        .collect::<Vec<_>>();
    let equations = store.all()?;
    let json = serde_json::to_string_pretty(&ExportFile {
        quantities: quantities.clone(),
        equations: equations.clone(),
    })?;
    std::fs::write(output_path, json)?;
    println!(
        "exported {} quantity(s), {} equation(s) to {}",
        quantities.len(),
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
    let (quantities, equations) = parse_import(&raw)?;
    let mut store = Store::open(db_path)?;
    let quantity_count = store.import_quantities(&quantities)?;
    let summary = store.import_equations(&equations, policy)?;
    println!(
        "imported {} quantity(s), {} new, updated {}, skipped {} duplicate(s) from {}",
        quantity_count,
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
    let frame_duration = Duration::from_millis(16); // ~60fps
    while !app.should_quit {
        let frame_start = std::time::Instant::now();

        while ct_event::poll(Duration::ZERO)? {
            match ct_event::read()? {
                ct_event::Event::Key(key) => {
                    let action = event::map_key(key, app);
                    app.apply(action);
                }
                ct_event::Event::Resize(_, _) | ct_event::Event::FocusGained => {
                    app.refresh_graphics_if_changed();
                }
                _ => {}
            }
        }

        app.tick_render();
        terminal.draw(|frame| ui::draw(frame, app))?;

        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
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

#[cfg(test)]
mod tests {
    use super::{export_file_schema, parse_import};
    use nullspace_core::{Equation, Quantity, Variable};

    #[test]
    fn parse_import_accepts_legacy_array() {
        let equation = Equation::new("Energy".to_string(), "E=mc^2".to_string());
        let raw = serde_json::to_string(&vec![equation]).unwrap();

        let (quantities, equations) = parse_import(&raw).unwrap();

        assert!(quantities.is_empty());
        assert_eq!(equations.len(), 1);
    }

    #[test]
    fn parse_import_accepts_export_file_object() {
        let quantity = Quantity::new("E".to_string());
        let mut equation = Equation::new("Energy".to_string(), "E=mc^2".to_string());
        equation.variables = vec![Variable {
            symbol: "E".to_string(),
            description: "energy".to_string(),
            quantity_id: Some(quantity.id),
        }];
        let raw = serde_json::json!({
            "quantities": [quantity],
            "equations": [equation],
        })
        .to_string();

        let (quantities, equations) = parse_import(&raw).unwrap();

        assert_eq!(quantities.len(), 1);
        assert_eq!(
            equations[0].variables[0].quantity_id,
            Some(quantities[0].id)
        );
    }

    #[test]
    fn export_schema_uses_serialized_export_shape() {
        let schema = serde_json::to_value(export_file_schema()).unwrap();

        assert_eq!(
            schema["required"],
            serde_json::json!(["quantities", "equations"])
        );
        assert!(
            schema["$defs"]["Equation"]["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("assumptions"))
        );
    }
}
