# Plan: Toggle horizontal / vertical pane layout (`v` key)

This document is a step-by-step implementation plan. Follow the steps **in order**.
Each step says exactly which file to edit, what to add, and shows the code. Do not
skip the verification step at the end.

---

## 1. What we are building (the goal)

The terminal app (`nullspace-tui`) shows two panes:

- A **primary pane**: the equation **list** in the Browser view, or the **edit form**
  in the Editor view.
- A **preview pane**: the rendered LaTeX image.

Right now these two panes are always **side by side** (left/right). We want three things:

1. **New feature:** Pressing the **`v`** key toggles the layout between:
   - **Horizontal** (current behavior): the two panes sit side by side (primary on the
     left, preview on the right).
   - **Vertical** (new): the preview pane sits **at the top** and is **5 lines tall**,
     and the primary pane fills the rest of the screen below it.

2. **Bug fix:** The preview pane must be the **same size** in the Browser view ("Overview")
   and the Editor view ("item detail"). Today it changes size slightly when you open an
   item. (See section 3 for why.)

3. The toggle state is **shared** between the Browser and Editor views: if you switch to
   vertical layout and then open an equation, the Editor must also use vertical layout.

---

## 2. Background: how the layout works today

There are two screens, each drawn by its own file:

- **Browser / Overview** → `crates/nullspace-tui/src/ui/browser.rs`
- **Editor / item detail** → `crates/nullspace-tui/src/ui/editor.rs`

Both files build their layout the same way:

1. First a **vertical** split of the whole screen into:
   - `outer[0]` = the main content area (everything except the bottom line)
   - `outer[1]` = a 1-line status bar at the bottom

2. Then a **horizontal** split of `outer[0]` into two side-by-side panes.

### Browser today (`browser.rs`, around lines 10-18)

```rust
let outer = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(1), Constraint::Length(1)])
    .split(frame.area());
let panes = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
    .split(outer[0]);
```

Here `panes[0]` is the **list** (45% width) and `panes[1]` is the **preview** (55% width).

### Editor today (`editor.rs`, around lines 38-45)

```rust
let outer = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(1), Constraint::Length(1)])
    .split(frame.area());
let panes = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
    .split(outer[0]);
```

Here `panes[0]` is the **edit form** (55% width) and `panes[1]` is the **preview** (45% width).

---

## 3. Why the preview "changes size" today (the bug)

Look closely at the two percentages above:

- In the **Browser**, the preview pane is **55%** of the width.
- In the **Editor**, the preview pane is **45%** of the width.

So when you select an equation and press Enter to open the Editor, the preview pane
shrinks from 55% to 45%. That is the "changes size slightly when clicking on an item"
problem.

**The fix:** use **one single percentage** for the preview pane in *both* views. We will
store that percentage in one shared constant called `PREVIEW_PERCENT`.

**Decision (recommended default): `PREVIEW_PERCENT = 50`.**
This gives a clean 50% / 50% split in both views. The preview is then exactly the same
size in the Browser and the Editor, which fixes the bug. If you later want the preview
bigger or smaller, change this one number. (`50` means: preview takes 50% of the width,
and the primary pane takes the other 50%.)

For the **vertical** layout, the preview is always 5 lines tall in both views, so it is
automatically consistent. We will store `5` in a constant called `PREVIEW_VERTICAL_ROWS`.

> Note: "5 lines" means the preview box is 5 terminal rows tall **including its top and
> bottom border**. That leaves 3 rows of actual image inside. This is what the request
> asked for. If it looks too small later, increase `PREVIEW_VERTICAL_ROWS`.

---

## 4. The overall approach

We will add a small piece of shared state that remembers the current orientation, a new
action to flip it, a key binding for `v`, and one shared helper function that both
`browser.rs` and `editor.rs` call to split the content area. Using **one shared helper**
guarantees the two views stay consistent (this is what fixes the size bug and keeps the
vertical layout identical between views).

There are 5 code changes plus documentation:

1. Add a `LayoutOrientation` enum and a `layout` field to `AppState` (`app.rs`).
2. Add a `ToggleLayout` action and handle it (`action.rs` + `app.rs`).
3. Bind the `v` key to `ToggleLayout` in Browser mode (`event.rs`).
4. Add the shared `content_panes` helper + the two constants (`ui/mod.rs`).
5. Make `browser.rs` and `editor.rs` use the helper.
6. Update docs/help text (`README.md` + status bar).

