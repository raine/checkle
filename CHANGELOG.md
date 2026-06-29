# Changelog

## v0.1.1 (2026-06-29)

- Run built-in and configured Rust check suites in parallel with compact progress output.
- Get focused failure summaries for Cargo, clippy, doctests, nextest, rustfmt, cargo-deny, and cargo-machete while full logs stay on disk.
- Use `checkle pre-commit` or `checkle format-staged` to format staged Rust files and run checks safely around unstaged work.
- Tune summary size with limits for diagnostics, failures, lines, line width, and fallback output.
