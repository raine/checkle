mod suite;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::Deserialize;

pub use suite::{
    ResolvedChecks, RunSpec, SkippedCheck, SuiteOptions, SuiteStatus, SuiteSummary, execute_specs,
    list_checks, resolve_checks, run_suite, tool_installed,
};

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
        self.render_with_limits(log_path, &SummaryLimits::default())
    }

    pub fn render_with_limits(&self, log_path: &Path, limits: &SummaryLimits) -> String {
        let mut output = format!("full log: {}\n\n", log_path.display());

        let rendered_diagnostics = self.diagnostics.iter().take(limits.max_diagnostics);
        for diagnostic in rendered_diagnostics {
            let mut heading = format!("{}: ", diagnostic.severity);
            if let Some(location) = &diagnostic.location {
                heading.push_str(location);
                if let Some(code) = &diagnostic.code {
                    heading.push(' ');
                    heading.push_str(code);
                }
            } else if let Some(code) = &diagnostic.code {
                heading.push_str(code);
            } else {
                heading.push_str("diagnostic");
            }
            push_limited_line(&mut output, &heading, limits.max_line_chars);
            push_limited_line(
                &mut output,
                &format!("  {}", diagnostic.message),
                limits.max_line_chars,
            );
            for detail in diagnostic.details.iter().take(limits.max_lines) {
                push_limited_line(&mut output, &format!("  {detail}"), limits.max_line_chars);
            }
            if diagnostic.details.len() > limits.max_lines {
                push_limited_line(
                    &mut output,
                    &format!(
                        "  omitted {} diagnostic detail lines; see full log above",
                        diagnostic.details.len() - limits.max_lines
                    ),
                    limits.max_line_chars,
                );
            }
            output.push('\n');
        }
        if self.diagnostics.len() > limits.max_diagnostics {
            push_limited_line(
                &mut output,
                &format!(
                    "omitted {} diagnostics; see full log above",
                    self.diagnostics.len() - limits.max_diagnostics
                ),
                limits.max_line_chars,
            );
            output.push('\n');
        }

        for failure in self.test_failures.iter().take(limits.max_failures) {
            push_limited_line(
                &mut output,
                &format!("failed: {}", failure.name),
                limits.max_line_chars,
            );
            render_stream(&mut output, "STDOUT", &failure.stdout, limits);
            render_stream(&mut output, "STDERR", &failure.stderr, limits);
            output.push('\n');
        }
        if self.test_failures.len() > limits.max_failures {
            push_limited_line(
                &mut output,
                &format!(
                    "omitted {} failed tests; see full log above",
                    self.test_failures.len() - limits.max_failures
                ),
                limits.max_line_chars,
            );
            output.push('\n');
        }

        for line in self.text_lines.iter().take(limits.max_fallback_lines) {
            push_limited_line(&mut output, line, limits.max_line_chars);
        }
        if self.text_lines.len() > limits.max_fallback_lines {
            push_limited_line(
                &mut output,
                &format!(
                    "omitted {} fallback lines; see full log above",
                    self.text_lines.len() - limits.max_fallback_lines
                ),
                limits.max_line_chars,
            );
        }

        output
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryLimits {
    pub max_diagnostics: usize,
    pub max_failures: usize,
    pub max_lines: usize,
    pub max_line_chars: usize,
    pub max_fallback_lines: usize,
}

impl Default for SummaryLimits {
    fn default() -> Self {
        Self {
            max_diagnostics: 20,
            max_failures: 20,
            max_lines: 12,
            max_line_chars: 240,
            max_fallback_lines: 80,
        }
    }
}

fn render_stream(output: &mut String, label: &str, lines: &[String], limits: &SummaryLimits) {
    if lines.is_empty() {
        return;
    }
    output.push_str("  ");
    output.push_str(label);
    output.push('\n');
    for line in lines.iter().take(limits.max_lines) {
        push_limited_line(output, &format!("  {line}"), limits.max_line_chars);
    }
    if lines.len() > limits.max_lines {
        push_limited_line(
            output,
            &format!(
                "  omitted {} {} lines; see full log above",
                lines.len() - limits.max_lines,
                label.to_ascii_lowercase()
            ),
            limits.max_line_chars,
        );
    }
}

fn push_limited_line(output: &mut String, line: &str, max_chars: usize) {
    output.push_str(&limit_line(line, max_chars));
    output.push('\n');
}

fn limit_line(line: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let keep = max_chars.saturating_sub(14);
    let mut trimmed: String = line.chars().take(keep).collect();
    trimmed.push_str("... truncated");
    trimmed
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    #[default]
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
    pub limits: SummaryLimits,
    pub command: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub exit_code: i32,
    pub log_path: PathBuf,
    pub output: Vec<u8>,
}

pub fn run(options: RunOptions) -> Result<i32> {
    if options.command.is_empty() {
        bail!("command is required");
    }

    let result = execute_check(&options)?;
    if result.exit_code == 0 {
        return Ok(0);
    }

    let summary = summarize_for_command(options.mode, &options.command, &result.output);
    let report = render_report(&summary, &result.log_path, &options.limits, &result.output);
    eprint!("{report}");

    Ok(result.exit_code)
}

pub fn execute_check(options: &RunOptions) -> Result<RunResult> {
    if options.command.is_empty() {
        bail!("command is required");
    }

    validate_label(&options.label)?;

    let log_dir = Path::new(&options.log_dir);
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("create log directory {}", log_dir.display()))?;
    let log_path = log_dir.join(format!("{}.log", safe_label(&options.label)));

    let run_output = run_command(&options.command, &log_path)?;
    Ok(RunResult {
        exit_code: status_code(run_output.status),
        log_path,
        output: run_output.output,
    })
}

