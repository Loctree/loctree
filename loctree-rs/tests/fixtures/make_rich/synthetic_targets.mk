# Synthetic edge cases for make semantic analyzer.

.PHONY: all build test clean install precheck

# Public entrypoints — must NOT be flagged as dead targets.
all: build test

build:
	@echo "Building..."
	@./scripts/_internal_compile.sh

test:
	@echo "Testing..."

clean:
	@rm -rf target/

install: build
	@cp target/release/foo /usr/local/bin/

precheck:
	@cargo fmt --check
	@cargo clippy -- -D warnings

# Variable assignments — must NOT classify as targets.
VERSION := 0.9.0
RELEASE_DIR := target/release
ARTIFACTS = $(RELEASE_DIR)/foo $(RELEASE_DIR)/bar

# Recipe lines — each tab-indented line is shell, NOT another make symbol.
release: build
	@mkdir -p dist/
	@cp $(ARTIFACTS) dist/
	@tar -czf dist/release-$(VERSION).tar.gz dist/

# Private/internal target — prefix `_` convention; T2 must classify as Internal.
_internal-cleanup:
	@find . -name '*.tmp' -delete