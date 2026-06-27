use std::collections::VecDeque;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: String,
    pub location: Option<String>,
    pub code: Option<String>,
    pub message: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFailure {
    pub name: String,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub diagnostics: Vec<Diagnostic>,
    pub test_failures: Vec<TestFailure>,
    pub text_lines: Vec<String>,
}

impl Summary {
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty() && self.test_failures.is_empty() && self.text_lines.is_empty()
    }

    pub fn render(&self, log_path: &Path) -> String {
        let mut output = format!("full log: {}\n\n", log_path.display());

        for diagnostic in &self.diagnostics {
            output.push_str(&diagnostic.severity);
            output.push_str(": ");
            if let Some(location) = &diagnostic.location {
                output.push_str(location);
                if let Some(code) = &diagnostic.code {
                    output.push(' ');
                    output.push_str(code);
                }
            } else if let Some(code) = &diagnostic.code {
                output.push_str(code);
            } else {
                output.push_str("diagnostic");
            }
            output.push('\n');
            output.push_str("  ");
            output.push_str(&diagnostic.message);
            output.push('\n');
            for detail in &diagnostic.details {
                output.push_str("  ");
                output.push_str(detail);
                output.push('\n');
            }
            output.push('\n');
        }

        for failure in &self.test_failures {
            output.push_str("failed: ");
            output.push_str(&failure.name);
            output.push('\n');
            if !failure.stdout.is_empty() {
                output.push_str("  STDOUT\n");
                for line in &failure.stdout {
                    output.push_str("  ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
            if !failure.stderr.is_empty() {
                output.push_str("  STDERR\n");
                for line in &failure.stderr {
                    output.push_str("  ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
            output.push('\n');
        }

        for line in &self.text_lines {
            output.push_str(line);
            output.push('\n');
        }

        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Cargo,
    Nextest,
    Rustfmt,
    CargoDeny,
    CargoMachete,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub label: String,
    pub mode: Mode,
    pub log_dir: String,
    pub command: Vec<String>,
}

pub fn run(options: RunOptions) -> Result<i32> {
    if options.command.is_empty() {
        bail!("command is required");
    }

    let log_dir = Path::new(&options.log_dir);
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("create log directory {}", log_dir.display()))?;
    let log_path = log_dir.join(format!("{}.log", safe_label(&options.label)));

    let output = Command::new(&options.command[0])
        .args(&options.command[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("run {}", options.command[0]))?;

    let mut combined = output.stdout;
    combined.extend_from_slice(&output.stderr);
    std::fs::write(&log_path, &combined)
        .with_context(|| format!("write log {}", log_path.display()))?;

    let status = output.status.code().unwrap_or(1);
    if status == 0 {
        return Ok(0);
    }

    let summary = summarize_for_command(options.mode, &options.command, &combined);
    if summary.is_empty() {
        eprintln!("full log: {}\n", log_path.display());
        eprintln!("no compact diagnostics found; showing recent log output:\n");
        eprint!("{}", recent_text(&combined, 80));
    } else {
        eprint!("{}", summary.render(&log_path));
    }

    Ok(status)
}

pub fn summarize(mode: Mode, bytes: &[u8]) -> Summary {
    summarize_for_command(mode, &[], bytes)
}

pub fn summarize_for_command(mode: Mode, command: &[String], bytes: &[u8]) -> Summary {
    let text = String::from_utf8_lossy(bytes);
    match detect_mode(mode, command, text.as_ref()) {
        Mode::Cargo => Summary {
            diagnostics: cargo_diagnostics(text.as_ref()),
            test_failures: Vec::new(),
            text_lines: Vec::new(),
        },
        Mode::Nextest => {
            let test_failures = nextest_failures(text.as_ref());
            let text_lines = if test_failures.is_empty() {
                nextest_text_summary(text.as_ref())
            } else {
                Vec::new()
            };
            Summary {
                diagnostics: Vec::new(),
                test_failures,
                text_lines,
            }
        }
        Mode::Rustfmt => Summary {
            diagnostics: Vec::new(),
            test_failures: Vec::new(),
            text_lines: rustfmt_summary(text.as_ref()),
        },
        Mode::CargoDeny => Summary {
            diagnostics: Vec::new(),
            test_failures: Vec::new(),
            text_lines: cargo_deny_summary(text.as_ref()),
        },
        Mode::CargoMachete => Summary {
            diagnostics: Vec::new(),
            test_failures: Vec::new(),
            text_lines: cargo_machete_summary(text.as_ref()),
        },
        Mode::Auto => Summary {
            diagnostics: Vec::new(),
            test_failures: Vec::new(),
            text_lines: fallback_summary(text.as_ref()),
        },
    }
}

fn detect_mode(mode: Mode, command: &[String], text: &str) -> Mode {
    if mode != Mode::Auto {
        return mode;
    }
    if looks_like_cargo_json(text) {
        return Mode::Cargo;
    }
    if looks_like_nextest_json(text)
        || looks_like_nextest_command(command)
        || looks_like_nextest_text(text)
    {
        return Mode::Nextest;
    }
    if looks_like_rustfmt_command(command) || looks_like_rustfmt_output(text) {
        return Mode::Rustfmt;
    }
    if looks_like_cargo_deny_command(command) || looks_like_cargo_deny_output(text) {
        return Mode::CargoDeny;
    }
    if looks_like_cargo_machete_command(command) || looks_like_cargo_machete_output(text) {
        return Mode::CargoMachete;
    }
    Mode::Auto
}

fn looks_like_cargo_json(text: &str) -> bool {
    text.lines().any(|line| {
        serde_json::from_str::<CargoEvent>(line)
            .map(|event| event.reason.as_deref() == Some("compiler-message"))
            .unwrap_or(false)
    })
}

fn looks_like_nextest_json(text: &str) -> bool {
    text.lines().any(|line| {
        serde_json::from_str::<NextestEvent>(line)
            .map(|event| event.kind.as_deref() == Some("test"))
            .unwrap_or(false)
    })
}

fn looks_like_nextest_text(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("FAIL ")
            || trimmed.starts_with("TRY ") && trimmed.contains(" FAIL ")
            || trimmed.starts_with("LEAK-FAIL ")
            || trimmed.starts_with("TIMEOUT ")
            || line.starts_with("Summary ") && line.contains(" test run")
    })
}

fn looks_like_nextest_command(command: &[String]) -> bool {
    command
        .windows(2)
        .any(|words| words[0] == "cargo" && words[1] == "nextest")
        || command.iter().any(|word| word == "cargo-nextest")
}

fn looks_like_rustfmt_command(command: &[String]) -> bool {
    command.iter().any(|word| word == "rustfmt")
        || command
            .windows(2)
            .any(|words| words[0] == "cargo" && words[1] == "fmt")
}

fn looks_like_rustfmt_output(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("Diff in") || line.starts_with("Error writing files"))
}

fn looks_like_cargo_deny_command(command: &[String]) -> bool {
    command
        .windows(2)
        .any(|words| words[0] == "cargo" && words[1] == "deny")
        || command.iter().any(|word| word == "cargo-deny")
}

fn looks_like_cargo_deny_output(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("error[")
            || line.starts_with("warning[")
            || line.starts_with("advisories")
            || line.starts_with("bans")
            || line.starts_with("licenses")
            || line.starts_with("sources")
    })
}

fn looks_like_cargo_machete_command(command: &[String]) -> bool {
    command
        .windows(2)
        .any(|words| words[0] == "cargo" && words[1] == "machete")
        || command.iter().any(|word| word == "cargo-machete")
}

fn looks_like_cargo_machete_output(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("The following dependencies seem to be unused")
            || line.starts_with("Error:") && line.contains("unused")
    })
}