Do them in this order.

---

## 5. Step-by-step changes

### Step 1 — Add the orientation type and state (`crates/nullspace-tui/src/app.rs`)

**1a.** Find the existing `Pane` enum (around lines 35-39). It looks like:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    List,
    Preview,
}
```

**Directly below it**, add a new enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutOrientation {
    /// Primary pane and preview pane sit side by side (left/right).
    Horizontal,
    /// Preview sits on top (a few lines tall); primary pane fills the rest.
    Vertical,
}
```

**1b.** Find the `AppState` struct definition (it starts around line 91 with
`pub struct AppState {`). Add a new public field. A good spot is right after the
`pub focus: Pane,` line:

```rust
    pub focus: Pane,
    pub layout: LayoutOrientation,
```

**1c.** Find where `AppState` is constructed inside `AppState::open` (the big struct
literal that starts around line 153 with `let mut app = Self {`). Add the initial value.
Put it right after the `focus: Pane::List,` line:

```rust
            focus: Pane::List,
            layout: LayoutOrientation::Horizontal,
```

This makes the app start in the current side-by-side layout.

---

### Step 2 — Add the `ToggleLayout` action

**2a.** Edit `crates/nullspace-tui/src/action.rs`. Add a new variant to the `Action`
enum. Put it near the focus actions (`FocusLeft`, `FocusRight`) so related things stay
together:

```rust
    FocusLeft,
    FocusRight,
    ToggleLayout,
```

**2b.** Edit `crates/nullspace-tui/src/app.rs`. Find the big `match action { ... }` block
inside the `apply` method (starts around line 333). Find the `Action::FocusRight => { ... }`
arm (around lines 364-367). Add a new arm right after it:

```rust
            Action::ToggleLayout => {
                self.layout = match self.layout {
                    LayoutOrientation::Horizontal => LayoutOrientation::Vertical,
                    LayoutOrientation::Vertical => LayoutOrientation::Horizontal,
                };
                Ok(())
            }
```

> Note: `LayoutOrientation` is defined in this same file (`app.rs`), so no import is
> needed here.

---

### Step 3 — Bind the `v` key (`crates/nullspace-tui/src/event.rs`)

