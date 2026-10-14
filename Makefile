.PHONY: build release-build install test lint fmt-check distribution-test check ci

VERSION ?= $(shell git describe --tags --match 'v*' --always --dirty 2>/dev/null || echo dev)

build:
	GLURP_VERSION="$(VERSION)" cargo build --release --locked

release-build: build

install:
	GLURP_VERSION="$(VERSION)" cargo install --locked --path .

test:
	cargo test --all-targets --locked

lint:
	cargo clippy --all-targets --locked -- -D warnings

fmt-check:
	cargo fmt --all -- --check

distribution-test:
	python3 -S distribution/tests/run_tests.py

check: fmt-check lint test build distribution-test

ci: check
