.PHONY: build release-build install test consumer-test git-consumer-test lint fmt-check distribution-test check ci

VERSION ?= $(shell git describe --tags --match 'v*' --always --dirty 2>/dev/null || echo dev)
GIT_CONSUMER_REPOSITORY ?= $(CURDIR)
GIT_CONSUMER_REVISION ?= $(shell git rev-parse HEAD)

build:
	GLURP_VERSION="$(VERSION)" cargo build --release --locked

release-build: build

install:
	GLURP_VERSION="$(VERSION)" cargo install --locked --path .

test:
	cargo test --all-targets --locked
	cargo test --doc --locked

consumer-test:
	cargo test --manifest-path tests/consumer/Cargo.toml --locked

# Reads only the exact committed revision; no worktree or path overrides.
git-consumer-test:
	python3 -S distribution/tests/check_git_consumer.py --repository "$(GIT_CONSUMER_REPOSITORY)" --revision "$(GIT_CONSUMER_REVISION)"

lint:
	cargo clippy --all-targets --locked -- -D warnings

fmt-check:
	cargo fmt --all -- --check

distribution-test:
	python3 -S distribution/tests/run_tests.py

check: fmt-check lint test consumer-test build distribution-test git-consumer-test

ci: check
