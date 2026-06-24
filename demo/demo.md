# Recording a demo

nullspace's whole point is inline rendered equations, so the demo should show
real graphics. There are two routes:

- **Self-record with OBS (recommended)** — run the app in a graphics-capable
  terminal and screen-record. Full-resolution equation images, no caveats.
- **VHS** — scripted, reproducible GIF, but it does **not** capture inline
  terminal graphics (see [VHS and graphics](#vhs-and-graphics)); equations fall
  back to half-block previews.

Either way, the data is the same: a demo library of 20 solid-state-physics
equations ([`solid-state-physics.json`](solid-state-physics.json)) with
descriptions, tags, variables, references, and cross-links, loaded into a dedicated
database so your real library is untouched.

## Quick start

```sh
make demo
```

This resets the demo database, imports the 20 equations, and launches the app
against it. **Run it inside a graphics-capable terminal** (kitty, WezTerm, or
Ghostty) so the previews render as real images. It builds the release binary the
first time, which takes a minute; subsequent runs are instant. Quit the app with
`q`; re-running `make demo` always starts from the same clean state.

> Under the hood `make demo` sets `NULLSPACE_DB` to `nullspace-demo.sqlite3`,
> so it never touches your normal library.

## Self-record with OBS

1. Open a graphics-capable terminal (kitty / WezTerm / Ghostty) and size it
   generously — the bottom status line should read **"terminal graphics
   detected"** once the app starts. If it says "no terminal graphics detected,"
   your terminal isn't exposing a supported protocol.
2. Start the library and let it warm up:
   ```sh
   make demo
   ```
3. In OBS, add a **Window Capture** source for the terminal window (more reliable
   than Display Capture for a clean crop). Set the canvas to the terminal's
   aspect, 30–60 fps.
4. Run through the [storyboard](#what-to-demo) below, pausing ~1 s on each step so
   the preview finishes rendering (the `•` marker / spinner in the list gutter
   shows cache state — wait for `•`).
5. Stop the recording and trim in OBS or your editor of choice.

Tip: a larger terminal font makes the equations read clearly at typical playback
sizes.

## What to demo

The same feature walkthrough works whether you drive it by hand (OBS) or via the
tape (VHS). It touches every feature:

1. **Browse** — move with `j`/`k`; the preview re-renders for each selection.
2. **Zoom** — `+` / `-` rescale the rendered preview.
3. **Search** — `/` then `transport`, `Enter` to apply, `Esc` to clear. Matches
   names, descriptions, LaTeX, and tags.
4. **Variable lookup** — `v` then `n`, `Enter`. Lists every equation that uses the
   carrier-density symbol `n` (Drude, Fermi energy, Hall, plasma frequency, Bragg).
5. **Editor** — search `Drude`, `Enter` to filter, `Enter` to open; `Tab` through
   the Name / Description / LaTeX / References / Tags / Variables / Related fields.
6. **Related picker** — in the Related field press `r`, type `fermi` to fuzzy
   search, `Space` to toggle a relation, `Enter` to apply.
7. **New equation (live preview)** — `n`, type a name, `Tab` to the LaTeX field,
   type `E = \hbar \omega` and watch it render live. Leaving with `Esc` autosaves.
8. **Copy & delete** — `c` clones the selection (a notification confirms), then
   `d` + `y` deletes the clone.
9. **Quit** — `q`.