fn cargo_deny_summary(text: &str) -> Vec<String> {
    grep_summary(
        text,
        80,
        &[
            "error[",
            "warning[",
            "advisories",
            "bans",
            "licenses",
            "sources",
            "    ├",
            "    └",
            "    │",
        ],
    )
}

fn cargo_machete_summary(text: &str) -> Vec<String> {
    grep_summary(
        text,
        80,
        &[
            "Error:",
            "warning:",
            "The following dependencies seem to be unused",
            "  ",
        ],
    )
}

fn fallback_summary(text: &str) -> Vec<String> {
    grep_summary(
        text,
        80,
        &["error:", "warning:", "   -->", "help:", "FAIL", "Summary"],
    )
}

fn safe_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct CargoEvent {
    reason: Option<String>,
    message: Option<CargoMessage>,
}

#[derive(Debug, Deserialize)]
struct CargoMessage {
    level: String,
    message: String,
    code: Option<CargoCode>,
    #[serde(default)]
    spans: Vec<CargoSpan>,
    #[serde(default)]
    children: Vec<CargoChild>,
    rendered: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CargoCode {
    code: String,
}

#[derive(Debug, Deserialize, Clone)]
struct CargoSpan {
    file_name: String,
    line_start: usize,
    column_start: usize,
    is_primary: bool,
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CargoChild {
    level: String,
    message: String,
    #[serde(default)]
    spans: Vec<CargoSpan>,
}

fn cargo_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<CargoEvent>(line).ok())
        .filter(|event| event.reason.as_deref() == Some("compiler-message"))
        .filter_map(|event| event.message)
        .filter(|message| matches!(message.level.as_str(), "error" | "warning"))
        .map(|message| {
            let span = message
                .spans
                .iter()
                .find(|span| span.is_primary)
                .cloned()
                .or_else(|| message.spans.first().cloned());
            let mut details = diagnostic_details(&message);
            if let Some(rendered) = &message.rendered {
                append_rendered_details(&mut details, rendered);
            }
            Diagnostic {
                severity: message.level,
                location: span.map(|span| {
                    format!(
                        "{}:{}:{}",
                        span.file_name, span.line_start, span.column_start
                    )
                }),
                code: message.code.map(|code| code.code),
                message: message.message,
                details,
            }
        })
        .collect()
}

