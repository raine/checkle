use std::time::Duration;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
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
            &format!("printf '%s\n' '{payload}'; exit 7"),
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
    assert_eq!(log, "[stdout] done\n");
}

#[test]
fn cli_streams_log_before_command_exits() {
    let dir = tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    let mut child = std::process::Command::new(cargo_bin("checkle"))
        .args([
            "--label",
            "stream",
            "--log-dir",
            log_dir.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "printf first; sleep 1; printf second; exit 1",
        ])
        .spawn()
        .unwrap();

    let log_path = log_dir.join("stream.log");
    for _ in 0..20 {
        if std::fs::read_to_string(&log_path)
            .map(|log| log.contains("first"))
            .unwrap_or(false)
        {
            let status = child.wait().unwrap();
            assert!(!status.success());
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
    panic!("log did not contain streamed output before command exit");
}

#[test]
fn cli_run_lists_available_checks() {
    Command::cargo_bin("checkle")
        .unwrap()
        .args(["run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("available checks:"))
        .stderr(predicate::str::contains("clippy"))
        .stderr(predicate::str::contains("test"));
}

#[test]
fn cli_run_rejects_unknown_checks() {
    Command::cargo_bin("checkle")
        .unwrap()
        .args(["run", "wat"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown check: wat"));
}

#[test]
fn cli_run_accepts_log_dir_after_subcommand() {
    let dir = tempdir().unwrap();
    let log_dir = dir.path().join("logs");

    Command::cargo_bin("checkle")
        .unwrap()
        .args(["run", "--log-dir", log_dir.to_str().unwrap(), "wat"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown check: wat"));
}

#[test]
fn cli_rejects_invalid_labels() {
    Command::cargo_bin("checkle")
        .unwrap()
        .args(["--label", "bad label", "--", "sh", "-c", "exit 0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("label can only contain ASCII"));
}

#[test]
fn cli_labels_stdout_and_stderr_in_log() {
    let dir = tempdir().unwrap();
    let log_dir = dir.path().join("logs");

    Command::cargo_bin("checkle")
        .unwrap()
        .args([
            "--label",
            "mixed",
            "--log-dir",
            log_dir.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "printf out; printf err >&2; exit 1",
        ])
        .assert()
        .code(1);

    let log = std::fs::read_to_string(log_dir.join("mixed.log")).unwrap();
    assert!(log.contains("[stdout] out"));
    assert!(log.contains("[stderr] err"));
}
