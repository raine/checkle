# checkle

`checkle` runs project checks and prints compact, agent-friendly failure output.
It keeps the full command output in `target/check-logs` and shows only the
useful diagnostics in the terminal.

The first target is Rust project checks:

- Cargo JSON compiler diagnostics, including clippy and doctests
- nextest failures, with JSON support and human-output fallback
- rustfmt diffs
- cargo-deny summaries
- cargo-machete summaries

## Install

From this repository:

```sh
cargo install --path .
```

From a checked-out copy in `~/code/checkle`:

```sh
cargo install --path ~/code/checkle
```

## Usage

Run a command through `checkle`:

```sh
checkle --label clippy --mode cargo -- \
  cargo clippy --message-format=json --all-targets -- -D warnings -D clippy::all
```

On success, `checkle` exits with code 0 and prints nothing. On failure, it exits
with the wrapped command's exit code and prints a compact summary:

```text
full log: target/check-logs/clippy.log

error: src/lib.rs:1:2 clippy::sample
  sample failure
  help: try sample fix
```

The full raw output remains in the log file. Logs are written relative to the
current directory unless `--log-dir` is absolute. Each log line is prefixed with
`[stdout]` or `[stderr]` so stream identity is visible. Labels can contain ASCII
letters, digits, `_`, `.`, and `-`; invalid labels fail rather than mapping to a
colliding log filename.

## Project checks

Run named Rust project checks in parallel:

```sh
checkle run format-check clippy test static-analysis
```

With no check names, `checkle run` lists available checks. The built-in checks are:

- `format-check`: `cargo fmt --all -- --check`
- `clippy`: `cargo clippy --message-format=json --all-targets --locked -- -D warnings`
- `test`: `cargo test --locked --message-format=json`
- `cargo-deny`: `cargo deny --format json check`
- `cargo-machete`: `cargo machete --with-metadata`

The `static-analysis` group runs `cargo-deny` and `cargo-machete` when those tools are installed. The `all` group runs `format-check`, `clippy`, `test`, and the installed static-analysis checks. Explicit `cargo-deny` and `cargo-machete` requests require their tools to be installed.

## Justfile integration

```just
check:
    checkle run all
```

Use the wrapper form for project-specific commands:

```sh
checkle --label doc-test --mode cargo -- cargo test --doc --message-format=json --locked
```

Use `--mode auto` for unknown checks. Specific modes produce better summaries.

## Local checks

Run the project checks with:

```sh
just check
```

Run `checkle` against this repository with:

```sh
just self-check
```

The self-check recipe installs the local binary and writes full logs under
`target/check-logs` while keeping terminal output compact.

## Agent guidance

Agents should use project `just` recipes that wrap checks through `checkle`.
Raw commands like `cargo clippy --message-format=json` print large JSON streams
and are intended for `checkle` to parse, not for direct agent output.