fn diagnostic_details(message: &CargoMessage) -> Vec<String> {
    let mut details = Vec::new();
    for span in message.spans.iter().filter(|span| !span.is_primary).take(4) {
        if let Some(label) = &span.label {
            if !label.is_empty() {
                details.push(format!(
                    "{}:{}:{}: {}",
                    span.file_name, span.line_start, span.column_start, label
                ));
            }
        }
    }
    for child in message.children.iter().take(6) {
        if !child.message.is_empty()
            && matches!(child.level.as_str(), "help" | "note" | "warning" | "error")
        {
            details.push(format!("{}: {}", child.level, child.message));
        }
        for span in child.spans.iter().filter(|span| !span.is_primary).take(2) {
            if let Some(label) = &span.label {
                if !label.is_empty() {
                    details.push(format!(
                        "{}:{}:{}: {}",
                        span.file_name, span.line_start, span.column_start, label
                    ));
                }
            }
        }
    }
    tail(details, 12)
}

fn append_rendered_details(details: &mut Vec<String>, rendered: &str) {
    let mut rendered_lines: Vec<String> = rendered
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty()
                && (trimmed.starts_with("= note:")
                    || trimmed.starts_with("= help:")
                    || trimmed.starts_with("help:")
                    || trimmed.starts_with("note:"))
        })
        .take(6)
        .map(ToOwned::to_owned)
        .collect();
    details.append(&mut rendered_lines);
    if details.len() > 12 {
        *details = tail(std::mem::take(details), 12);
    }
}

#[derive(Debug, Deserialize)]
struct NextestEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    event: Option<String>,
    name: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
}

fn nextest_failures(text: &str) -> Vec<TestFailure> {
    let mut failures = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for event in text
        .lines()
        .filter_map(|line| serde_json::from_str::<NextestEvent>(line).ok())
        .filter(|event| {
            event.kind.as_deref() == Some("test") && event.event.as_deref() == Some("failed")
        })
    {
        let Some(name) = event.name else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        failures.push(TestFailure {
            name,
            stdout: recent_lines(event.stdout.as_deref().unwrap_or(""), 12),
            stderr: recent_lines(event.stderr.as_deref().unwrap_or(""), 12),
        });
    }

    failures
}

fn nextest_text_summary(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_output = false;
    let mut output_count = 0;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("FAIL ")
            || trimmed.starts_with("TRY ") && trimmed.contains(" FAIL ")
            || trimmed.starts_with("LEAK-FAIL ")
            || trimmed.starts_with("TIMEOUT ")
        {
            lines.push(line.to_string());
            continue;
        }
        if line == "--- STDOUT ---" || line == "--- STDERR ---" {
            in_output = true;
            output_count = 0;
            lines.push(line.to_string());
            continue;
        }
        if line == "------------" {
            in_output = false;
            lines.push(line.to_string());
            continue;
        }
        if in_output && output_count < 12 {
            lines.push(format!("  {line}"));
            output_count += 1;
            continue;
        }
        if line.starts_with("error:")
            || line.starts_with("warning:")
            || line.starts_with("   -->")
            || line.starts_with("Summary ")
        {
            lines.push(line.to_string());
        }
    }

    tail(lines, 120)
}

fn rustfmt_summary(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut remaining = 0;
    for line in text.lines() {
        if line.starts_with("Diff in")
            || line.starts_with("Error writing files")
            || line.starts_with("error:")
        {
            remaining = 20;
            lines.push(line.to_string());
            continue;
        }
        if remaining > 0 {
            lines.push(line.to_string());
            remaining -= 1;
        }
    }
    tail(lines, 80)
}

fn grep_summary(text: &str, limit: usize, prefixes: &[&str]) -> Vec<String> {
    tail(
        text.lines()
            .filter(|line| prefixes.iter().any(|prefix| line.starts_with(prefix)))
            .map(ToOwned::to_owned)
            .collect(),
        limit,
    )
}

