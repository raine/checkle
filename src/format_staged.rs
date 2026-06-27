use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::git;

#[derive(Debug, Clone)]
pub struct FormatStagedOptions {
    pub repo_root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormatSummary {
    pub formatted: Vec<String>,
    pub unchanged: Vec<String>,
    pub skipped: Vec<String>,
}

struct PendingUpdate {
    path: String,
    mode: String,
    old_sha: String,
    new_sha: String,
    original: Vec<u8>,
    formatted: Vec<u8>,
    mirror_worktree: bool,
}

struct FormattedBlob {
    path: String,
    mode: String,
    old_sha: String,
    original: Vec<u8>,
    formatted: Vec<u8>,
    mirror_worktree: bool,
}

pub fn format_staged_rust(options: &FormatStagedOptions) -> Result<FormatSummary> {
    let paths = git::staged_paths("ACMR", &["*.rs"])?;
    if paths.is_empty() {
        return Ok(FormatSummary::default());
    }

    let dirty_paths = git::dirty_paths(&paths)?;
    let entries = git::index_entries(&paths)?;
    let mut formatted = Vec::new();
    let mut unchanged = Vec::new();
    let mut skipped = Vec::new();

    for entry in entries {
        if entry.stage != 0 {
            bail!(
                "cannot format staged Rust file with merge conflicts: {}",
                entry.path
            );
        }
        if !matches!(entry.mode.as_str(), "100644" | "100755") {
            skipped.push(entry.path);
            continue;
        }
        let original = git::cat_blob(&entry.sha)
            .with_context(|| format!("read staged blob for {}", entry.path))?;
        let edition = detect_edition(&options.repo_root, &entry.path);
        let output = rustfmt(&original, edition)
            .with_context(|| format!("format staged Rust file {}", entry.path))?;
        if output == original {
            unchanged.push(entry.path);
            continue;
        }
        formatted.push(FormattedBlob {
            mirror_worktree: !dirty_paths.contains(&entry.path),
            path: entry.path,
            mode: entry.mode,
            old_sha: entry.sha,
            original,
            formatted: output,
        });
    }

    let mut updates = Vec::new();
    for item in formatted {
        let new_sha = git::hash_object(&item.path, &item.formatted)
            .with_context(|| format!("hash formatted Rust file {}", item.path))?;
        updates.push(PendingUpdate {
            path: item.path,
            mode: item.mode,
            old_sha: item.old_sha,
            original: item.original,
            formatted: item.formatted,
            mirror_worktree: item.mirror_worktree,
            new_sha,
        });
    }

    verify_index_unchanged(&updates)?;
    let index_info: Vec<(&str, &str, &str)> = updates
        .iter()
        .map(|item| {
            (
                item.mode.as_str(),
                item.new_sha.as_str(),
                item.path.as_str(),
            )
        })
        .collect();
    git::update_index(&index_info)?;

    for update in &updates {
        if should_mirror_worktree(options.repo_root.as_path(), update) {
            let path = git::absolute_path(&options.repo_root, &update.path);
            std::fs::write(&path, &update.formatted)
                .with_context(|| format!("write formatted worktree file {}", path.display()))?;
        }
    }

    Ok(FormatSummary {
        formatted: updates.into_iter().map(|item| item.path).collect(),
        unchanged,
        skipped,
    })
}

fn verify_index_unchanged(updates: &[PendingUpdate]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let paths: Vec<String> = updates.iter().map(|item| item.path.clone()).collect();
    let entries = git::index_entries(&paths)?;
    let by_path: HashMap<&str, &git::IndexEntry> = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    for update in updates {
        let entry = by_path
            .get(update.path.as_str())
            .with_context(|| format!("staged Rust file disappeared: {}", update.path))?;
        if entry.mode != update.mode || entry.sha != update.old_sha || entry.stage != 0 {
            bail!("staged Rust file changed while formatting: {}", update.path);
        }
    }
    Ok(())
}

fn should_mirror_worktree(repo_root: &Path, update: &PendingUpdate) -> bool {
    if !update.mirror_worktree {
        return false;
    }
    let path = git::absolute_path(repo_root, &update.path);
    std::fs::read(path)
        .map(|bytes| bytes == update.original)
        .unwrap_or(false)
}

fn rustfmt(input: &[u8], edition: &'static str) -> Result<Vec<u8>> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", edition, "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run rustfmt")?;
    child
        .stdin
        .as_mut()
        .context("open rustfmt stdin")?
        .write_all(input)
        .context("write rustfmt stdin")?;
    let output = child.wait_with_output().context("wait for rustfmt")?;
    if !output.status.success() {
        bail!(
            "rustfmt failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn detect_edition(repo_root: &Path, path: &str) -> &'static str {
    let mut dir = git::absolute_path(repo_root, path)
        .parent()
        .map(Path::to_path_buf);
    while let Some(current) = dir {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.is_file() {
            if let Ok(text) = std::fs::read_to_string(cargo_toml) {
                if let Some(edition) = parse_package_edition(&text) {
                    return edition;
                }
                if let Some(edition) = parse_workspace_package_edition(&text) {
                    return edition;
                }
            }
        }
        if current == repo_root {
            break;
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    "2024"
}

fn parse_package_edition(text: &str) -> Option<&'static str> {
    parse_table_edition(text, "[package]")
}

fn parse_workspace_package_edition(text: &str) -> Option<&'static str> {
    parse_table_edition(text, "[workspace.package]")
}

fn parse_table_edition(text: &str, table: &str) -> Option<&'static str> {
    let mut in_table = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_table = line == table;
            continue;
        }
        if !in_table {
            continue;
        }
        let Some(rest) = line.strip_prefix("edition") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = rest.trim().trim_matches('"');
        match value {
            "2015" => return Some("2015"),
            "2018" => return Some("2018"),
            "2021" => return Some("2021"),
            "2024" => return Some("2024"),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_edition() {
        let text = r#"
[workspace]
members = []

[package]
name = "sample"
edition = "2021"
"#;
        assert_eq!(parse_package_edition(text), Some("2021"));
    }

    #[test]
    fn parses_workspace_package_edition() {
        let text = r#"
[workspace.package]
edition = "2021"
"#;
        assert_eq!(parse_workspace_package_edition(text), Some("2021"));
    }
}
