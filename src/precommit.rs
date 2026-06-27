use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::format_staged::{FormatStagedOptions, format_staged_rust};
use crate::{SuiteOptions, SummaryLimits, git, run_suite};

pub struct PreCommitOptions {
    pub checks: Vec<String>,
    pub log_dir: String,
    pub limits: SummaryLimits,
}

pub fn run_pre_commit(options: PreCommitOptions) -> Result<i32> {
    let repo_root = git::repo_root()?;
    std::env::set_current_dir(&repo_root)
        .with_context(|| format!("enter git repository {}", repo_root.display()))?;

    let summary = format_staged_rust(&FormatStagedOptions {
        repo_root: repo_root.clone(),
    })?;
    if !summary.formatted.is_empty() {
        eprintln!(
            "pre-commit: formatted {} staged Rust file(s)",
            summary.formatted.len()
        );
    }
    for path in &summary.skipped {
        eprintln!("pre-commit: skip non-regular Rust path {path}");
    }

    if staged_paths_are_docs_or_media_only()? {
        eprintln!("skip pre-commit checks: only documentation or media files staged");
        return Ok(0);
    }

    let mut stash = StashGuard::create()?;
    let checks = if options.checks.is_empty() {
        vec!["all".to_string()]
    } else {
        options.checks
    };
    let suite_result = run_suite(SuiteOptions {
        checks,
        log_dir: options.log_dir,
        limits: options.limits,
    });
    let restore_result = stash.restore();
    if let Err(error) = restore_result {
        eprintln!("error: failed to restore unstaged changes after pre-commit checks");
        eprintln!("resolve the stash conflict, then run git stash drop when finished");
        eprintln!("{error:#}");
        return Ok(1);
    }

    let code = suite_result?;
    if code != 0 {
        eprintln!("pre-commit checks failed");
        eprintln!("review and stage any fixes, then commit again");
    }
    Ok(code)
}

fn staged_paths_are_docs_or_media_only() -> Result<bool> {
    let paths = git::staged_paths("ACMRD", &["*"])?;
    if paths.is_empty() {
        return Ok(false);
    }
    Ok(paths.iter().all(|path| is_doc_or_media(path)))
}

fn is_doc_or_media(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".md",
        ".markdown",
        ".avif",
        ".gif",
        ".jpeg",
        ".jpg",
        ".png",
        ".svg",
        ".webp",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

struct StashGuard {
    created: bool,
}

impl StashGuard {
    fn create() -> Result<Self> {
        if git::worktree_has_unstaged_or_untracked()? {
            git::stash_push_keep_index("checkle pre-commit unstaged changes")?;
            Ok(Self { created: true })
        } else {
            Ok(Self { created: false })
        }
    }

    fn restore(&mut self) -> Result<()> {
        if self.created {
            git::stash_pop()?;
            self.created = false;
        }
        Ok(())
    }
}

impl Drop for StashGuard {
    fn drop(&mut self) {
        if self.created {
            let _ = git::stash_pop();
        }
    }
}

pub fn format_staged_from_git_root() -> Result<i32> {
    let repo_root: PathBuf = git::repo_root()?;
    std::env::set_current_dir(&repo_root)
        .with_context(|| format!("enter git repository {}", repo_root.display()))?;
    let summary = format_staged_rust(&FormatStagedOptions { repo_root })?;
    if !summary.formatted.is_empty() {
        eprintln!("formatted {} staged Rust file(s)", summary.formatted.len());
    }
    for path in &summary.skipped {
        eprintln!("skip non-regular Rust path {path}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_doc_and_media_paths() {
        assert!(is_doc_or_media("README.md"));
        assert!(is_doc_or_media("assets/logo.PNG"));
        assert!(!is_doc_or_media("src/lib.rs"));
    }
}
