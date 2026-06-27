use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn cli_writes_log_and_prints_compact_cargo_summary() {
    let dir = tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    let payload = r#"{"reason":"compiler-message","message":{"level":"error","message":"sample failure","code":{"code":"clippy::sample"},"spans":[{"file_name":"src/lib.rs","line_start":1,"column_start":2,"is_primary":true}],"children":[{"level":"help","message":"try sample fix"}]}}"#;

    Command::cargo_bin("checkle")
        .unwrap()
        .args([
            "--label",
            "clippy",
            "--mode",
            "cargo",
            "--log-dir",
            log_dir.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            &format!("printf '%s\\n' '{payload}'; exit 7"),
        ])
        .assert()
        .code(7)
        .stderr(predicate::str::contains(
            "error: src/lib.rs:1:2 clippy::sample",
        ))
        .stderr(predicate::str::contains("sample failure"))
        .stderr(predicate::str::contains("help: try sample fix"));

    let log = std::fs::read_to_string(log_dir.join("clippy.log")).unwrap();
    assert!(log.contains("compiler-message"));
}

#[test]
fn cli_preserves_success_exit_without_summary() {
    let dir = tempdir().unwrap();
    let log_dir = dir.path().join("logs");

    Command::cargo_bin("checkle")
        .unwrap()
        .args([
            "--label",
            "ok",
            "--log-dir",
            log_dir.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "printf done",
        ])
        .assert()
        .success()
        .stdout("")
        .stderr("");

    let log = std::fs::read_to_string(log_dir.join("ok.log")).unwrap();
    assert_eq!(log, "done");
}
