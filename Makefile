.PHONY: all build build-bins build-hook setup status clean test

TARGET_DIR := target/release

all: build

build: build-hook build-bins

build-bins:
	cargo build --release -p nucleon-cli -p nucleon-runner

build-hook:
	cargo build --release --target aarch64-apple-darwin -p nucleon-hook
	cargo build --release --target x86_64-apple-darwin -p nucleon-hook
	mkdir -p $(TARGET_DIR)
	lipo -create -output $(TARGET_DIR)/libnucleon.dylib \
		target/aarch64-apple-darwin/release/libnucleon.dylib \
		target/x86_64-apple-darwin/release/libnucleon.dylib
	cp $(TARGET_DIR)/libnucleon.dylib $(TARGET_DIR)/nucleon.dylib
	codesign -fs - $(TARGET_DIR)/libnucleon.dylib
	codesign -fs - $(TARGET_DIR)/nucleon.dylib

setup: build
	./$(TARGET_DIR)/nucleon setup

status:
	./$(TARGET_DIR)/nucleon status

test:
	cargo test

clean:
	cargo clean
