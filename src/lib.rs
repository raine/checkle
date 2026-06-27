use std::collections::VecDeque;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub location: Option<String>,
    pub code: Option<String>,
    pub message: String,
    pub help: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFailure {
    pub name: String,
    pub output: Vec<String>,
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
            if let Some(location) = &diagnostic.location {
                output.push_str(location);
                if let Some(code) = &diagnostic.code {
                    output.push(' ');
                    output.push_str(code);
                }
                output.push('\n');
            } else if let Some(code) = &diagnostic.code {
                output.push_str(code);
                output.push('\n');
            } else {
                output.push_str("diagnostic\n");
            }
            output.push_str("  ");
            output.push_str(&diagnostic.message);
            output.push('\n');
            if let Some(help) = &diagnostic.help {
                output.push_str("  help: ");
                output.push_str(help);
                output.push('\n');
            }
            output.push('\n');
        }

        for failure in &self.test_failures {
            output.push_str("failed: ");
            output.push_str(&failure.name);
            output.push('\n');
            for line in &failure.output {
                output.push_str("  ");
                output.push_str(line);
                output.push('\n');
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

    let summary = summarize(options.mode, &combined);
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
    let text = String::from_utf8_lossy(bytes);
    let mut summary = Summary {
        diagnostics: Vec::new(),
        test_failures: Vec::new(),
        text_lines: Vec::new(),
    };

    if matches!(mode, Mode::Auto | Mode::Cargo | Mode::Nextest) {
        summary.diagnostics.extend(cargo_diagnostics(text.as_ref()));
    }
    if matches!(mode, Mode::Auto | Mode::Nextest) {
        summary.test_failures.extend(nextest_failures(text.as_ref()));
        if summary.test_failures.is_empty() {
            summary.text_lines.extend(nextest_text_summary(text.as_ref()));
        }
    }
    if matches!(mode, Mode::Rustfmt) {
        summary.text_lines.extend(rustfmt_summary(text.as_ref()));
    }
    if matches!(mode, Mode::CargoDeny) {
        summary.text_lines.extend(grep_summary(
            text.as_ref(),
            80,
            &["error[", "warning[", "advisories", "bans", "licenses", "sources", "    ├", "    └", "    │"],
        ));
    }
    if matches!(mode, Mode::CargoMachete) {
        summary.text_lines.extend(grep_summary(
            text.as_ref(),
            80,
            &["Error:", "warning:", "The following dependencies seem to be unused", "  "],
        ));
    }
    if matches!(mode, Mode::Auto)
        && summary.diagnostics.is_empty()
        && summary.test_failures.is_empty()
        && summary.text_lines.is_empty()
    {
        summary.text_lines.extend(grep_summary(
            text.as_ref(),
            80,
            &["error:", "warning:", "   -->", "help:", "FAIL", "Summary"],
        ));
    }

    summary
}

fn safe_label(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' })
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
    spans: Vec<CargoSpan>,
    children: Vec<CargoChild>,
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
}

#[derive(Debug, Deserialize)]
struct CargoChild {
    level: String,
    message: String,
}

fn cargo_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<CargoEvent>(line).ok())
        .filter(|event| event.reason.as_deref() == Some("compiler-message"))
        .filter_map(|event| event.message)
        .filter(|message| message.level == "error")
        .map(|message| {
            let span = message
                .spans
                .iter()
                .find(|span| span.is_primary)
                .cloned()
                .or_else(|| message.spans.first().cloned());
            let help = message
                .children
                .iter()
                .find(|child| child.level == "help" && !child.message.is_empty())
                .map(|child| child.message.clone());
            Diagnostic {
                location: span.map(|span| format!("{}:{}:{}", span.file_name, span.line_start, span.column_start)),
                code: message.code.map(|code| code.code),
                message: message.message,
                help,
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct NextestEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    event: Option<String>,
    name: Option<String>,
    stdout: Option<String>,
}

fn nextest_failures(text: &str) -> Vec<TestFailure> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<NextestEvent>(line).ok())
        .filter(|event| event.kind.as_deref() == Some("test") && event.event.as_deref() == Some("failed"))
        .filter_map(|event| {
            Some(TestFailure {
                name: event.name?,
                output: recent_lines(event.stdout.as_deref().unwrap_or(""), 12),
            })
        })
        .collect()
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
        if line.starts_with("Diff in") || line.starts_with("Error writing files") || line.starts_with("error:") {
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
    tail(text.lines().filter(|line| !line.is_empty()).map(ToOwned::to_owned).collect(), limit)
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
{"reason":"compiler-message","message":{"level":"error","message":"sample failure","code":{"code":"clippy::sample"},"spans":[{"file_name":"src/lib.rs","line_start":1,"column_start":2,"is_primary":true}],"children":[{"level":"help","message":"try sample fix"}]}}
{"reason":"compiler-artifact"}
"#;
        let summary = summarize(Mode::Cargo, input.as_bytes());
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(summary.diagnostics[0].location.as_deref(), Some("src/lib.rs:1:2"));
        assert_eq!(summary.diagnostics[0].code.as_deref(), Some("clippy::sample"));
        assert_eq!(summary.diagnostics[0].help.as_deref(), Some("try sample fix"));
    }

    #[test]
    fn summarizes_nextest_json_failures() {
        let input = r#"{"type":"test","event":"failed","name":"crate::test_name","stdout":"a\nb\nc"}
{"type":"suite","event":"failed"}
"#;
        let summary = summarize(Mode::Nextest, input.as_bytes());
        assert_eq!(summary.test_failures.len(), 1);
        assert_eq!(summary.test_failures[0].name, "crate::test_name");
        assert_eq!(summary.test_failures[0].output, vec!["a", "b", "c"]);
    }

    #[test]
    fn summarizes_nextest_text_failures() {
        let input = "        FAIL [   1.000s] crate test_name\n--- STDOUT ---\nline1\nline2\n------------\nSummary [   1.000s] 1 test run: 0 passed, 1 failed\n";
        let summary = summarize(Mode::Nextest, input.as_bytes());
        assert!(summary.text_lines.iter().any(|line| line.contains("FAIL")));
        assert!(summary.text_lines.iter().any(|line| line == "  line1"));
        assert!(summary.text_lines.iter().any(|line| line.starts_with("Summary")));
    }

    #[test]
    fn summarizes_rustfmt_diffs() {
        let input = "noise\nDiff in /tmp/example.rs:1:\n- bad\n+ good\n";
        let summary = summarize(Mode::Rustfmt, input.as_bytes());
        assert_eq!(summary.text_lines, vec!["Diff in /tmp/example.rs:1:", "- bad", "+ good"]);
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
    fn summarizes_cargo_machete_findings() {
        let input = "noise\nThe following dependencies seem to be unused:\n  anyhow\n  serde\n";
        let summary = summarize(Mode::CargoMachete, input.as_bytes());
        assert_eq!(
            summary.text_lines,
            vec!["The following dependencies seem to be unused:", "  anyhow", "  serde"]
        );
    }
}
