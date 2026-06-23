# EquiVault — Implementation Plan (executable checklist)

A terminal (TUI) app in Rust to store, browse, and edit equations written in LaTeX, with
live in-terminal rendering on a graphics-capable terminal (Kitty / WezTerm / Ghostty /
foot / iTerm2 / Sixel-xterm).

> **This plan is written to be executed step by step, including by a lower-capability
> model.** Build in the phase order given. Do **not** skip a phase's "Definition of Done"
> (DoD) gate. Each phase ends with exact `cargo` commands that must succeed before moving
> on. When a code block says "exact", type it as written; when it says "reference", adapt
> it but keep the given function signatures — other code depends on them.

---

## 0. How to work (read first)

- **Toolchain:** stable Rust, edition 2021. Run `rustc --version` ≥ 1.78.
- **One phase at a time.** After each phase, run its DoD commands. If they fail, fix
  before continuing. Never start phase N+1 with phase N red.
- **Compile-and-fix loop for version-sensitive APIs:** `ratatui` 0.30, `ratatui-image`
  2.x, and `tui-textarea` evolve. If a call doesn't compile, run
  `cargo doc -p <crate> --open` (or read https://docs.rs/<crate>) and adjust the call —
  **do not change the function signatures this plan defines**, only the body.
- **Never change `latex` handling:** LaTeX text is the canonical source of truth. Images
  and unicode are always derived, never stored as truth.
- **Commit after each green phase** (if the user initialises git): `phase N: <summary>`.
- **Keep `equivault-core` free of `ratatui`, `crossterm`, `ratatui-image`,
  `tui-textarea`.** Only `String`, `image::RgbaImage`, and the domain types defined here
  may cross the boundary. This is a hard rule; a future GUI depends on it.

### Pinned versions (set in Cargo.toml; bump only if a version fails to resolve)

| crate | version | where |
|---|---|---|
| `rusqlite` (feature `bundled`) | `0.32` | core |
| `uuid` (feature `v4`) | `1` | core |
| `time` (features `formatting`,`parsing`) | `0.3` | core |
| `thiserror` | `1` | core |
| `serde` (feature `derive`), `serde_json` | `1` | core (import/export only) |
| `image` | `0.25` | core + tui |
| `resvg` | `0.44` | core (RaTeX phase) |
| `usvg` | `0.44` | core (RaTeX phase) |
| `tiny-skia` | `0.11` | core (RaTeX phase) |
| `ratex-svg` | `*` (0.0.x) | core (RaTeX phase, isolated) |
| `ratatui` | `0.30` | tui |
| `crossterm` | `0.28` | tui |
| `ratatui-image` | `2` | tui |
| `tui-textarea` (feature `crossterm`) | `0.7` | tui |
| `directories` | `5` | tui |
| `anyhow` | `1` | tui |

> Keep `resvg`/`usvg` versions identical to each other. If `0.44` doesn't resolve, pick
> the newest matching pair and keep both equal.

---

## 1. Workspace layout (target end state)

```
equivault/
├─ Cargo.toml                  # [workspace] only
├─ PLAN.md
├─ crates/
│  ├─ equivault-core/
│  │  ├─ Cargo.toml
│  │  └─ src/
│  │     ├─ lib.rs             # pub use of model, store, render, error
│  │     ├─ error.rs
│  │     ├─ model.rs
│  │     ├─ store/
│  │     │  ├─ mod.rs          # Store + CRUD
│  │     │  └─ migrations.rs
│  │     └─ render/
│  │        ├─ mod.rs          # render_image(), to_unicode_approx()
│  │        ├─ stub.rs         # placeholder renderer (Phase 2)
│  │        ├─ ratex.rs        # real renderer (Phase 7, isolated)
│  │        └─ unicode.rs      # latex -> unicode table
│  └─ equivault-tui/
│     ├─ Cargo.toml
│     └─ src/
│        ├─ main.rs            # entry, CLI/db path, run()
│        ├─ tui.rs             # terminal init/teardown + panic hook
│        ├─ app.rs            # AppState, Mode, update logic
│        ├─ action.rs          # Action enum
│        ├─ event.rs           # crossterm event -> Action (mode-aware keymap)
│        ├─ graphics.rs        # ratatui-image Picker + warning
│        ├─ render_worker.rs   # background render thread + debounce + LRU cache
│        └─ ui/
│           ├─ mod.rs          # draw(frame, app) dispatch
│           ├─ browser.rs
│           ├─ detail.rs
│           ├─ editor.rs
│           └─ widgets.rs      # shared: help/status bar, latex image pane
```

---

## 2. Phase 0 — Workspace skeleton

**Goal:** an empty two-crate workspace that builds.

### Files (exact)

`Cargo.toml` (workspace root):
```toml
[workspace]
resolver = "2"
members = ["crates/equivault-core", "crates/equivault-tui"]
```

`crates/equivault-core/Cargo.toml`:
```toml
[package]
name = "equivault-core"
version = "0.1.0"
edition = "2021"

[dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
time = { version = "0.3", features = ["formatting", "parsing"] }
thiserror = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
image = "0.25"
# render backend deps (resvg/usvg/tiny-skia/ratex-svg) are added in Phase 7.
```

`crates/equivault-core/src/lib.rs`:
```rust
pub mod error;
pub mod model;
pub mod render;
pub mod store;

pub use error::Error;
pub use model::{Equation, EquationId, EquationSummary, Reference, Variable};
pub use store::Store;
```

`crates/equivault-tui/Cargo.toml`:
```toml
[package]
name = "equivault-tui"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "equivault"
path = "src/main.rs"

[dependencies]
equivault-core = { path = "../equivault-core" }
ratatui = "0.30"
crossterm = "0.28"
ratatui-image = "2"
tui-textarea = { version = "0.7", features = ["crossterm"] }
directories = "5"
anyhow = "1"
image = "0.25"
```

`crates/equivault-tui/src/main.rs` (temporary):
```rust
fn main() { println!("equivault skeleton"); }
```

Create empty-but-valid module files so `lib.rs` compiles:
- `error.rs`, `model.rs`, `store/mod.rs`, `store/migrations.rs`,
  `render/mod.rs`, `render/stub.rs`, `render/unicode.rs` — each may start with `//!`.
- `render/mod.rs` must contain `mod stub; mod unicode;` once those exist (Phase 2).
- `store/mod.rs` must contain `mod migrations;` (Phase 1).

### DoD
```
cargo build --workspace
```
Must succeed. `cargo run -p equivault-tui` prints "equivault skeleton".

---

## 3. Phase 1 — Core model + SQLite store (headless, fully tested)

**Goal:** all domain types and full CRUD with transactions, with passing unit tests. No
TUI, no rendering.

### 3.1 `error.rs` (exact)
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("equation not found: {0}")]
    NotFound(String),
    #[error("render error: {0}")]
    Render(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

### 3.2 `model.rs` (exact)
```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EquationId(pub Uuid);

impl EquationId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
    pub fn to_string(&self) -> String { self.0.to_string() }
    pub fn parse(s: &str) -> Option<Self> { Uuid::parse_str(s).ok().map(Self) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub symbol: String,       // e.g. "e"
    pub description: String,   // e.g. "elementary charge"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Equation {
    pub id: EquationId,
    pub name: String,
    pub description: String,
    pub latex: String,                 // canonical source of truth
    pub references: Vec<Reference>,
    pub tags: Vec<String>,
    pub variables: Vec<Variable>,      // ordered symbol -> description
    pub related: Vec<EquationId>,      // bidirectional links by id
    pub created_at: String,            // RFC3339
    pub updated_at: String,            // RFC3339
}

/// Lightweight row for the browser list (no child tables loaded).
#[derive(Debug, Clone)]
pub struct EquationSummary {
    pub id: EquationId,
    pub name: String,
    pub description: String,
    pub latex: String,
}

impl Equation {
    /// New equation with generated id and current timestamps.
    pub fn new(name: String, latex: String) -> Self {
        let now = crate::store::now_rfc3339();
        Self {
            id: EquationId::new(),
            name,
            description: String::new(),
            latex,
            references: Vec::new(),
            tags: Vec::new(),
            variables: Vec::new(),
            related: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}
```

### 3.3 `store/migrations.rs` (exact)
```rust
use rusqlite::Connection;
use crate::error::Result;

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS equations (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    latex       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS variables (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    symbol      TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    position    INTEGER NOT NULL,
    PRIMARY KEY (equation_id, symbol)
);

CREATE TABLE IF NOT EXISTS tags (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    tag         TEXT NOT NULL,
    PRIMARY KEY (equation_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);

CREATE TABLE IF NOT EXISTS refs (
    equation_id TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    text        TEXT NOT NULL,
    url         TEXT,
    position    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS related (
    a TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    b TEXT NOT NULL REFERENCES equations(id) ON DELETE CASCADE,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    Ok(())
}
```

### 3.4 `store/mod.rs` — required API and rules

```rust
mod migrations;

use rusqlite::Connection;
use std::path::Path;
use crate::error::{Error, Result};
use crate::model::*;

pub struct Store { conn: Connection }

pub fn now_rfc3339() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> { /* open, PRAGMA foreign_keys=ON, migrate */ }
    pub fn open_in_memory() -> Result<Self> { /* for tests */ }

    pub fn list(&self) -> Result<Vec<EquationSummary>> { /* SELECT ordered by name */ }
    pub fn get(&self, id: EquationId) -> Result<Equation> { /* join all child tables */ }
    pub fn insert(&self, eq: &Equation) -> Result<()> { /* one transaction */ }
    pub fn update(&self, eq: &Equation) -> Result<()> { /* txn: update row, delete+reinsert children */ }
    pub fn delete(&self, id: EquationId) -> Result<()> { /* DELETE; cascade handles children */ }
}
```

**Implementation rules (must follow):**
- `open`: `Connection::open(path)`, then `conn.execute("PRAGMA foreign_keys = ON", [])?`,
  then `migrations::migrate(&conn)`.
- `insert`/`update` wrap **all** statements in a single transaction
  (`let tx = conn.transaction()?; ...; tx.commit()?`).
- `related` is **undirected**: before writing a pair `(eq.id, other)`, sort the two id
  strings and store as `(min, max)` to satisfy `CHECK (a < b)`. When loading `related`
  for an equation X, select rows where `a = X OR b = X` and return the *other* id.
- `update` strategy for children: `DELETE FROM variables WHERE equation_id=?` (same for
  tags, refs, related touching this id), then re-insert from the struct. Simple and
  correct.
- `get` on missing id returns `Error::NotFound(id.to_string())`.
- `variables`/`refs` use the `position` column to preserve order (`ORDER BY position`).

### 3.5 Tests (exact list — put in `store/mod.rs` `#[cfg(test)]`)
Write these named tests; all must pass:
1. `insert_then_get_roundtrip` — full equation with 2 variables, 2 tags, 1 ref survives
   round-trip with fields and order intact.
2. `list_returns_summaries_sorted_by_name`.
3. `update_replaces_children` — change variables/tags, confirm old ones gone.
4. `delete_cascades` — after delete, child tables have no rows for that id.
5. `related_is_bidirectional` — link A↔B once; `get(A).related` contains B and
   `get(B).related` contains A.
6. `get_missing_is_not_found` — returns `Error::NotFound`.

Use `Store::open_in_memory()` in every test.

### DoD
```
cargo test -p equivault-core
```
All six tests pass. `cargo clippy -p equivault-core -- -D warnings` is clean.

---

## 4. Phase 2 — Rendering (stub) + unicode approximation

**Goal:** a working `render_image` that does **not** need RaTeX yet, plus the cheap
unicode preview. This unblocks the entire TUI.

### 4.1 `render/mod.rs` (exact public API — never change these signatures)
```rust
mod stub;
mod unicode;
// mod ratex;  // enabled in Phase 7

use image::RgbaImage;

pub use unicode::to_unicode_approx;

/// Render a LaTeX math string to an RGBA image.
/// `px_height` is the target glyph height in pixels (caller picks based on cell size).
/// Invalid LaTeX must return Err so the editor can show the message instead of crashing.
pub fn render_image(latex: &str, px_height: u32) -> Result<RgbaImage, String> {
    stub::render(latex, px_height)
    // Phase 7: switch to ratex::render(latex, px_height)
}
```

### 4.2 `render/stub.rs` (reference)
Render the raw LaTeX string as plain dark text on a white background into an `RgbaImage`,
sized roughly to the text length and `px_height`. It does **not** need to look good — it
must be non-empty and deterministic so the TUI and tests work. Implementation options, in
order of preference:
- Draw the string with a simple bitmap: fill a white `RgbaImage` of
  `width = latex.chars().count() as u32 * (px_height/2)` (min 16), `height = px_height`,
  and draw black rectangles per non-space char as a placeholder "glyph". This needs only
  the `image` crate.
- Return `Err("empty".into())` when `latex.trim().is_empty()`.

Keep it under ~40 lines. This is intentionally a placeholder swapped out in Phase 7.

### 4.3 `render/unicode.rs` (reference)
```rust
/// Best-effort LaTeX -> unicode for the browser list preview. Not a typesetter.
pub fn to_unicode_approx(latex: &str) -> String { /* table-driven replace */ }
```
Starter table (extend freely): `\alpha`→α, `\beta`→β, `\gamma`→γ, `\delta`→δ,
`\theta`→θ, `\lambda`→λ, `\mu`→μ, `\pi`→π, `\sigma`→σ, `\omega`→ω, `\hbar`→ℏ,
`\nabla`→∇, `\partial`→∂, `\infty`→∞, `\times`→×, `\cdot`→·, `\sum`→∑, `\int`→∫,
`\sqrt`→√, `\pm`→±, `\leq`→≤, `\geq`→≥, `\neq`→≠, `\approx`→≈, `\rightarrow`→→.
Then handle `^2`→², `^3`→³, `^n`→ⁿ where simple; strip remaining `\`, `{`, `}`, `$`.
Order matters: replace longer tokens before shorter ones.

### 4.4 Tests (exact)
1. `stub_renders_nonempty_image` — `render_image("E = mc^2", 48)` is `Ok` with
   width>0 and height>0.
2. `stub_empty_is_err` — `render_image("   ", 48)` is `Err`.
3. `unicode_alpha` — `to_unicode_approx("\\alpha + \\beta")` contains "α" and "β".
4. `unicode_superscript` — `to_unicode_approx("x^2")` contains "²".

### DoD
```
cargo test -p equivault-core
```
All Phase 1 + Phase 2 tests pass. Clippy clean.

---

## 5. Phase 3 — TUI shell + browser (read-only)

**Goal:** launch the TUI, show the two-pane browser over seeded data, navigate with
`j`/`k`, render the selected equation's image in the right pane, quit with `q`. No
create/edit/delete yet.

> **Start with a "hello ratatui" spike.** Before wiring EquiVault, make `main.rs` open the
> alt screen, draw a single `Paragraph`, and quit on `q`. Get that compiling against
> `ratatui` 0.30 first (fix any API drift here, once). Then build the rest.

### 5.1 `tui.rs` (reference — terminal lifecycle + panic safety)
Provide:
- `init() -> io::Result<Terminal<...>>`: enable raw mode, enter alternate screen, return a
  `ratatui` `Terminal` over a `CrosstermBackend<Stdout>`.
- `restore() -> io::Result<()>`: leave alternate screen, disable raw mode.
- Install a **panic hook** in `main` that calls `restore()` before the default hook, so a
  panic never leaves the user's terminal broken.

### 5.2 `action.rs` (exact)
```rust
use equivault_core::EquationId;

#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    MoveUp,
    MoveDown,
    FocusLeft,
    FocusRight,
    NewEquation,
    DeleteRequest,
    ConfirmYes,
    ConfirmNo,
    OpenDetail,
    Back,         // Esc
    EditCurrent,
    // editor-only actions added in Phase 5
    None,
}
```

### 5.3 `event.rs` (mode-aware keymap — exact mapping)
Map a `crossterm::event::KeyEvent` to an `Action` **depending on the current `Mode`**:

- **Browser:** `q`→Quit, `j`/`Down`→MoveDown, `k`/`Up`→MoveUp, `h`→FocusLeft,
  `l`→FocusRight, `n`→NewEquation, `d`→DeleteRequest, `Enter`→OpenDetail,
  `Ctrl-C`→Quit, `?`→(help; optional).
- **Detail:** `Esc`→Back, `j`/`k`→scroll, `e`→EditCurrent, `Ctrl-C`→Quit.
- **ConfirmDelete:** `y`→ConfirmYes, `n`/`Esc`→ConfirmNo.
- **Editor:** keys go to the focused `tui-textarea` field **except** `Esc`→Back,
  `Tab`/`Shift-Tab`→field switch, `Ctrl-S`→save (handled in Phase 5). `h`/`l` are
  **literal text** here, never focus changes.

> The `h`/`l` ambiguity is resolved entirely by this mode switch. Get it right.

### 5.4 `app.rs` (state)
```rust
pub enum Mode { Browser, Detail, Editor, ConfirmDelete(EquationId) }
pub enum Pane { List, Preview }

