SHELL := /bin/bash

LOCAL_BIN := $(CURDIR)/bin

.PHONY: fmt test build run clean

fmt:
	@cargo fmt

test:
	@cargo test

build:
	@cargo build --release --locked

run:
	@cargo run --quiet --release -- $(ARGS)

clean:
	@rm -rf $(LOCAL_BIN) target
