SHELL := /bin/bash

LOCAL_BIN := $(CURDIR)/bin

.PHONY: fmt test build run clean

fmt:
	@cargo fmt

test:
	@cargo test

build:
	@mkdir -p $(LOCAL_BIN)
	@cargo build --release
	@cp target/release/ss $(LOCAL_BIN)/ss

run:
	@cargo run --quiet --release -- $(ARGS)

clean:
	@rm -rf $(LOCAL_BIN) target
