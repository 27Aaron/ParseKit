# Run `just --list` to view project tasks.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set dotenv-load := true

export RUSTDOCFLAGS := "-D warnings"

default:
    @just --list

# Formatting and linting

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --locked --all-targets --all-features -- -D warnings

# Tests and documentation

test:
    cargo test --locked --all-targets --all-features

test-lib:
    cargo test --locked --no-default-features --lib

doc:
    cargo doc --locked --no-default-features --no-deps

# Matches the GitHub Actions gate.
check: fmt-check clippy test test-lib doc

# Ignored live tests; WeChat requires YUANBAO_COOKIE.
test-wechat:
    cargo test --locked --test wechat -- --ignored --nocapture

test-douyin:
    cargo test --locked --test douyin -- --ignored --nocapture

test-bilibili:
    cargo test --locked --test bilibili -- --ignored --nocapture

test-live: test-wechat test-douyin test-bilibili

# Builds

build:
    cargo build --locked --all-features

release:
    cargo build --locked --release --all-features

clean:
    cargo clean

# CLI shortcuts; recipes insert Cargo's `--` separator.

pk *args:
    cargo run --locked --bin pk -- {{ args }}

resolve *args:
    cargo run --locked --bin pk -- resolve {{ args }}

download *args:
    cargo run --locked --bin pk -- download {{ args }}

platforms:
    cargo run --locked --bin pk -- platforms --check

doctor:
    cargo run --locked --bin pk -- doctor
