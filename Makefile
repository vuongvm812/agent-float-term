# Pass user values as environment, never interpolate them into a shell command.
export TAG CONFIRM DRY_RUN TAP_DIR

.DEFAULT_GOAL := help
.PHONY: help release install uninstall
help:
	@printf '%s\n' 'make install: build/install the managed app and a private patched tmux (no server restart)' 'make install DRY_RUN=1: read-only installation plan' 'make release DRY_RUN=1: read-only release plan' 'make release [TAG=vX.Y.Z] [TAP_DIR=/path/to/tap]: requires exact publish TAG approval'
	@printf '%s\n' 'make uninstall: remove the managed app and verified idle Make-created tmux builds' 'make uninstall DRY_RUN=1: preview removal without changing files or bindings'

install:
	@bash scripts/install-local.sh

uninstall:
	@python3 -B scripts/uninstall-local.py

release:
	@python3 -B scripts/release.py
