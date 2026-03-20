.DEFAULT_GOAL := all
.PHONY: build release check test clean

build:
	cargo build

release:
	cargo build --release

check:
	cargo clippy -- -D warnings

test:
	sudo -E $$(which cargo) test

clean:
	cargo clean

all: check build
