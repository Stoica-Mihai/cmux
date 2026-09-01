# cmux
#
#   make build      release binaries into target/release
#   make install    build, then put cmux + cmuxd on your PATH
#   make uninstall  take them off again
#   make check      everything CI runs
#   make smoke      end-to-end test against a real daemon
#   make demo       rendered walkthrough of the TUI
#
# `cargo install --path .` does not work from the workspace root: that manifest
# is a virtual one with no package, so each binary is installed by its own path.

CARGO ?= cargo
CRATES := crates/cmux crates/cmuxd
BINS := cmux cmuxd

.PHONY: all build install uninstall test fmt lint check smoke demo clean

all: build

build:
	$(CARGO) build --release --workspace

# --target-dir points cargo install at the same target/ that `build` just
# populated. Without it cargo builds into a scratch directory and recompiles
# the dependency tree from zero: measured here at 34 crates vs 0 for cmux.
# A single-crate install resolves features differently from the workspace
# build, so a few crates can still rebuild (5 for cmuxd) — not the whole tree.
install: build
	@for c in $(CRATES); do \
		$(CARGO) install --path $$c --locked --force --target-dir target || exit 1; \
	done
	@echo
	@for b in $(BINS); do \
		p=$$(command -v $$b 2>/dev/null); \
		if [ -n "$$p" ]; then \
			echo "installed  $$b -> $$p"; \
		else \
			echo "WARNING: $$b installed but not on PATH — add ~/.cargo/bin to PATH"; \
		fi; \
	done

uninstall:
	-$(CARGO) uninstall cmux
	-$(CARGO) uninstall cmuxd

test:
	$(CARGO) test --workspace

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Mirrors .github/workflows/ci.yml, in the same order.
check:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) build --workspace --all-targets
	$(CARGO) test --workspace

smoke: build
	PROFILE=release ./scripts/smoke.sh

demo: build
	PROFILE=release ./scripts/demo.sh

clean:
	$(CARGO) clean
