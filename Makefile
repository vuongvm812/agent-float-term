# Pass user values as environment, never interpolate them into a shell command.
export TAG CONFIRM DRY_RUN TAP_DIR

.DEFAULT_GOAL := help
.PHONY: help release
help:
	@printf '%s\n' 'make release DRY_RUN=1: read-only release plan' 'make release [TAG=vX.Y.Z] [TAP_DIR=/path/to/tap]: requires exact publish TAG approval'

release:
	@python3 -B scripts/release.py
