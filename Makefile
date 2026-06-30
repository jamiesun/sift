CARGO ?= cargo
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
BINARY ?= sift
HOOKS_PATH ?= .githooks

.PHONY: all build release test fmt fmt-check clippy ci local-ci internal-gate githooks-install githooks-uninstall install uninstall clean

all: build

build:
	$(CARGO) build

release:
	$(CARGO) build --release --locked

test:
	$(CARGO) test

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt -- --check

clippy:
	$(CARGO) clippy -- -D warnings

internal-gate:
	@out="$$(mktemp)"; \
	if SIFT_INTERNAL_GATE=1 $(CARGO) run --quiet -- . >"$$out" 2>&1; then \
		/bin/rm -f "$$out"; \
	else \
		cat "$$out"; \
		/bin/rm -f "$$out"; \
		exit 1; \
	fi

ci: fmt-check test clippy internal-gate

local-ci: ci

githooks-install:
	chmod +x "$(HOOKS_PATH)/pre-commit"
	git config core.hooksPath "$(HOOKS_PATH)"
	@printf 'git hooks installed: core.hooksPath=%s\n' "$(HOOKS_PATH)"

githooks-uninstall:
	@current="$$(git config --get core.hooksPath || true)"; \
	if [ "$$current" = "$(HOOKS_PATH)" ]; then \
		git config --unset core.hooksPath; \
		printf 'git hooks uninstalled\n'; \
	else \
		printf 'git hooks not changed: current core.hooksPath=%s\n' "$${current:-<unset>}"; \
	fi

install: release
	mkdir -p "$(BINDIR)"
	install -m 0755 "target/release/$(BINARY)" "$(BINDIR)/$(BINARY)"
	@case ":$$PATH:" in *:"$(BINDIR)":*) ;; *) printf '%s\n' 'warning: $(BINDIR) is not in PATH' >&2 ;; esac
	@printf 'installed %s\n' "$(BINDIR)/$(BINARY)"

uninstall:
	rm -f "$(BINDIR)/$(BINARY)"

clean:
	$(CARGO) clean
