set dotenv-load := true

check: fmt clippy test

fmt:
    cargo fmt --check

clippy:
    cargo clippy --all-targets --locked -- -D warnings

test:
    cargo test --locked

self-check: install-local
    checkle --label self-fmt --mode rustfmt -- cargo fmt --check
    checkle --label self-clippy --mode cargo -- cargo clippy --all-targets --locked --message-format=json -- -D warnings
    checkle --label self-test --mode cargo -- cargo test --locked --message-format=json

install-local:
    cargo install --path . --locked