pub fn render_report(
    summary: &Summary,
    log_path: &Path,
    limits: &SummaryLimits,
    raw_output: &[u8],
) -> String {
    if summary.is_empty() {
        let mut output = format!(
            "full log: {}\n\nno compact diagnostics found; showing recent log output:\n\n",
            log_path.display()
        );
        output.push_str(&recent_text(raw_output, limits.max_fallback_lines));
        return output;
    }
    summary.render_with_limits(log_path, limits)
}

#[derive(Debug)]
struct RunOutput {
    status: ExitStatus,
    output: Vec<u8>,
}

#[derive(Debug)]
enum StreamEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

fn run_command(command: &[String], log_path: &Path) -> Result<RunOutput> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run {}", command[0]))?;

    let stdout = child.stdout.take().context("capture child stdout")?;
    let stderr = child.stderr.take().context("capture child stderr")?;
    let (sender, receiver) = mpsc::channel();
    let stdout_thread = stream_reader(stdout, sender.clone(), StreamKind::Stdout);
    let stderr_thread = stream_reader(stderr, sender, StreamKind::Stderr);

    let mut log =
        File::create(log_path).with_context(|| format!("write log {}", log_path.display()))?;
    let mut output = Vec::new();
    for event in receiver {
        let (label, bytes) = match event {
            StreamEvent::Stdout(bytes) => ("stdout", bytes),
            StreamEvent::Stderr(bytes) => ("stderr", bytes),
        };
        write_stream_event(&mut log, label, &bytes)
            .with_context(|| format!("write log {}", log_path.display()))?;
        output.extend_from_slice(&bytes);
    }

    let status = child
        .wait()
        .with_context(|| format!("wait for {}", command[0]))?;
    join_reader(stdout_thread)?;
    join_reader(stderr_thread)?;
    Ok(RunOutput { status, output })
}

#[derive(Debug, Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

fn stream_reader<R>(
    reader: R,
    sender: mpsc::Sender<StreamEvent>,
    kind: StreamKind,
) -> thread::JoinHandle<Result<()>>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let count = reader.read_until(b'\n', &mut buffer)?;
            if count == 0 {
                break;
            }
            let event = match kind {
                StreamKind::Stdout => StreamEvent::Stdout(buffer.clone()),
                StreamKind::Stderr => StreamEvent::Stderr(buffer.clone()),
            };
            if sender.send(event).is_err() {
                break;
            }
        }
        Ok(())
    })
}

