# Nullspace — fix plan (review items 5–10)

Scope: the polish/correctness items from the code review. Items **1–4 are out of scope**
here (query ordering, sync render on UI thread, autosave/discard, redundant generation
bumps) — tracked separately.

Conventions: after each item, run its **Check**. Keep `nullspace-core` free of
`ratatui`/`ratatui-image`. Final gate for the whole batch:
`cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`.

---

## 5. Don't show a previous equation's image under a new selection's error

**Problem.** On a render `Err`, `tick_render` (`app.rs:467-469`) keeps the old
`preview_protocol`, and `widgets.rs::preview_pane` then draws that stale image plus a
warning. For **editor live-preview** (same equation, mid-typing) that continuity is
desired. For **browser navigation** to an equation that fails to render, it shows the
*previous* equation's image — misleading.

**Change.** Distinguish the two contexts by a flag captured at schedule time.
- Add `preview_preserve_on_error: bool` to `AppState`.
- In `schedule_latex` (`app.rs:513`), set it from the mode:
  `self.preview_preserve_on_error = matches!(self.mode, Mode::Editor | Mode::RelatedPicker | Mode::ConfirmRemoveRelated(_));`
- In `tick_render`'s `Err` branch (`app.rs:467`):
  ```rust
  Err(err) => {
      self.preview_error = Some(err);
      if !self.preview_preserve_on_error {
          self.preview = None;
          self.preview_protocol = None;   // widgets then shows error-only
      }
  }
  ```
- `widgets.rs::preview_pane` already renders error-only when `preview_protocol` is `None`
  (`widgets.rs:37-40`), and image+warning when it's `Some` — no UI change needed.

**Files.** `app.rs`.
**Check.** In the browser, select an equation with valid LaTeX (image shows), then select
one whose LaTeX fails to render: the pane shows **only** the error, not the prior image.
In the editor, type an invalid fragment: the last good image stays with a warning.

---

## 6. Cache the *displayable* image; add a cross-run disk cache

Today there is only an in-memory `HashMap<u64, RgbaImage>` of **raw** renders
(`app.rs:83`, `remember_cache` at `app.rs:838`), and `protocol_for` recolors on **every**
navigation/cache-hit (`graphics.rs:32`, called at `app.rs:464` and `app.rs:519`). And the
cache is **lost on exit** — every restart re-renders everything.

Two independent changes:

### 6a. In-memory cache stores the recolored (display-ready) image

Recolor exactly once, when a fresh raw render arrives; cache that result; never recolor on
a cache hit.

- Split `graphics.rs`:
  - `pub fn recolor(&self, image: RgbaImage) -> RgbaImage` (the palette pass; returns the
    image unchanged when `palette` is `None`).
  - `pub fn protocol_from(&self, image: RgbaImage) -> StatefulProtocol` (wrap only, **no**
    recolor): `self.picker.new_resize_protocol(DynamicImage::ImageRgba8(image))`.
  - Keep `protocol_for` only if still needed; otherwise remove.
- `app.rs` worker-result `Ok` branch (`app.rs:460-466`):
  ```rust
  Ok(raw) => {
      self.preview_error = None;
      let display = self.graphics.recolor(raw);
      self.remember_cache(hash_latex(&result.latex), display.clone());
      self.preview_protocol = Some(self.graphics.protocol_from(display.clone()));
      self.preview = Some(display);
  }
  ```
- `app.rs` cache-hit in `schedule_latex` (`app.rs:518-522`): the cached image is already
  display-ready, so just `protocol_from` it (no recolor):
  ```rust
  if let Some(display) = self.cache.get(&key) {
      self.preview_protocol = Some(self.graphics.protocol_from(display.clone()));
      self.preview = Some(display.clone());
      self.preview_error = None;
      self.dispatched_generation = self.generation;
  }
  ```
