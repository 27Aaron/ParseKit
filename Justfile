# ParseKit task runner — `just` / `just --list`
# Recipes mirror .github/workflows/ci.yml where noted.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set dotenv-load := true

export RUSTDOCFLAGS := "-D warnings"

default:
    @just --list

# --- format / lint -----------------------------------------------------------

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --locked --all-targets --all-features -- -D warnings

# --- test / doc --------------------------------------------------------------

test:
    cargo test --locked --all-targets --all-features

test-lib:
    cargo test --locked --no-default-features --lib

doc:
    cargo doc --locked --no-default-features --no-deps

# Full CI gate (same steps as GitHub Actions)
check: fmt-check clippy test test-lib doc

# Network integration tests (default ignored). WeChat needs YUANBAO_COOKIE.
test-wechat:
    cargo test --locked --test wechat -- --ignored --nocapture

test-douyin:
    cargo test --locked --test douyin -- --ignored --nocapture

test-bilibili:
    cargo test --locked --test bilibili -- --ignored --nocapture

test-live: test-wechat test-douyin test-bilibili

# --- build -------------------------------------------------------------------

build:
    cargo build --locked --all-features

release:
    cargo build --locked --release --all-features

clean:
    cargo clean

# --- CLI ---------------------------------------------------------------------
#
# Correct:
#   just resolve 'https://v.douyin.com/…'
#   just resolve 'https://v.douyin.com/…' --json
#   just download 'https://weixin.qq.com/sph/…'
#   just download 'https://weixin.qq.com/sph/…' --json --force
#   just pk resolve '…' --json
#   just pk download '…' --force
#   just platforms
#
# Wrong (extra `--` becomes a pk argument):
#   just pk -- resolve '…'     # don't do this
#
# Recipe already inserts cargo's option terminator (`cargo run … -- …`).

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