fn write_stream_event(log: &mut File, label: &str, bytes: &[u8]) -> std::io::Result<()> {
    log.write_all(b"[")?;
    log.write_all(label.as_bytes())?;
    log.write_all(b"] ")?;
    log.write_all(bytes)?;
    if !bytes.ends_with(b"\n") {
        log.write_all(b"\n")?;
    }
    log.flush()
}

fn join_reader(handle: thread::JoinHandle<Result<()>>) -> Result<()> {
    match handle.join() {
        Ok(result) => result,
        Err(_) => bail!("stream reader thread panicked"),
    }
}

fn status_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
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
    let structured = cargo_deny_json_summary(text);
    if !structured.is_empty() {
        return structured;
    }

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
            "cargo-machete found the following unused dependencies",
            "cargo-machete didn't find any unused dependencies",
            "\t",
            "  ",
        ],
    )
}

#[derive(Debug, Deserialize)]
struct CargoDenyDiagnostic {
    #[serde(rename = "type")]
    kind: Option<String>,
    fields: Option<CargoDenyDiagnosticFields>,
}

#[derive(Debug, Deserialize)]
struct CargoDenyDiagnosticFields {
    severity: Option<String>,
    message: Option<String>,
    code: Option<String>,
    #[serde(default)]
    labels: Vec<CargoDenyLabel>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoDenyLabel {
    message: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
}

fn cargo_deny_json_summary(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<CargoDenyDiagnostic>(line).ok())
        .filter(|diagnostic| diagnostic.kind.as_deref() == Some("diagnostic"))
        .filter_map(|diagnostic| diagnostic.fields)
        .flat_map(|fields| {
            let mut lines = Vec::new();
            let severity = fields.severity.unwrap_or_else(|| "diagnostic".to_string());
            let message = fields.message.unwrap_or_default();
            let mut heading = severity;
            if let Some(code) = fields.code {
                heading.push('[');
                heading.push_str(&code);
                heading.push(']');
            }
            if !message.is_empty() {
                heading.push_str(": ");
                heading.push_str(&message);
            }
            lines.push(heading);

            for label in fields.labels.iter().take(4) {
                if let Some(message) = &label.message {
                    let location = match (label.line, label.column) {
                        (Some(line), Some(column)) => format!("{line}:{column}: "),
                        (Some(line), None) => format!("{line}: "),
                        _ => String::new(),
                    };
                    lines.push(format!("  {location}{message}"));
                }
            }
            for note in fields.notes.iter().take(4) {
                lines.push(format!("  note: {note}"));
            }
            lines
        })
        .collect()
}

fn fallback_summary(text: &str) -> Vec<String> {
    grep_summary(
        text,
        80,
        &["error:", "warning:", "   -->", "help:", "FAIL", "Summary"],
    )
}

pub fn validate_label(label: &str) -> Result<()> {
    if label.is_empty() {
        bail!("label cannot be empty");
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        bail!("label can only contain ASCII letters, digits, '_', '.', and '-'");
    }
    Ok(())
}

