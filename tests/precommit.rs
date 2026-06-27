use std::path::Path;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use tempfile::tempdir;

fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.name", "Checkle Test"]);
    git(
        dir.path(),
        &["config", "user.email", "checkle@example.test"],
    );
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    dir
}

#[test]
fn format_staged_formats_index_and_clean_worktree() {
    let dir = init_repo();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn sample(){println!(\"hi\");}\n",
    )
    .unwrap();
    git(dir.path(), &["add", "Cargo.toml", "src/lib.rs"]);

    Command::new(cargo_bin("checkle"))
        .current_dir(dir.path())
        .args(["format-staged"])
        .assert()
        .success()
        .stderr(predicate::str::contains("formatted 1 staged Rust file"));

    let staged = git(dir.path(), &["show", ":src/lib.rs"]);
    let worktree = std::fs::read(dir.path().join("src/lib.rs")).unwrap();
    assert_eq!(staged, worktree);
    assert!(String::from_utf8_lossy(&staged).contains("pub fn sample()"));
}

#[test]
fn format_staged_preserves_unstaged_worktree_edits() {
    let dir = init_repo();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn sample(){println!(\"hi\");}\n",
    )
    .unwrap();
    git(dir.path(), &["add", "Cargo.toml", "src/lib.rs"]);
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn sample(){println!(\"unstaged\");}\n",
    )
    .unwrap();

    Command::new(cargo_bin("checkle"))
        .current_dir(dir.path())
        .args(["format-staged"])
        .assert()
        .success();

    let staged = String::from_utf8(git(dir.path(), &["show", ":src/lib.rs"])).unwrap();
    let worktree = std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(staged.contains("println!(\"hi\")"));
    assert!(worktree.contains("println!(\"unstaged\")"));
}

#[test]
fn pre_commit_skips_docs_only_changes() {
    let dir = init_repo();
    std::fs::write(dir.path().join("README.md"), "# sample\n").unwrap();
    std::fs::write(
        dir.path().join("checkle.toml"),
        "[[check]]\nname = \"fail\"\ncommand = [\"sh\", \"-c\", \"exit 9\"]\n\n[[group]]\nname = \"all\"\nchecks = [\"fail\"]\n",
    )
    .unwrap();
    git(dir.path(), &["add", "README.md"]);

    Command::new(cargo_bin("checkle"))
        .current_dir(dir.path())
        .args(["pre-commit"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "skip pre-commit checks: only documentation or media files staged",
        ));
}

#[test]
fn pre_commit_runs_configured_checks() {
    let dir = init_repo();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn sample() {}\n").unwrap();
    std::fs::write(
        dir.path().join("checkle.toml"),
        "[[check]]\nname = \"quick\"\ncommand = [\"sh\", \"-c\", \"printf checked\"]\n\n[[group]]\nname = \"all\"\nchecks = [\"quick\"]\n",
    )
    .unwrap();
    git(
        dir.path(),
        &["add", "Cargo.toml", "src/lib.rs", "checkle.toml"],
    );

    Command::new(cargo_bin("checkle"))
        .current_dir(dir.path())
        .args(["pre-commit"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ok quick"));
}
