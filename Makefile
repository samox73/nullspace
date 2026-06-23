.PHONY: export import

all:
	cargo run -p nullspace-tui

export:
	cargo run -p nullspace-tui -- --export equations.json

import:
	cargo run -p nullspace-tui -- --import equations.json