pub struct AppState {
    pub store: Store,
    pub mode: Mode,
    pub items: Vec<EquationSummary>,
    pub cursor: usize,
    pub focus: Pane,
    pub should_quit: bool,
    pub graphics_ok: bool,
    pub status: String,
    // render plumbing (Phase 4 worker handle), editor state (Phase 5)
}
```
Methods: `reload(&mut self)` (refill `items` from `store.list()`, clamp `cursor`),
`selected_id()`, `apply(&mut self, Action)` (the update function — a big `match`).

### 5.5 `graphics.rs`
- Build a `ratatui_image::picker::Picker` (via terminal font-size query or env). On
  success → `graphics_ok = true`. On failure, set false and keep going (the half-block
  fallback still renders). **Check the ratatui-image 2.x docs for the exact constructor**
  (`Picker::from_query_stdio()` or `Picker::from_fontsize(...)`); fix the call to match.
- When `graphics_ok == false`, the UI shows a one-line yellow banner:
  `No terminal graphics detected — using low-res fallback. Use Kitty/WezTerm/Ghostty/foot.`

### 5.6 `ui/` (layout)
- `ui/mod.rs::draw(frame, app)` splits by `Mode`.
- `browser.rs`: horizontal split 45% / 55%. Left = `List` of items showing
  `name` + dim `description` + `to_unicode_approx(latex)` line. Right = the rendered
  image pane (see `widgets.rs`). Highlight the row at `cursor`.
- `widgets.rs::preview_pane`: render the current equation image with
  `ratatui_image`'s `StatefulImage` widget using the `Picker`. (The actual image bytes
  come from the render worker in Phase 4; in Phase 3 you may render synchronously via
  `core::render_image(latex, px)` once per selection change to prove the pipeline.)
- Bottom line: a help/status bar showing the key hints for the current mode.

### 5.7 main loop (`main.rs`)
```
init terminal + panic hook
let mut app = AppState::open(db_path)?  // seeds a few demo equations if empty
loop {
    terminal.draw(|f| ui::draw(f, &mut app))?;
    if event::poll(Duration::from_millis(50))? {
        let action = event::map(read_key()?, &app.mode);
        app.apply(action);
    }
    drain render-worker results (Phase 4)
    if app.should_quit { break; }
}
restore terminal
```

**Seed data:** on first run with an empty DB, insert 3 demo equations (e.g. `E = mc^2`,
`\nabla \cdot \mathbf{E} = \rho/\varepsilon_0`, `e^{i\pi} + 1 = 0`) so the browser isn't
empty. Put this in `AppState::open`.

### DoD
- `cargo run -p equivault-tui` launches, shows the browser with the 3 seeded equations.
- `j`/`k` move the highlight; the right pane updates.
- On a Kitty/WezTerm terminal an image shows; elsewhere the warning banner shows.
- `q` quits and the terminal is fully restored (no broken state, even after a forced
  panic — test by temporarily `panic!()`-ing in `draw`).

---

## 6. Phase 4 — Async render worker + debounce + cache

**Goal:** move rendering off the UI thread, add the **150ms debounce**, and cache results.
This makes navigation snappy and is the foundation for the live editor.

### 6.1 `render_worker.rs` (reference)
- Spawn one OS thread. Channel in: `RenderJob { generation: u64, latex: String, px: u32 }`
  (`std::sync::mpsc`). Channel out: `RenderResult { generation: u64, image: Result<RgbaImage,String> }`.
- The worker loops: `recv()` a job, call `core::render_image`, send the result.
- **Generation drop:** the UI keeps a monotonically increasing `generation`. Each time the
  selected/edited LaTeX changes, bump `generation` and (after debounce) send a job tagged
  with it. When a result arrives, **ignore it if its generation < current** (stale).
- **LRU cache** in the UI thread: `HashMap<u64hash_of_latex, RgbaImage>` capped (e.g. 64
  entries, evict oldest). On selection change: if cached, use immediately and skip the
  worker; else schedule a render.

### 6.2 Debounce
- Track `last_change: Instant` and `dispatched_generation`. In the main loop tick, if
  `current_generation != dispatched_generation` **and**
  `last_change.elapsed() >= 150ms`, send the job and set `dispatched_generation`.
- Because `event::poll` already wakes every 50ms, no extra timer is needed.

### 6.3 Wire into browser
- Selection change sets `latex` + bumps generation + records `last_change`.
- The preview pane draws the latest decoded `StatefulProtocol`/image; while a render is in
  flight it keeps showing the previous image (no fling/flicker).

### DoD
- Rapidly holding `j` does **not** stutter; renders coalesce (you can log
  "render fired" and confirm it fires ~once per 150ms quiet period, not per keypress).
- Re-selecting a previously viewed equation is instant (cache hit).
- No panic, no UI freeze.

---

## 7. Phase 5 — Detail view, delete, and the editor (create/edit) with live preview

**Goal:** complete the interaction model.

### 7.1 Detail view (`ui/detail.rs`)
- `Enter` in Browser → `Mode::Detail`. Left pane lists: **name, description, references**
  (text + url), **tags**, **variables** (`symbol → description`, one per line),
  **related equations** (resolve ids to names via `store.get`/`list`). Right pane: same
  rendered image as Browser. `j`/`k` scroll the left pane; `Esc`→Browser; `e`→Editor.

### 7.2 Delete (`ConfirmDelete`)
- `d` in Browser → `Mode::ConfirmDelete(selected_id)` showing
  `Delete "<name>"? (y/n)`. `y`→`store.delete(id)` + `reload()` + back to Browser;
  `n`/`Esc`→Browser unchanged.

### 7.3 Editor (`ui/editor.rs`) — use `tui-textarea`
- `n` → empty Editor (new). `e` (from Detail) → Editor pre-filled from the equation.
- Fields, each a `tui_textarea::TextArea`:
  `name`, `description`, `latex`, `references`, `tags`, `variables`, `related`.
  - `tags`: single line, comma-separated → `Vec<String>`.
  - `variables`: one `symbol = description` per line → `Vec<Variable>`.
  - `references`: one `text | url` per line (url optional) → `Vec<Reference>`.
  - `related`: chosen names → ids (simple v1: comma-separated names matched against
    `store.list()`; a fancier picker is Phase 6).
- **Layout:** left = the form (highlight focused field); right = **live preview** of the
  `latex` field, driven by the Phase 4 worker with the **150ms debounce**. On a render
  `Err`, show the error text in the right pane in red instead of an image.
- Keys (Editor mode): `Tab`/`Shift-Tab` cycle fields; typing edits the focused field via
  `textarea.input(key)`; `Ctrl-S` validate + save; `Esc` cancel (if dirty, confirm).
- **Save:** build an `Equation` (new id if creating; keep id if editing), set
  `updated_at = now`, call `store.insert` or `store.update`, then `reload()` and return to
  Browser (or Detail) with a status message.

### 7.4 Editor state (`app.rs`)
```rust
pub struct EditorState {
    pub editing: Option<EquationId>,   // None = creating
    pub fields: [TextArea<'static>; 7],
    pub focus: usize,                  // which field
    pub dirty: bool,
    pub last_change: Instant,
    pub generation: u64,
    pub preview: Option<RgbaImage>,
    pub preview_error: Option<String>,
}
```

### DoD
- `n` → fill fields → `Ctrl-S` → new equation appears in the browser and persists across
  restarts (verify by quitting and relaunching).
- Editing the `latex` field updates the right preview ~150ms after you stop typing;
  invalid LaTeX shows a red error, not a crash.
- `e` edits and saves changes; `d`+`y` deletes; `Esc` exits views correctly.
- `cargo clippy --workspace -- -D warnings` clean.

---

## 8. Phase 6 — Polish (optional, after core flow works)

Pick from, in priority order:
- **Search:** `/` opens a filter; `store.search(q)` over name/description/tags.
- **Variable lookup:** `store.by_symbol(sym)` → list equations using a symbol.
- **Related-equation picker:** a popup list with multi-select instead of typing names.
- **Help overlay** (`?`): full keymap.
- **JSON import/export:** `serde_json` over `Vec<Equation>` (`--export file.json`,
  `--import file.json`).
- **Config / `--db <path>`** flag and `EQUIVAULT_DB` env var.

Each is independently shippable; give each its own small DoD (a test or a manual check).

---

## 9. Phase 7 — Swap in the real RaTeX renderer (isolated)

**Do this only after Phases 0–5 work with the stub.** Everything else stays untouched;
only `render/mod.rs`'s one call site changes.

### Steps
1. `cargo add ratex-svg resvg@0.44 usvg@0.44 tiny-skia@0.11 -p equivault-core`.
   (If `ratex-svg`'s API/name differs, check https://github.com/erweixin/RaTeX and
   https://docs.rs/ratex-svg — it is `0.0.x`, so confirm the entry function.)
2. Implement `render/ratex.rs::render(latex, px_height) -> Result<RgbaImage, String>`:
   - LaTeX (math) → SVG string via `ratex-svg`'s render entry point.
   - `let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())?;`
   - Compute a scale so the rendered height ≈ `px_height`; create
     `tiny_skia::Pixmap::new(w, h)`.
   - `resvg::render(&tree, transform, &mut pixmap.as_mut());`
   - Convert `pixmap.data()` (RGBA premultiplied) → `image::RgbaImage`
     (un-premultiply if needed).
   - Map every failure to `Err(String)` (parse error, empty input, unsupported macro).
3. In `render/mod.rs`, switch `render_image` from `stub::render` to `ratex::render`.
   **Keep `stub.rs`** as a fallback you can flip back to.
4. **Fallback rule:** if `ratex-svg` proves too immature (panics, missing macros you
   need), keep the stub or replace the body with a Tectonic-based path — *no other file
   changes*, because `render_image`'s signature is fixed.

### DoD
- `render_image("E = mc^2", 48)` returns a real rasterised equation (eyeball it in the
  TUI: `\frac`, `^`, `_`, Greek all look right).
- Invalid LaTeX still returns `Err` and the editor shows it.
- All earlier tests still pass; adjust `stub_*` tests to target the active backend or keep
  them pointed at `stub::render` directly.

---

## 10. Definition-of-Done summary (gates)

| Phase | Command(s) that must pass |
|---|---|
| 0 | `cargo build --workspace` |
| 1 | `cargo test -p equivault-core` (6 store tests) + clippy clean |
| 2 | `cargo test -p equivault-core` (store + 4 render tests) |
| 3 | `cargo run -p equivault-tui` → browser navigates, quits cleanly |
| 4 | manual: renders coalesce at ~150ms, cache hits are instant |
| 5 | manual: create/edit/delete persist; live preview + error handling work |
| 6 | per chosen feature |
| 7 | real equations render; `Err` path intact; tests green |

---

## 11. Risks & mitigations

- **RaTeX is `0.0.x` and unverified** → quarantined to Phase 7 behind a fixed
  `render_image` signature; app is fully functional on the stub first; Tectonic is a
  drop-in fallback touching only `render/ratex.rs`.
- **ratatui 0.30 / ratatui-image 2.x / tui-textarea API drift** → each TUI phase starts
  with the smallest compiling milestone; fix call sites against docs without changing the
  signatures this plan defines.
- **Terminal without graphics** → detection + half-block fallback + warning banner
  (Phase 3); never a hard failure.
- **Render latency / UI jank** → worker thread + 150ms debounce + generation-drop + LRU
  cache (Phase 4).
- **`h`/`l` text-vs-navigation ambiguity** → resolved by the mode-aware keymap (Phase 3);
  literal text inside the editor.
- **Panic leaving a broken terminal** → panic hook restores the terminal (Phase 3 DoD).
- **Over-abstraction** → no speculative GUI "port" traits; `equivault-core`'s public API
  *is* the boundary, enforced by the no-`ratatui`-in-core dependency rule.
```
