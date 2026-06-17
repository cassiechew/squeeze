BINARY      := squeeze
CARGO_BIN   := $(HOME)/.cargo/bin
HOOK_SRC    := hooks/squeeze-rewrite.py
HOOK_DST    := $(HOME)/.claude/hooks/squeeze-rewrite.py
SETTINGS    := $(HOME)/.claude/settings.json

.PHONY: build test bench install install-hook setup uninstall clean help

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?##' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

build: ## Build squeeze (debug)
	cargo build

test: ## Run all tests
	cargo test

bench: ## Run compression benchmarks (requires release build)
	@test -f target/release/$(BINARY) || (echo "Run 'make build-release' first" && exit 1)
	@test -f /tmp/squeeze-bench/bench.sh && bash /tmp/squeeze-bench/bench.sh || echo "No bench script found at /tmp/squeeze-bench/bench.sh"

build-release: ## Build squeeze (release, optimised)
	cargo build --release

install-binary: build-release ## Install squeeze binary to ~/.cargo/bin
	cargo install --path . --force
	@echo "Installed: $(CARGO_BIN)/$(BINARY)"

install-hook: ## Copy hook script to ~/.claude/hooks/
	@mkdir -p $(HOME)/.claude/hooks
	cp $(HOOK_SRC) $(HOOK_DST)
	chmod +x $(HOOK_DST)
	@echo "Installed: $(HOOK_DST)"

register-hook: ## Register the PreToolUse hook in ~/.claude/settings.json
	@python3 scripts/register-hook.py

unregister-hook: ## Remove the squeeze hook from ~/.claude/settings.json
	@python3 scripts/unregister-hook.py

install: install-binary install-hook ## Install binary + hook script
	@echo ""
	@echo "Done. Run 'make setup' to also register the hook in settings.json."

setup: install register-hook ## Full setup: build, install binary, install hook, register in settings.json
	@echo ""
	@echo "squeeze is ready:"
	@echo "  Binary:  $(CARGO_BIN)/$(BINARY)"
	@echo "  Hook:    $(HOOK_DST)"
	@echo "  Config:  $(SETTINGS)"
	@echo ""
	@echo "Open /hooks in Claude Code to verify, or start a new session."

uninstall: unregister-hook ## Remove binary, hook script, and settings.json entry
	rm -f $(CARGO_BIN)/$(BINARY)
	rm -f $(HOOK_DST)
	@echo "Uninstalled squeeze."

clean: ## Remove build artifacts
	cargo clean
