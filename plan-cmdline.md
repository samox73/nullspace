# Plan: Vim-style command line (cmdline)

## Context

The TUI currently exposes every action through single-key bindings in Browser mode
(`n` new, `d` delete, `/` search, `q` quit — see `crates/nullspace-tui/src/event.rs`).
We want a discoverable, vim-style command line: press `:` to open a floating prompt,
type a command name, get autocompletion (greyed ghost text + a dropdown of matches),
accept with Tab/→, and execute with Enter. While it's open, all other input is
suspended (the prompt has exclusive focus). Initial commands: `exit`, `new`,
`delete`, `search`.

This slots cleanly into the existing `Mode` → `map_key` → `Action` → `AppState::apply`
pipeline; each command reuses an action that already exists.

## Decisions (confirmed with user)

- **Open key:** `:` from Browser mode (prompt glyph shown inside is `>`).
- **Suggestions:** greyed ghost text inline **and** a dropdown list of all matches.
- **`delete`:** reuses the existing confirm dialog (`Mode::ConfirmDelete`), same as `d`.

## Command → existing behavior mapping

| Command  | Reuses | Effect |
|----------|--------|--------|
| `exit`   | `Action::Quit` | `should_quit = true` |
| `new`    | `Action::NewEquation` | `open_editor(None)` |
| `delete` | `Action::DeleteRequest` | set `Mode::ConfirmDelete(selected_id)` |
| `search` | `Action::StartSearch` | enter search filter (same as `/`) |

## Implementation

### 1. State — `crates/nullspace-tui/src/app.rs`

- Add `Mode::Cmdline` to the `Mode` enum (around line 40).
- Add a small state struct mirroring the existing input-state structs
  (`BrowserFilter`, `RelatedPickerState`):
  ```rust
  pub struct CmdlineState {
      pub input: String,
      pub cursor: usize, // byte index into `input`
  }
  ```
- Add field `pub cmdline: Option<CmdlineState>` to `AppState` and initialize it to
  `None` in `AppState::open` (alongside `editor: None`).
- Add a module-level command table + helpers (near the other free functions at the
  bottom of `app.rs`):
  ```rust
  const COMMANDS: [&str; 4] = ["delete", "exit", "new", "search"]; // alphabetical
  ```
  - `command_matches(prefix: &str) -> Vec<&'static str>` — names starting with
    `prefix` (case-insensitive); empty prefix → all commands.
  - The "active" match (for ghost text / Enter / accept) is the first element of
    `command_matches`.

### 2. Actions — `crates/nullspace-tui/src/action.rs`

Add variants:
- `OpenCmdline`
- `CmdlineInput(crossterm::event::KeyEvent)` (chars, Backspace, Delete, Left, Right, Home, End)
- `CmdlineAccept` (Tab)
- `CmdlineExecute` (Enter)
- `CmdlineCancel` (Esc)

### 3. Key mapping — `crates/nullspace-tui/src/event.rs`

- In the `Mode::Browser` arm add `KeyCode::Char(':') => Action::OpenCmdline`.
- Add a new `Mode::Cmdline` arm:
  ```
  Esc                => CmdlineCancel
  Tab                => CmdlineAccept
  Enter              => CmdlineExecute
  Backspace | Delete | Left | Right | Home | End | Char(_) => CmdlineInput(key)
  _                  => None
  ```
  (Right is routed through `CmdlineInput` so the handler can decide: accept the
  suggestion when the cursor is at end-of-input and a ghost exists, otherwise move
  right. Tab always accepts.)

### 4. apply() handlers — `crates/nullspace-tui/src/app.rs`

In `AppState::apply` add arms:
- `Action::OpenCmdline` → `self.cmdline = Some(CmdlineState { input: String::new(), cursor: 0 }); self.mode = Mode::Cmdline;`
- `Action::CmdlineInput(key)` → `self.input_cmdline(key)` — edits `input`/`cursor`
  reusing the existing `prev_boundary` / `next_boundary` helpers (already used by
  `input_browser_filter`). Special-case `Right`: if `cursor == input.len()` and a
  ghost suggestion exists, accept it; else move right.
- `Action::CmdlineAccept` → `self.accept_cmdline()` — set `input` to the active
  match and `cursor = input.len()` (no execution).
- `Action::CmdlineExecute` → `self.execute_cmdline()`:
  - Resolve: exact name match, else first of `command_matches(input)`.
  - Close cmdline first: `self.cmdline = None; self.mode = Mode::Browser;`
  - Map resolved name to the action above and call `self.apply(action)`
    (so `StartSearch`/`DeleteRequest`/etc. run from a clean Browser state).
  - No match → `self.status = format!("Unknown command: {input}"); self.cmdline = None; self.mode = Mode::Browser;`
- `Action::CmdlineCancel` → `self.cmdline = None; self.mode = Mode::Browser;`

### 5. Rendering — `crates/nullspace-tui/src/ui/`

- In `ui/mod.rs` `draw`, route `Mode::Cmdline` to draw the browser first, then the
  overlay: `Mode::Cmdline => { browser::draw(frame, app); widgets::cmdline(frame, app); }`.
  (The browser stays visible but unfocused behind the prompt.)
- Add `widgets::cmdline(frame, app)` in `crates/nullspace-tui/src/ui/widgets.rs`:
  - Two stacked floating boxes near the top, reusing a centered-rect helper
    (the `centered_rect` logic in `browser.rs` lines 166-184 — lift it into
    `widgets.rs` or duplicate the small helper).
  - Top box: title `Cmdline`, single line `> ` + typed text (default style) +
    ghost completion (`Style::default().fg(Color::DarkGray)`), `Clear` first.
    Set the block cursor with `frame.set_cursor_position` at the end of the typed
    text (before the ghost), matching image 2.
  - Bottom box: a `List` of `command_matches(input)`, active (first) row
    highlighted with the same highlight style used by `equation_list`
    (`bg(DarkGray) fg(White)`). Skip the box entirely when there are no matches.
- `widgets::status_bar` — add a `Mode::Cmdline` arm to the `help` match (e.g.
  `"type command  tab/→ accept  enter run  esc cancel"`) so the exhaustive match
  still compiles.

### 6. Exhaustiveness

`Mode` is matched exhaustively in `ui/mod.rs`, `widgets::status_bar`, and
`event.rs`; adding `Mode::Cmdline` forces updating each — covered above.

## Verification

- `cargo build -p nullspace-tui` and `cargo clippy` clean.
- Unit tests (add to `app.rs` test module): `command_matches("")` returns all 4;
  `command_matches("s")` → `["search"]`; ghost/accept turns `"se"` into `"search"`;
  `execute_cmdline` for each command leaves the expected `Mode`/`should_quit`
  (`exit`→`should_quit`, `new`→`Editor`, `delete`→`ConfirmDelete`, `search`→`Search`);
  unknown command sets `status` and returns to `Browser`.
- Manual run (`cargo run -p nullspace-tui`): press `:`, confirm focus is captured
  (j/k/etc. do nothing), type `d` → see `elete` ghost + dropdown, press Tab to fill,
  Enter to trigger the delete confirm; repeat for `new`, `search`, `exit`; Esc closes
  with no side effects.