We only bind `v` in **Browser** mode. We must NOT bind it in Editor mode, because in the
Editor the user types text into fields and needs to be able to type the letter "v".
(The orientation state is shared, so the layout you pick in the Browser is still used when
you open the Editor — you just can't toggle it again until you go back.)

Find the `Mode::Browser => match key.code { ... }` block (around lines 10-26). Add a line
for `v`. A natural place is right after the focus keys (`h` / `l`):

```rust
            KeyCode::Char('h') => Action::FocusLeft,
            KeyCode::Char('l') => Action::FocusRight,
            KeyCode::Char('v') => Action::ToggleLayout,
```

Do not change any other mode.

---

### Step 4 — Add the shared helper and constants (`crates/nullspace-tui/src/ui/mod.rs`)

This file is currently very small (just module declarations and a `draw` function).
We will add two constants and one helper function that both screens use.

**4a.** At the top of the file, make sure these imports exist. The file currently imports:

```rust
use crate::app::{AppState, Mode};
use ratatui::Frame;
```

Add the layout types and the `LayoutOrientation` enum. Change those two lines to:

```rust
use crate::app::{AppState, LayoutOrientation, Mode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;
```

**4b.** Below the imports (before or after the `draw` function is fine), add the constants
and helper:

```rust
/// Width of the preview pane (as a percentage of the content area) when the panes
/// are side by side. The primary pane gets the remaining `100 - PREVIEW_PERCENT`.
/// This is used by BOTH the browser and the editor so the preview is the same size
/// in both views.
pub const PREVIEW_PERCENT: u16 = 50;

/// Height of the preview pane (in terminal rows, including its border) when the
/// layout is vertical and the preview sits on top.
pub const PREVIEW_VERTICAL_ROWS: u16 = 5;

/// Splits the main content `area` into the primary pane and the preview pane,
/// according to the current layout `orientation`.
///
/// Returns `(primary_area, preview_area)`:
/// - `primary_area` is where the list (browser) or edit form (editor) is drawn.
/// - `preview_area` is where the rendered LaTeX preview is drawn.
///
/// Horizontal: primary on the left, preview on the right.
/// Vertical:   preview on top (PREVIEW_VERTICAL_ROWS tall), primary below.
pub fn content_panes(area: Rect, orientation: LayoutOrientation) -> (Rect, Rect) {
    match orientation {
        LayoutOrientation::Horizontal => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(100 - PREVIEW_PERCENT),
                    Constraint::Percentage(PREVIEW_PERCENT),
                ])
                .split(area);
            // chunks[0] = primary (left), chunks[1] = preview (right)
            (chunks[0], chunks[1])
        }
        LayoutOrientation::Vertical => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(PREVIEW_VERTICAL_ROWS),
                    Constraint::Min(1),
                ])
                .split(area);
            // chunks[0] = preview (top), chunks[1] = primary (below)
            (chunks[1], chunks[0])
        }
    }
}
```

> Why a shared helper? Because both screens call the exact same function, the preview is
> guaranteed to be the same size and position in both. That is what fixes the "preview
> changes size" bug and keeps the vertical layout identical between views.

---

### Step 5 — Use the helper in the Browser (`crates/nullspace-tui/src/ui/browser.rs`)

**5a.** Find the top of `draw` (around lines 10-18). Replace the `panes` block with a call
to the helper. After the change it should look like this:

```rust
pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let (list_area, preview_area) = crate::ui::content_panes(outer[0], app.layout);
```

(Keep the `outer` split exactly as it is — we only replaced the old `panes` split.)

**5b.** Now update the rest of `draw` to use `list_area` and `preview_area` instead of
`panes[0]` and `panes[1]`. There are three spots:

- The line that sets the list height (was `panes[0].height`):

  ```rust
      app.list_visible_height = list_area.height.saturating_sub(2);
  ```

- The line that renders the list (was `panes[0]`):

  ```rust
      frame.render_stateful_widget(list, list_area, &mut state);
  ```

- The line that renders the preview (was `panes[1]`):

  ```rust
      widgets::preview_pane(frame, preview_area, app, &preview_title);
  ```

Leave everything else in this file unchanged.

---

### Step 6 — Use the helper in the Editor (`crates/nullspace-tui/src/ui/editor.rs`)

**6a.** Find the top of `draw` (around lines 37-45). Replace the `panes` block with the
helper. After the change:

```rust
pub fn draw(frame: &mut Frame<'_>, app: &mut AppState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let (form_area, preview_area) = crate::ui::content_panes(outer[0], app.layout);
```

**6b.** Update the places that used `panes[0]` (the form) and `panes[1]` (the preview):

- The constraints computation (was `panes[0].width`):

  ```rust
      let row_constraints = app
          .editor
          .as_ref()
          .map(|editor| editor_row_constraints(editor, form_area.width))
          .unwrap_or_else(default_row_constraints);
  ```

- The rows split (was `.split(panes[0])`):

  ```rust
      let rows = Layout::default()
          .direction(Direction::Vertical)
          .constraints(row_constraints)
          .split(form_area);
  ```

- The preview render (was `panes[1]`):

  ```rust
      widgets::preview_pane(frame, preview_area, app, "Live preview");
  ```

Leave the rest of the file unchanged. In particular, **do not** touch
`draw_related_picker` — the related-picker modal has its own independent layout and is not
affected by this feature.

---

### Step 7 — Update the help text and README (documentation)

**7a.** Edit `crates/nullspace-tui/src/ui/widgets.rs`. Find the `status_bar` function and
its `Mode::Browser` help string (around lines 229-232). Add `v layout` to the help. For
example, insert it after the zoom hint:

```rust
        Mode::Browser => {
            "j/k move  / search  enter edit  +/- zoom  v layout  n new  c clone  y copy latex  d delete  q quit"
        }
```

**7b.** Edit `README.md`. In the **Browser** keybindings table (around lines 38-49), add a
row for `v`. For example, after the `+` / `-` zoom row:

```markdown
| `+` / `-` | Zoom the preview in / out |
| `v` | Toggle horizontal / vertical pane layout |
```

---

## 6. Optional: add tests

`AGENTS.md` asks for focused unit tests next to the code they exercise. Two small,
high-value tests:

**Test A — toggling flips orientation** (add to a `#[cfg(test)]` module; the `app.rs`
tests module is a natural home, but it currently only tests pure helpers, so the simplest
is to test the helper directly). Add this to the test module in
`crates/nullspace-tui/src/ui/mod.rs` (create one if it does not exist):

```rust
#[cfg(test)]
mod tests {
    use super::{content_panes, PREVIEW_VERTICAL_ROWS};
    use crate::app::LayoutOrientation;
    use ratatui::layout::Rect;

    #[test]
    fn vertical_layout_puts_preview_on_top_with_fixed_height() {
        let area = Rect::new(0, 0, 80, 40);
        let (primary, preview) = content_panes(area, LayoutOrientation::Vertical);
        assert_eq!(preview.y, 0);
        assert_eq!(preview.height, PREVIEW_VERTICAL_ROWS);
        assert_eq!(primary.y, PREVIEW_VERTICAL_ROWS);
        // Full width in vertical mode.
        assert_eq!(preview.width, 80);
        assert_eq!(primary.width, 80);
    }

    #[test]
    fn horizontal_layout_is_side_by_side() {
        let area = Rect::new(0, 0, 80, 40);
        let (primary, preview) = content_panes(area, LayoutOrientation::Horizontal);
        // Same vertical extent, placed left/right.
        assert_eq!(primary.height, 40);
        assert_eq!(preview.height, 40);
        assert!(preview.x >= primary.x + primary.width - 1);
    }
}
```

---

## 7. How to verify your work

Run these from the repo root, in order. They must all pass.

1. **Format:**

   ```sh
   cargo fmt --all
   ```

2. **Type-check:**

   ```sh
   cargo check
   ```

3. **Lint:**

   ```sh
   cargo clippy --workspace --all-targets
   ```

4. **Tests:**

   ```sh
   cargo test
   ```

5. **Manual check** (use a disposable database so you do not touch the real library):

   ```sh
   NULLSPACE_DB=/tmp/nullspace-test.sqlite3 cargo run -p nullspace-tui
   ```

   Then confirm by hand:
   - In the Browser, the list and preview are side by side (horizontal). Note how big the
     preview is.
   - Press `Enter` to open an equation in the Editor. The preview pane should be **the
     same size** as it was in the Browser (this confirms the bug fix). Press `Esc` to go
     back.
   - Press `v`. The preview should jump to the **top** and be **5 rows tall**, with the
     list filling the area below it.
   - With vertical layout still active, press `Enter` to open an equation. The Editor
     should **also** be vertical (preview on top, 5 rows), confirming the state is shared.
     Press `Esc` to go back.
   - Press `v` again to return to the side-by-side layout.
   - Move up/down with `j` / `k` in the vertical layout to confirm the list still scrolls
     correctly in its smaller area.

---

## 8. Notes, edge cases, and things NOT to do

- **Caching:** You do not need to clear or invalidate any image/render caches when the
  layout changes. The preview rendering refits the image to whatever rectangle it is given
  every frame, and `preview_pane` already reports the new preview size to the app via
  `app.set_preview_warm_size(...)`, which re-warms neighbor images at the new size. So the
  toggle "just works" with the existing caching code.

- **Why `v` is Browser-only:** In the Editor every printable key is typed into the focused
  text field, so binding `v` there would stop the user from typing the letter "v". The
  orientation is shared state, so the Editor still honors whatever layout the Browser is
  set to.

- **Very short terminals:** In vertical layout the editor form gets
  `screen_height - 5 - 1` rows. On a very small terminal the 7 stacked form fields may be
  clipped. This is acceptable and matches how ratatui already handles overflow; no special
  handling is required.

- **Do not change the related-picker modal** (`draw_related_picker` in `editor.rs`) or the
  search/confirm popups. They are overlays with their own fixed layouts and are out of
  scope.

- **Tuning later:** The two numbers that control sizing are `PREVIEW_PERCENT` (horizontal
  width split) and `PREVIEW_VERTICAL_ROWS` (vertical preview height), both in
  `crates/nullspace-tui/src/ui/mod.rs`. Change those constants if the defaults need
  adjusting; nothing else needs to move.
```