# nullspace

A terminal app for building a personal library of LaTeX equations. Browse, search,
and edit equations and see them rendered as real math inline in your terminal.

https://github.com/user-attachments/assets/4fb89555-f74a-410a-a634-12447759eaf9

## Requirements

- A recent Rust toolchain (`cargo`).
- For graphical previews, a terminal with image support (kitty, WezTerm, iTerm2,
  Ghostty, or any Sixel-capable terminal). Without one, previews fall back to
  half-block rendering.

## Running

```sh
cargo run -p nullspace-tui
# or
make all
```

Your library is stored locally and persists between runs. By default it lives in
your platform's data directory; set `NULLSPACE_DB` to use a custom path:

```sh
NULLSPACE_DB=/path/to/library.sqlite3 cargo run -p nullspace-tui
```

To launch a disposable demo library:

```sh
make demo
```

## Keybindings

### Browser

| Key | Action |
| --- | --- |
| `j` / `k` (or `↓` / `↑`) | Move selection |
| `Enter` | Edit the selected equation |
| `n` | New equation |
| `c` | Clone the selected equation |
| `y` | Copy the selected equation's LaTeX to the clipboard |
| `d` | Delete the selected equation |
| `/` | Search |
| `+` / `-` | Zoom the preview in / out |
| `v` | Toggle horizontal / vertical pane layout |
| `Esc` | Clear the active filter |
| `q` / `Ctrl-C` | Quit |

Search is broad by default and matches name, description, LaTeX, and tags.
Prefix the query to narrow it:

| Prefix | Matches |
| --- | --- |
| `tag:physics` | Tags |
| `var:E` | Variable symbols and descriptions |
| `name:gauss` | Equation names |
| `latex:\pi` | LaTeX source |
| `related:circle` | Equations related to matching equations |

### Editor

| Key | Action |
| --- | --- |
| `Tab` / `Shift-Tab` | Next / previous field |
| `Ctrl-S` | Save |
| `Esc` | Back |

In the **Related** field: `r` to choose equations from the library, `Enter` to open
the highlighted relation, `d` to remove it.

In the **References** field: `a` to add a reference, `Enter` to edit the highlighted
one, `d` to remove it, `j`/`k` or arrow keys to move. Each reference has authors,
year, title, DOI, and URL fields; a bare DOI such as `10.1103/PhysRev.140.A1133`
is automatically turned into a `https://doi.org/...` link. Existing libraries and
JSON files using the old single-line reference format still import unchanged.

### Related picker

Type to fuzzy-search, `Space` to toggle, `Enter` to apply, `Esc` to cancel.

## Import / export

Equations can be exported to and imported from JSON.

```sh
# Export the whole library
cargo run -p nullspace-tui -- --export equations.json   # or: make export

# Import equations from a file
cargo run -p nullspace-tui -- --import equations.json    # or: make import

# Choose how duplicates are handled on import (default: skip)
cargo run -p nullspace-tui -- --import equations.json --on-duplicate overwrite
```
