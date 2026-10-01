.PHONY: build test lint fmt-check check

build:
	cargo build --release --locked

test:
	cargo test --all-targets --locked

lint:
	cargo clippy --all-targets --locked -- -D warnings

fmt-check:
	cargo fmt --all -- --check

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets --locked -- -D warnings
	cargo test --all-targets --locked
	cargo build --release --locked