fn safe_label(label: &str) -> String {
    validate_label(label).expect("label validated before log path construction");
    label.to_string()
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

    fn fixture(name: &str) -> &'static [u8] {
        match name {
            "cargo_clippy_warning" => {
                include_bytes!("../tests/fixtures/cargo_clippy_warning.jsonl")
            }
            "cargo_error_rendered" => {
                include_bytes!("../tests/fixtures/cargo_error_rendered.jsonl")
            }
            "cargo_doctest_failure" => {
                include_bytes!("../tests/fixtures/cargo_doctest_failure.jsonl")
            }
            "nextest_human_failure" => {
                include_bytes!("../tests/fixtures/nextest_human_failure.txt")
            }
            "nextest_json_failure" => {
                include_bytes!("../tests/fixtures/nextest_json_failure.jsonl")
            }
            "rustfmt_diff" => include_bytes!("../tests/fixtures/rustfmt_diff.txt"),
            "cargo_deny_json" => include_bytes!("../tests/fixtures/cargo_deny_json.jsonl"),
            "cargo_deny_human" => include_bytes!("../tests/fixtures/cargo_deny_human.txt"),
            "cargo_machete_unused" => include_bytes!("../tests/fixtures/cargo_machete_unused.txt"),
            "ansi_colored_output" => include_bytes!("../tests/fixtures/ansi_colored_output.txt"),
            "multiple_failures" => include_bytes!("../tests/fixtures/multiple_failures.txt"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn summarizes_cargo_json_diagnostics() {
        let summary = summarize(Mode::Cargo, fixture("cargo_error_rendered"));
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(summary.diagnostics[0].severity, "error");
        assert_eq!(
            summary.diagnostics[0].location.as_deref(),
            Some("src/main.rs:7:9")
        );
        assert_eq!(summary.diagnostics[0].code.as_deref(), Some("E0308"));
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"note: expected struct `String` found reference `&str`".to_string())
        );
    }

    #[test]
    fn summarizes_cargo_warnings_and_context() {
        let summary = summarize(Mode::Cargo, fixture("cargo_clippy_warning"));
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(summary.diagnostics[0].severity, "warning");
        assert_eq!(
            summary.diagnostics[0].location.as_deref(),
            Some("src/lib.rs:4:5")
        );
        assert_eq!(
            summary.diagnostics[0].code.as_deref(),
            Some("clippy::dbg_macro")
        );
        assert!(
            summary.diagnostics[0]
                .details
                .contains(&"help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#dbg_macro".to_string())
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
    fn truncates_cargo_diagnostics() {
        let summary = Summary {
            diagnostics: vec![
                Diagnostic {
                    severity: "error".to_string(),
                    location: Some("src/lib.rs:1:1".to_string()),
                    code: None,
                    message: "first diagnostic message with many characters".to_string(),
                    details: vec!["detail 1".to_string(), "detail 2".to_string()],
                },
                Diagnostic {
                    severity: "error".to_string(),
                    location: Some("src/lib.rs:2:1".to_string()),
                    code: None,
                    message: "second".to_string(),
                    details: Vec::new(),
                },
            ],
            test_failures: Vec::new(),
            text_lines: Vec::new(),
        };
        let rendered = summary.render_with_limits(
            Path::new("target/check-logs/clippy.log"),
            &SummaryLimits {
                max_diagnostics: 1,
                max_failures: 20,
                max_lines: 1,
                max_line_chars: 50,
                max_fallback_lines: 80,
            },
        );
        assert!(rendered.contains("... truncated"));
        assert!(rendered.contains("omitted 1 diagnostic detail"));
        assert!(rendered.contains("omitted 1 diagnostics"));
        assert!(rendered.contains("full log: target/check-logs/clippy.log"));
    }

    #[test]
    fn summarizes_nextest_json_failures() {
        let summary = summarize(Mode::Nextest, fixture("nextest_json_failure"));
        assert_eq!(summary.test_failures.len(), 1);
        assert_eq!(
            summary.test_failures[0].name,
            "sample::tests::panics_with_output"
        );
        assert_eq!(summary.test_failures[0].stdout, vec!["about to panic"]);
        assert_eq!(
            summary.test_failures[0].stderr,
            vec![
                "thread 'panics_with_output' panicked at src/lib.rs:42:9:",
                "boom"
            ]
        );
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
    fn truncates_nextest_failures() {
        let summary = Summary {
            diagnostics: Vec::new(),
            test_failures: vec![
                TestFailure {
                    name: "crate::first".to_string(),
                    stdout: vec!["stdout 1".to_string(), "stdout 2".to_string()],
                    stderr: vec!["stderr 1".to_string(), "stderr 2".to_string()],
                },
                TestFailure {
                    name: "crate::second".to_string(),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
            ],
            text_lines: Vec::new(),
        };
        let rendered = summary.render_with_limits(
            Path::new("target/check-logs/test.log"),
            &SummaryLimits {
                max_diagnostics: 20,
                max_failures: 1,
                max_lines: 1,
                max_line_chars: 240,
                max_fallback_lines: 80,
            },
        );
        assert!(rendered.contains("omitted 1 stdout lines"));
        assert!(rendered.contains("omitted 1 stderr lines"));
        assert!(rendered.contains("omitted 1 failed tests"));
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
        let summary = summarize(Mode::Nextest, fixture("nextest_human_failure"));
        assert!(summary.text_lines.iter().any(|line| line.contains("FAIL")));
        assert!(
            summary
                .text_lines
                .iter()
                .any(|line| line == "  about to panic")
        );
        assert!(summary.text_lines.iter().any(|line| line.contains("boom")));
        assert!(
            summary
                .text_lines
                .iter()
                .any(|line| line.starts_with("Summary"))
        );
    }

    #[test]
    fn summarizes_rustfmt_diffs() {
        let summary = summarize(Mode::Rustfmt, fixture("rustfmt_diff"));
        assert_eq!(
            summary.text_lines,
            vec![
                "Diff in /workspace/src/lib.rs:1:",
                "-fn main(){println!(\"hi\");}",
                "+fn main() {",
                "+    println!(\"hi\");",
                "+}",
            ]
        );
    }

    #[test]
    fn summarizes_cargo_deny_findings() {
        let summary = summarize(Mode::CargoDeny, fixture("cargo_deny_human"));
        assert_eq!(
            summary.text_lines,
            vec![
                "advisories ok",
                "bans FAILED",
                "error[duplicate]: found 2 duplicate entries for crate `syn`",
                "    ├ syn v1.0.109",
                "    └ syn v2.0.48",
            ]
        );
    }

    #[test]
    fn summarizes_cargo_deny_json_diagnostics() {
        let summary = summarize(Mode::CargoDeny, fixture("cargo_deny_json"));
        assert_eq!(
            summary.text_lines,
            vec![
                "error[bans]: found duplicate versions for crate",
                "  20:1: crate `syn` v1.0.109",
                "  24:1: crate `syn` v2.0.48",
                "  note: multiple versions increase build time",
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
            vec!["  anyhow"]
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
    fn truncates_fallback_text() {
        let summary = summarize(Mode::Auto, fixture("multiple_failures"));
        let rendered = summary.render_with_limits(
            Path::new("target/check-logs/check.log"),
            &SummaryLimits {
                max_diagnostics: 20,
                max_failures: 20,
                max_lines: 12,
                max_line_chars: 240,
                max_fallback_lines: 1,
            },
        );
        assert!(rendered.contains("error: first failure"));
        assert!(rendered.contains("omitted 2 fallback lines"));
    }

    #[test]
    fn supports_non_utf8_input() {
        let summary = summarize(Mode::Auto, b"error: before\n\xff\nwarning: after\n");
        assert_eq!(
            summary.text_lines,
            vec!["error: before".to_string(), "warning: after".to_string()]
        );
    }

    #[test]
    fn supports_empty_failing_output() {
        let summary = summarize(Mode::Auto, b"");
        assert!(summary.is_empty());
    }

    #[test]
    fn documents_ansi_colored_fallback_limit() {
        let summary = summarize(Mode::Auto, fixture("ansi_colored_output"));
        assert!(summary.is_empty());
    }

    #[test]
    fn summarizes_cargo_machete_findings() {
        let summary = summarize(Mode::CargoMachete, fixture("cargo_machete_unused"));
        assert_eq!(
            summary.text_lines,
            vec![
                "cargo-machete found the following unused dependencies in this directory:",
                "\tanyhow",
                "\tserde",
            ]
        );
    }
}
