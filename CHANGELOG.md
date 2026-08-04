# Changelog

## v0.1.2 (2026-08-04)

- Stop sibling process groups after the first suite failure and preserve the initiating check's exit code.
- Handle SIGINT and SIGTERM cleanly while suites run.
- Stream job-prefixed output with `checkle run --verbose` while retaining compact default summaries and full logs.
- Publish checksum-verified binaries for arm64 and amd64 macOS and Linux releases.

## v0.1.1 (2026-06-29)

- Run built-in and configured Rust check suites in parallel with compact progress output.
- Get focused failure summaries for Cargo, clippy, doctests, nextest, rustfmt, cargo-deny, and cargo-machete while full logs stay on disk.
- Use `checkle pre-commit` or `checkle format-staged` to format staged Rust files and run checks safely around unstaged work.
- Tune summary size with limits for diagnostics, failures, lines, line width, and fallback output.