fn recent_text(bytes: &[u8], limit: usize) -> String {
    recent_lines(String::from_utf8_lossy(bytes).as_ref(), limit).join("\n") + "\n"
}

fn recent_lines(text: &str, limit: usize) -> Vec<String> {
    tail(
        text.lines()
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        limit,
    )
}

fn tail<T>(items: Vec<T>, limit: usize) -> Vec<T> {
    let mut queue = VecDeque::with_capacity(limit);
    for item in items {
        if queue.len() == limit {
            queue.pop_front();
        }
        queue.push_back(item);
    }
    queue.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_cargo_json_diagnostics() {
        let input = r#"not json
{"reason":"compiler-message","message":{"level":"error","message":"sample failure","code":{"code":"clippy::sample"},"spans":[{"file_name":"src/lib.rs","line_start":1,"column_start":2,"is_primary":true}],"children":[{"level":"help","message":"try sample fix","spans":[]}]}}
{"reason":"compiler-artifact"}
"#;
        let summary = summarize(Mode::Cargo, input.as_bytes());
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(summary.diagnostics[0].severity, "error");
        assert_eq!(
            summary.diagnostics[0].location.as_deref(),
            Some("src/lib.rs:1:2")
        );
        assert_eq!(
            summary.diagnostics[0].code.as_deref(),
            Some("clippy::sample")
        );
        assert_eq!(summary.diagnostics[0].details, vec!["help: try sample fix"]);
    }

    #[test]
    fn summarizes_cargo_warnings_and_context() {
        let input = r#"{"reason":"compiler-message","message":{"level":"warning","message":"lint failure","code":{"code":"clippy::dbg_macro"},"spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":4,"is_primary":true,"label":"primary label"},{"file_name":"src/lib.rs","line_start":8,"column_start":9,"is_primary":false,"label":"secondary label"}],"children":[{"level":"help","message":"remove dbg","spans":[]},{"level":"note","message":"lint comes from command line","spans":[{"file_name":"src/main.rs","line_start":1,"column_start":1,"is_primary":false,"label":"note span"}]}],"rendered":"warning: lint failure\n  = note: rendered note\n  = help: rendered help\n"}}"#;
        let summary = summarize(Mode::Cargo, input.as_bytes());
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(summary.diagnostics[0].severity, "warning");
        assert_eq!(
            summary.diagnostics[0].location.as_deref(),
            Some("src/lib.rs:3:4")
        );
        assert_eq!(
            summary.diagnostics[0].code.as_deref(),
            Some("clippy::dbg_macro")
        );
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"src/lib.rs:8:9: secondary label".to_string())
        );
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"help: remove dbg".to_string())
        );
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"note: lint comes from command line".to_string())
        );
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"src/main.rs:1:1: note span".to_string())
        );
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"  = note: rendered note".to_string())
        );
    }

    #[test]
    fn renders_diagnostic_severity() {
        let summary = Summary {
            diagnostics: vec![Diagnostic {
                severity: "warning".to_string(),
                location: Some("src/lib.rs:1:2".to_string()),
                code: Some("clippy::sample".to_string()),
                message: "sample failure".to_string(),
                details: vec!["help: try sample fix".to_string()],
            }],
            test_failures: Vec::new(),
            text_lines: Vec::new(),
        };
        let rendered = summary.render(Path::new("target/check-logs/clippy.log"));
        assert!(rendered.contains("warning: src/lib.rs:1:2 clippy::sample"));
        assert!(rendered.contains("  help: try sample fix"));
    }

    #[test]
    fn summarizes_nextest_json_failures() {
        let input = r#"{"type":"test","event":"failed","name":"crate::test_name","stdout":"a\nb\nc","stderr":"panic\nbacktrace"}
{"type":"suite","event":"failed"}
"#;
        let summary = summarize(Mode::Nextest, input.as_bytes());
        assert_eq!(summary.test_failures.len(), 1);
        assert_eq!(summary.test_failures[0].name, "crate::test_name");
        assert_eq!(summary.test_failures[0].stdout, vec!["a", "b", "c"]);
        assert_eq!(summary.test_failures[0].stderr, vec!["panic", "backtrace"]);
    }

    #[test]
    fn renders_nextest_stdout_and_stderr() {
        let summary = Summary {
            diagnostics: Vec::new(),
            test_failures: vec![TestFailure {
                name: "crate::test_name".to_string(),
                stdout: vec!["stdout line".to_string()],
                stderr: vec!["stderr line".to_string()],
            }],
            text_lines: Vec::new(),
        };
        let rendered = summary.render(Path::new("target/check-logs/test.log"));
        assert!(rendered.contains("  STDOUT\n  stdout line"));
        assert!(rendered.contains("  STDERR\n  stderr line"));
    }

    #[test]
    fn deduplicates_nextest_json_failures() {
        let input = r#"{"type":"test","event":"failed","name":"crate::test_name","stdout":"first"}
{"type":"test","event":"failed","name":"crate::test_name","stdout":"second"}
"#;
        let summary = summarize(Mode::Nextest, input.as_bytes());
        assert_eq!(summary.test_failures.len(), 1);
        assert_eq!(summary.test_failures[0].stdout, vec!["first"]);
    }

    #[test]
    fn summarizes_nextest_text_failures() {
        let input = "        FAIL [   1.000s] crate test_name\n--- STDOUT ---\nline1\nline2\n------------\nSummary [   1.000s] 1 test run: 0 passed, 1 failed\n";
        let summary = summarize(Mode::Nextest, input.as_bytes());
        assert!(summary.text_lines.iter().any(|line| line.contains("FAIL")));
        assert!(summary.text_lines.iter().any(|line| line == "  line1"));
        assert!(
            summary
                .text_lines
                .iter()
                .any(|line| line.starts_with("Summary"))
        );
    }

    #[test]
    fn summarizes_rustfmt_diffs() {
        let input = "noise\nDiff in /tmp/example.rs:1:\n- bad\n+ good\n";
        let summary = summarize(Mode::Rustfmt, input.as_bytes());
        assert_eq!(
            summary.text_lines,
            vec!["Diff in /tmp/example.rs:1:", "- bad", "+ good"]
        );
    }

    #[test]
    fn summarizes_cargo_deny_findings() {
        let input = "noise\nadvisories ok\nerror[duplicate]: duplicate dependency\n    ├ crate-a\n    └ crate-b\n";
        let summary = summarize(Mode::CargoDeny, input.as_bytes());
        assert_eq!(
            summary.text_lines,
            vec![
                "advisories ok",
                "error[duplicate]: duplicate dependency",
                "    ├ crate-a",
                "    └ crate-b",
            ]
        );
    }

    #[test]
    fn auto_detects_supported_formats() {
        let cargo = r#"{"reason":"compiler-message","message":{"level":"error","message":"sample","spans":[],"children":[]}}"#;
        assert_eq!(summarize(Mode::Auto, cargo.as_bytes()).diagnostics.len(), 1);

        let nextest = r#"{"type":"test","event":"failed","name":"crate::test","stderr":"panic"}"#;
        assert_eq!(
            summarize(Mode::Auto, nextest.as_bytes())
                .test_failures
                .len(),
            1
        );

        let rustfmt = "Diff in /tmp/example.rs:1:\n- bad\n+ good\n";
        assert_eq!(
            summarize(Mode::Auto, rustfmt.as_bytes()).text_lines[0],
            "Diff in /tmp/example.rs:1:"
        );

        let deny = "advisories ok\nerror[duplicate]: duplicate dependency\n";
        assert_eq!(
            summarize(Mode::Auto, deny.as_bytes()).text_lines,
            vec!["advisories ok", "error[duplicate]: duplicate dependency"]
        );

        let machete = "The following dependencies seem to be unused:\n  anyhow\n";
        assert_eq!(
            summarize(Mode::Auto, machete.as_bytes()).text_lines,
            vec!["The following dependencies seem to be unused:", "  anyhow"]
        );
    }

    #[test]
    fn auto_uses_command_when_content_is_ambiguous() {
        let command = vec!["cargo".to_string(), "fmt".to_string()];
        let summary = summarize_for_command(Mode::Auto, &command, b"error: bad format\n");
        assert_eq!(summary.text_lines, vec!["error: bad format"]);
    }

    #[test]
    fn auto_uses_one_parser() {
        let input = r#"{"reason":"compiler-message","message":{"level":"error","message":"sample","spans":[],"children":[]}}
error: fallback duplicate
"#;
        let summary = summarize(Mode::Auto, input.as_bytes());
        assert_eq!(summary.diagnostics.len(), 1);
        assert!(summary.text_lines.is_empty());
    }

    #[test]
    fn summarizes_cargo_machete_findings() {
        let input = "noise\nThe following dependencies seem to be unused:\n  anyhow\n  serde\n";
        let summary = summarize(Mode::CargoMachete, input.as_bytes());
        assert_eq!(
            summary.text_lines,
            vec![
                "The following dependencies seem to be unused:",
                "  anyhow",
                "  serde"
            ]
        );
    }
}
