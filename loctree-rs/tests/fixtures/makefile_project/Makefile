# Root Makefile — includes common.mk and defines top-level targets.

VERSION := 0.9.0
NAME ?= loctree
CARGO := cargo

include common.mk

.PHONY: all build test clean

all: build test

build:
	$(CARGO) build --release

test:
	$(CARGO) test

clean:
	rm -rf target
