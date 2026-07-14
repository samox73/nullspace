# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2021 Cargo workspace with two members under `crates/`.
`crates/nullspace-core` contains the data model, SQLite store, identity logic, and equation rendering backends. `crates/nullspace-tui` contains the terminal UI, input handling, render workers, graphics integration, and the `nullspace` binary. Demo data lives in `demo/`, `demo.md`, and `equations.json`; keep `target/` out of commits.

## Build, Test, and Development Commands

- `cargo run -p nullspace-tui`: run the terminal app with the default local library database.
- `cargo run -p nullspace-tui -- scan`: scan an equation image from the clipboard.
- `make all`: shorthand for launching the TUI.
- `NULLSPACE_DB=/tmp/nullspace.sqlite3 cargo run -p nullspace-tui`: run against an isolated database.
- `cargo test`: run all workspace unit tests.
- `cargo check`: type-check the workspace without producing release artifacts.
- `cargo fmt --all`: format all Rust code with rustfmt.
- `cargo clippy --workspace --all-targets`: run lint checks before submitting changes.
- `make export` / `make import`: export from or import to `equations.json`.
- `make demo`: reset `demo/nullspace-demo.sqlite3`, import `demo/solid-state-physics.json`, and launch the release TUI.

## Coding Style & Naming Conventions

Use standard rustfmt output: four-space indentation, idiomatic line wrapping, and organized imports. Prefer Rust naming: `snake_case` for modules, functions, variables, and tests; `PascalCase` for types and enum variants; `SCREAMING_SNAKE_CASE` for constants. Keep storage and rendering behavior in `nullspace-core`; keep terminal interaction and UI state in `nullspace-tui`.

## Testing Guidelines

Most tests are inline `#[cfg(test)]` modules in files such as `identity.rs`, `render/mod.rs`, `store/mod.rs`, `app.rs`, and `graphics.rs`. Add focused unit tests next to the code they exercise, using names like `import_skips_duplicate_equations`. Run `cargo test` for behavior changes, or `cargo test -p nullspace-core` for crate-specific edits.

## Commit & Pull Request Guidelines

Recent commits use short imperative summaries, sometimes with a conventional prefix, for example `fix: multi-line textbox dynamic resize`, `add deduplication`, or `perf: fix several caching/render issues`. Keep subjects concise and explain user-visible behavior in the body when needed.

Pull requests should describe the change, list validation commands run, and call out data migrations, database behavior, rendering changes, or terminal compatibility concerns. Include screenshots or recordings for visible TUI changes.

## Security & Configuration Tips

Use `NULLSPACE_DB` for disposable test databases instead of mutating a personal library. Treat imported JSON as external input; validate parsing, duplicate handling, and error paths when changing import/export logic.
