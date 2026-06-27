use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexEntry {
    pub mode: String,
    pub sha: String,
    pub stage: u8,
    pub path: String,
}

pub(crate) fn repo_root() -> Result<PathBuf> {
    let output = git_output(["rev-parse", "--show-toplevel"])?;
    let text = String::from_utf8(output).context("decode git repository root")?;
    Ok(PathBuf::from(text.trim_end()))
}

pub(crate) fn staged_paths(diff_filter: &str, patterns: &[&str]) -> Result<Vec<String>> {
    let mut args = vec![
        "diff".to_string(),
        "--cached".to_string(),
        "--name-only".to_string(),
        "-z".to_string(),
        format!("--diff-filter={diff_filter}"),
        "--".to_string(),
    ];
    args.extend(patterns.iter().map(|pattern| pattern.to_string()));
    nul_strings(git_output(args)?)
}

pub(crate) fn index_entries(paths: &[String]) -> Result<Vec<IndexEntry>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec![
        "ls-files".to_string(),
        "-s".to_string(),
        "-z".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().cloned());
    git_output(args)?
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(parse_index_entry)
        .collect()
}

pub(crate) fn dirty_paths(paths: &[String]) -> Result<HashSet<String>> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut args = vec![
        "diff".to_string(),
        "--name-only".to_string(),
        "-z".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().cloned());
    Ok(nul_strings(git_output(args)?)?.into_iter().collect())
}

pub(crate) fn worktree_has_unstaged_or_untracked() -> Result<bool> {
    if !git_status(["diff", "--quiet", "--ignore-submodules", "--"])? {
        return Ok(true);
    }
    let output = git_output(["ls-files", "--others", "--exclude-standard"])?;
    Ok(!output.is_empty())
}

pub(crate) fn stash_push_keep_index(message: &str) -> Result<()> {
    let output = Command::new("git")
        .args([
            "stash",
            "push",
            "--quiet",
            "--keep-index",
            "--include-untracked",
            "-m",
            message,
        ])
        .output()
        .context("run git stash push")?;
    if !output.status.success() {
        bail!(
            "git stash push failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub(crate) fn stash_pop() -> Result<()> {
    let output = Command::new("git")
        .args(["stash", "pop", "--quiet"])
        .output()
        .context("run git stash pop")?;
    if !output.status.success() {
        bail!(
            "git stash pop failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub(crate) fn cat_blob(sha: &str) -> Result<Vec<u8>> {
    git_output(["cat-file", "blob", sha])
}

pub(crate) fn hash_object(path: &str, bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("git")
        .arg("hash-object")
        .arg("-w")
        .arg(format!("--path={path}"))
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run git hash-object")?;
    child
        .stdin
        .as_mut()
        .context("open git hash-object stdin")?
        .write_all(bytes)
        .context("write git hash-object stdin")?;
    let output = child
        .wait_with_output()
        .context("wait for git hash-object")?;
    if !output.status.success() {
        bail!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8(output.stdout).context("decode git hash-object output")?;
    Ok(text.trim_end().to_string())
}

pub(crate) fn update_index(entries: &[(&str, &str, &str)]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut input = Vec::new();
    for (mode, sha, path) in entries {
        input.extend_from_slice(mode.as_bytes());
        input.push(b' ');
        input.extend_from_slice(sha.as_bytes());
        input.push(b'\t');
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut child = Command::new("git")
        .args(["update-index", "-z", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run git update-index")?;
    child
        .stdin
        .as_mut()
        .context("open git update-index stdin")?
        .write_all(&input)
        .context("write git update-index stdin")?;
    let output = child
        .wait_with_output()
        .context("wait for git update-index")?;
    if !output.status.success() {
        bail!(
            "git update-index failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub(crate) fn git_status<I, S>(args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new("git").args(args).status().context("run git")?;
    Ok(status.success())
}

pub(crate) fn git_output<I, S>(args: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git").args(args).output().context("run git")?;
    if !output.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

pub(crate) fn nul_strings(bytes: Vec<u8>) -> Result<Vec<String>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
        .map(|item| String::from_utf8(item.to_vec()).context("decode git path"))
        .collect()
}

fn parse_index_entry(record: &[u8]) -> Result<IndexEntry> {
    let text = String::from_utf8(record.to_vec()).context("decode git index entry")?;
    let (meta, path) = text
        .split_once('\t')
        .with_context(|| format!("parse git index entry: {text}"))?;
    let mut fields = meta.split_whitespace();
    let mode = fields.next().context("parse git index mode")?.to_string();
    let sha = fields.next().context("parse git index sha")?.to_string();
    let stage = fields
        .next()
        .context("parse git index stage")?
        .parse()
        .context("parse git index stage number")?;
    Ok(IndexEntry {
        mode,
        sha,
        stage,
        path: path.to_string(),
    })
}

pub(crate) fn absolute_path(repo_root: &Path, path: &str) -> PathBuf {
    repo_root.join(path)
}
