.PHONY: all export import demo

DEMO_DB := $(CURDIR)/demo/nullspace-demo.sqlite3
DEMO_DATA := demo/solid-state-physics.json

all:
	cargo run -p nullspace-tui

export:
	cargo run -p nullspace-tui -- --export equations.json

import:
	cargo run -p nullspace-tui -- --import equations.json

# Reset the demo library and launch the app against it, ready to record.
# Run inside a graphics-capable terminal (kitty, WezTerm, Ghostty) so the
# equation previews render as full-resolution inline images.
demo:
	rm -f "$(DEMO_DB)"
	NULLSPACE_DB="$(DEMO_DB)" cargo run --release -p nullspace-tui -- --import "$(DEMO_DATA)"
	NULLSPACE_DB="$(DEMO_DB)" cargo run --release -p nullspace-tui
