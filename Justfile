set dotenv-load := true

check:
    checkle run all

fmt:
    checkle run format-check

clippy:
    checkle run clippy

test:
    checkle run test

self-check: install-local
    checkle run format-check clippy test

install-local:
    cargo install --path . --locked
