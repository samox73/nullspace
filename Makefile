.PHONY: all install export import schema ai-complete ai-complete-codex ai-diff demo scan

DEMO_DB := $(CURDIR)/demo/nullspace-demo.sqlite3
DEMO_DATA := demo/solid-state-physics.json

all:
	cargo run -p nullspace-tui

scan:
	cargo run -p nullspace-tui -- scan

install:
	cargo install --path crates/nullspace-tui --bin nullspace --profile release

clear-db:
	rm -f ~/.local/share/nullspace/nullspace.sqlite3

export:
	cargo run -p nullspace-tui -- --export equations.json

schema:
	cargo run -p nullspace-tui -- --export-schema schema.json

import:
	cargo run -p nullspace-tui -- --import equations.json

ai-complete: export
	cp equations.json equations.json.bak
	claude -p "Follow the instructions in ai-complete.md." \
		--allowedTools "Read,Edit,Write,WebSearch,WebFetch" \
		--model opus
	jq -e '.equations | length' equations.json > /dev/null
	@echo "review with 'make ai-diff', then apply with 'make import'"

ai-complete-codex: export
	cp equations.json equations.json.bak
	codex --search exec -m gpt-5.5 -c 'model_reasoning_effort="high"' \
		"Follow the instructions in ai-complete.md."
	jq -e '.equations | length' equations.json > /dev/null
	@echo "review with 'make ai-diff', then apply with 'make import'"

ai-diff:
	git diff --no-index equations.json.bak equations.json || true

# Reset the demo library and launch the app against it, ready to record.
# Run inside a graphics-capable terminal (kitty, WezTerm, Ghostty) so the
# equation previews render as full-resolution inline images.
demo:
	rm -f "$(DEMO_DB)"
	NULLSPACE_DB="$(DEMO_DB)" cargo run --release -p nullspace-tui -- --import "$(DEMO_DATA)"
	NULLSPACE_DB="$(DEMO_DB)" cargo run --release -p nullspace-tui
