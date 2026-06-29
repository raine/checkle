# Releasing

Requires [rust-release-tools](https://github.com/raine/rust-release-tools):

```bash
pipx install git+https://github.com/raine/rust-release-tools.git
```

To release a patch version:

```bash
just release
```

To release a specific bump:

```bash
just _release minor
just _release major
just _release current
```

The release helper:

1. Bumps `Cargo.toml`.
2. Generates a `CHANGELOG.md` entry using Claude.
3. Opens the changelog for review.
4. Commits, publishes to crates.io, tags, and pushes.

Useful recovery commands:

```bash
cargo-release --continue
cargo-release --continue --skip-publish
```
