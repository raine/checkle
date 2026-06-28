# Rust project checks

set dotenv-load := true
set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run project checks through checkle
check:
    checkle run all

# Run check and fail if there are uncommitted changes for CI
check-ci: check
    #!/usr/bin/env bash
    set -euo pipefail
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "Error: check caused uncommitted changes"
        echo "Run 'just check' locally and commit the results"
        git diff --stat
        exit 1
    fi

# Install shims into the Git hooks directory
install-hooks:
    scripts/install-git-hook-shims

# Check Rust formatting through checkle
format:
    checkle run format-check

# Check Rust formatting through checkle
fmt: format

# Check clippy through checkle
clippy:
    checkle run clippy

# Build the binary
build:
    cargo build --locked

# Run tests through checkle
test:
    checkle run test

# Install release binary globally
install:
    cargo install --offline --path . --locked

# Install release binary globally from local sources
install-local:
    cargo install --path . --locked

# Install debug binary globally via symlink
install-dev:
    cargo build && ln -sf $(pwd)/target/debug/checkle ~/.cargo/bin/checkle

# Run the application
run *ARGS:
    cargo run -- "$@"

# Install local binary and run core project checks
self-check: install-local
    checkle run format-check clippy test