- The in-memory `cache` now holds **recolored** images (valid only for the current run's
  palette — fine, it's per-process).
- Note: `AppState::preview` is currently write-only (nothing reads it). Either keep it as
  set above for future use or delete the field; do **not** rely on it for display.

### 6b. Persist raw renders to disk (survives restarts)

Put the disk cache in the **render worker** (TUI side), not core: it needs a cache
directory (an app concern) and keeps `core::render_image` a pure function with a stable
signature. The worker is the single render entry point, so browser and editor both benefit.

- New file `crates/nullspace-tui/src/render_cache.rs`:
  - `const RENDER_CACHE_VERSION: u32 = 1;` (bump when renderer output changes / on renderer
    swap, to invalidate stale files).
  - Key = 64-bit hash of `(latex, px, RENDER_CACHE_VERSION)`; filename `<hex>.png` under
    `ProjectDirs::from("dev","nullspace","Nullspace").cache_dir()/renders/`.
  - `pub fn load(latex, px) -> Option<RgbaImage>` — read+decode PNG (ignore errors → miss).
  - `pub fn store(latex, px, &RgbaImage)` — `create_dir_all` then encode PNG (best-effort,
    ignore errors).
  - **Disk stores the RAW render** (palette-independent). Recolor stays at display time, so
    a different terminal theme on the next run still renders correctly.
- `render_worker.rs` worker loop (`render_worker.rs:28-35`):
  ```rust
  let image = match render_cache::load(&job.latex, job.px) {
      Some(img) => Ok(img),
      None => {
          let r = nullspace_core::render::render_image(&job.latex, job.px);
          if let Ok(img) = &r { render_cache::store(&job.latex, job.px, img); }
          r
      }
  };
  ```
- Register `mod render_cache;` in `main.rs`.
- Optional (nice-to-have, not required): prune the renders dir to the most-recent N files
  by mtime on startup to bound growth. Equations are few, so this can wait.

**Files.** `graphics.rs`, `app.rs`, `render_worker.rs`, new `render_cache.rs`, `main.rs`.
**Check.**
- Navigate the browser rapidly: no recolor cost per move (in-memory hits use
  `protocol_from`); profile/log shows `recolor` runs only on fresh renders.
- Quit and relaunch: previously-viewed equations appear **instantly** (disk hit, no
  RaTeX render). Confirm `.png` files exist under the cache dir's `renders/`.
- Switch to a terminal with a different theme: equations still render legibly (recolor
  applied to the cached raw image at display time).

---

## 7. Escape LIKE wildcards without becoming case-sensitive

**Problem.** `search` builds `%{lower(query)}%` with no escaping (`store.rs:46-54`), so a
query containing `%` or `_` acts as a wildcard.

**Change.** Escaping is orthogonal to case — keep `lower()` on both sides and add an
`ESCAPE` clause.
- Helper in `store.rs`:
  ```rust
  fn like_escape(input: &str) -> String {
      input.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
  }
  ```
- In `search`: `let pattern = format!("%{}%", like_escape(&query.to_lowercase()));`
  (order is fine: `lower()` of `\ % _` is unchanged).
- Add `ESCAPE '\'` to **each** LIKE in the query:
  ```sql
  WHERE lower(e.name)        LIKE ?1 ESCAPE '\'
     OR lower(e.description)  LIKE ?1 ESCAPE '\'
     OR lower(t.tag)          LIKE ?1 ESCAPE '\'
  ```
- `by_symbol` is an exact `=` match — leave it unchanged.

**Files.** `store.rs`.
**Check.** Add a test: insert names `"50% off"` and `"500 off"`; `search("50%")` returns
only `"50% off"`. Add a case test: `search("PHYSICS")` still matches tag `physics`.

---

## 8. Don't panic on a malformed stored UUID

**Problem.** `EquationId::parse(..).expect("stored ids are valid UUIDs")` at
`store.rs:106`, `store.rs:235`, `store.rs:333` panics the whole app on a corrupt DB.

**Change.** Convert to a recoverable error inside the rusqlite closures (which return
`rusqlite::Result`), so it surfaces as `Error::Db` via existing `?` and is shown in the
status line instead of crashing.
- Helper in `store.rs`:
  ```rust
  fn parse_id_col(raw: &str) -> rusqlite::Result<EquationId> {
      EquationId::parse(raw).ok_or_else(|| {
          rusqlite::Error::FromSqlConversionFailure(
              0, rusqlite::types::Type::Text,
              Box::new(crate::error::Error::NotFound(format!("invalid uuid: {raw}"))),
          )
      })
  }
  ```
  (or a dedicated `Error` variant — either is fine; reuse keeps it short.)
- Replace the three `.expect(...)` sites with `parse_id_col(&raw)?` /
  `parse_id_col(&other)?`.

**Files.** `store.rs`.
**Check.** `cargo test --workspace` still green. Optional test: insert a row with a junk id
via raw SQL, then `get`/`list` returns `Err(Error::Db(..))` rather than panicking.

---

## 9. Remove the `e` keybinding (Enter already edits)

**Problem.** Browser maps both `Enter`→`OpenCurrent` and `e`→`EditCurrent`
(`event.rs:21-22`); they do the same thing (`app.rs:300-306, 331-337`).

**Change.**
- Remove `KeyCode::Char('e') => Action::EditCurrent` from the `Browser` arm in `event.rs`.
- Remove the now-unused `Action::EditCurrent` variant (`action.rs`) and its handler arm in
  `app.rs` (keep `OpenCurrent`).
- Update the help text in `widgets.rs::status_bar` (`widgets.rs:105`):
  `"... enter/e edit ..."` → `"... enter edit ..."`.

**Files.** `event.rs`, `action.rs`, `app.rs`, `widgets.rs`.
**Check.** In the browser, `e` does nothing; `Enter` still opens the editor. No
dead-code/clippy warnings for `EditCurrent`.

---

## 10. Fix the clippy lint in `preview_pane`

**Problem.** `let mut lines = Vec::new(); lines.push(...)` (`widgets.rs` ~51) triggers
clippy's "calls to `push` immediately after creation".

**Change.** Build the vec literally:
```rust
let lines = vec![
    Line::from(""),
    Line::styled(to_unicode_approx(&app.preview_latex), Style::default().add_modifier(Modifier::BOLD)),
    Line::from(""),
    Line::from(app.preview_latex.clone()),
];
```

**Files.** `widgets.rs`.
**Check.** `cargo clippy --workspace --all-targets -- -D warnings` is clean.

---

## Suggested order

8 → 7 → 10 → 9 (small, isolated) → 5 (state flag) → 6 (largest: cache refactor + disk).

## Batch acceptance gate
```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Plus the per-item manual checks for 5 and 6.
