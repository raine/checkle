use std::collections::HashSet;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    Mode, RunOptions, RunResult, SummaryLimits, execute_check, render_report, summarize_for_command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub label: String,
    pub mode: Mode,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    check: Vec<ConfigCheck>,
    #[serde(default)]
    group: Vec<ConfigGroup>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigCheck {
    name: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    mode: Mode,
    command: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigGroup {
    name: String,
    checks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SuiteOptions {
    pub checks: Vec<String>,
    pub log_dir: String,
    pub limits: SummaryLimits,
}

#[derive(Debug, Clone)]
pub struct SuiteStatus {
    pub label: String,
    pub mode: Mode,
    pub command: Vec<String>,
    pub elapsed: Duration,
    pub result: Result<RunResult, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedCheck {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct SuiteSummary {
    pub statuses: Vec<SuiteStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChecks {
    pub specs: Vec<RunSpec>,
    pub skipped: Vec<SkippedCheck>,
}

#[derive(Debug, Clone, Copy)]
struct BuiltinCheck {
    name: &'static str,
    label: &'static str,
    mode: Mode,
    command: &'static [&'static str],
    required_tool: Option<&'static str>,
}

const BUILTIN_CHECKS: &[BuiltinCheck] = &[
    BuiltinCheck {
        name: "format-check",
        label: "format-check",
        mode: Mode::Rustfmt,
        command: &["cargo", "fmt", "--all", "--", "--check"],
        required_tool: None,
    },
    BuiltinCheck {
        name: "clippy",
        label: "clippy",
        mode: Mode::Cargo,
        command: &[
            "cargo",
            "clippy",
            "--message-format=json",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        required_tool: None,
    },
    BuiltinCheck {
        name: "test",
        label: "test",
        mode: Mode::Cargo,
        command: &["cargo", "test", "--locked", "--message-format=json"],
        required_tool: None,
    },
    BuiltinCheck {
        name: "cargo-deny",
        label: "cargo-deny",
        mode: Mode::CargoDeny,
        command: &["cargo", "deny", "--format", "json", "check"],
        required_tool: Some("cargo-deny"),
    },
    BuiltinCheck {
        name: "cargo-machete",
        label: "cargo-machete",
        mode: Mode::CargoMachete,
        command: &["cargo", "machete", "--with-metadata"],
        required_tool: Some("cargo-machete"),
    },
];

const STATIC_ANALYSIS: &[&str] = &["cargo-deny", "cargo-machete"];
const ALL_CHECKS: &[&str] = &[
    "format-check",
    "clippy",
    "test",
    "cargo-deny",
    "cargo-machete",
];

pub fn run_suite(options: SuiteOptions) -> Result<i32> {
    if options.checks.is_empty() {
        eprint!("{}", list_checks());
        return Ok(0);
    }

    let resolved = match resolve_checks(&options.checks) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("{error:#}");
            return Ok(2);
        }
    };
    let skipped = resolved.skipped.clone();
    let specs = resolved.specs;
    for skipped_check in &skipped {
        eprintln!("skip {}: {}", skipped_check.label, skipped_check.reason);
    }
    if specs.is_empty() {
        return Ok(0);
    }

    let progress = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let summary =
        execute_specs_with_progress(specs, options.log_dir, options.limits.clone(), progress)?;
    render_suite_summary(&summary, &options.limits);
    if summary.statuses.iter().any(|status| {
        status
            .result
            .as_ref()
            .map(|result| result.exit_code != 0)
            .unwrap_or(true)
    }) {
        Ok(1)
    } else {
        Ok(0)
    }
}

pub fn list_checks() -> String {
    let mut output = String::from("available checks:\n");
    for check in BUILTIN_CHECKS {
        output.push_str("  ");
        output.push_str(check.name);
        if check.required_tool.is_some() {
            output.push_str(" (optional tool)");
        }
        output.push('\n');
    }
    output.push_str("  static-analysis (group)\n");
    output.push_str("  all (group)\n");
    output
}

pub fn resolve_checks(names: &[String]) -> Result<ResolvedChecks> {
    let config = read_config()?;
    let mut specs = Vec::new();
    let mut skipped = Vec::new();
    for name in names {
        resolve_name(name, &config, &mut specs, &mut skipped, false)?;
    }
    reject_duplicate_labels(&specs)?;
    Ok(ResolvedChecks { specs, skipped })
}

fn resolve_name(
    name: &str,
    config: &Option<ConfigFile>,
    specs: &mut Vec<RunSpec>,
    skipped: &mut Vec<SkippedCheck>,
    from_group: bool,
) -> Result<()> {
    if let Some(config) = config {
        if let Some(check) = config.check.iter().find(|check| check.name == name) {
            specs.push(check.to_run_spec());
            return Ok(());
        }
        if let Some(group) = config.group.iter().find(|group| group.name == name) {
            for name in &group.checks {
                resolve_name(name, &Some(config.clone()), specs, skipped, true)?;
            }
            return Ok(());
        }
    }

    match name {
        "static-analysis" => resolve_group(STATIC_ANALYSIS, specs, skipped)?,
        "all" => resolve_group(ALL_CHECKS, specs, skipped)?,
        name => {
            let check = builtin_check(name).with_context(|| format!("unknown check: {name}"))?;
            if let Some(tool) = check.required_tool {
                if !tool_installed(tool) {
                    if from_group {
                        skipped.push(SkippedCheck {
                            label: check.label.to_string(),
                            reason: format!("{tool} not installed"),
                        });
                        return Ok(());
                    }
                    bail!("required tool for {name} is not installed: {tool}");
                }
            }
            specs.push(check.to_run_spec());
        }
    }
    Ok(())
}

fn read_config() -> Result<Option<ConfigFile>> {
    let path = std::path::Path::new("checkle.toml");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).context("read checkle.toml")?;
    let config = toml::from_str(&text).context("parse checkle.toml")?;
    Ok(Some(config))
}

impl ConfigCheck {
    fn to_run_spec(&self) -> RunSpec {
        RunSpec {
            label: self.label.clone().unwrap_or_else(|| self.name.clone()),
            mode: self.mode,
            command: self.command.clone(),
        }
    }
}

fn reject_duplicate_labels(specs: &[RunSpec]) -> Result<()> {
    let mut seen = HashSet::new();
    for spec in specs {
        if !seen.insert(spec.label.as_str()) {
            bail!("duplicate check label: {}", spec.label);
        }
    }
    Ok(())
}

fn resolve_group(
    names: &[&str],
    specs: &mut Vec<RunSpec>,
    skipped: &mut Vec<SkippedCheck>,
) -> Result<()> {
    for name in names {
        let check = builtin_check(name).with_context(|| format!("unknown check: {name}"))?;
        if let Some(tool) = check.required_tool {
            if !tool_installed(tool) {
                skipped.push(SkippedCheck {
                    label: check.label.to_string(),
                    reason: format!("{tool} not installed"),
                });
                continue;
            }
        }
        specs.push(check.to_run_spec());
    }
    Ok(())
}

fn builtin_check(name: &str) -> Option<BuiltinCheck> {
    BUILTIN_CHECKS
        .iter()
        .copied()
        .find(|check| check.name == name)
}

impl BuiltinCheck {
    fn to_run_spec(self) -> RunSpec {
        RunSpec {
            label: self.label.to_string(),
            mode: self.mode,
            command: self.command.iter().map(|part| part.to_string()).collect(),
        }
    }
}

pub fn tool_installed(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn execute_specs(
    specs: Vec<RunSpec>,
    log_dir: String,
    limits: SummaryLimits,
) -> Result<SuiteSummary> {
    execute_specs_with_progress(specs, log_dir, limits, false)
}

fn execute_specs_with_progress(
    specs: Vec<RunSpec>,
    log_dir: String,
    limits: SummaryLimits,
    progress: bool,
) -> Result<SuiteSummary> {
    let log_dir_path = std::path::PathBuf::from(&log_dir);
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for (index, spec) in specs.iter().cloned().enumerate() {
        let sender = sender.clone();
        let log_dir = log_dir.clone();
        let limits = limits.clone();
        handles.push(thread::spawn(move || {
            let started = Instant::now();
            let result = execute_check(&RunOptions {
                label: spec.label.clone(),
                mode: spec.mode,
                log_dir,
                limits,
                command: spec.command.clone(),
            })
            .map_err(|error| format!("{error:#}"));
            let _ = sender.send((index, spec, started.elapsed(), result));
        }));
    }
    drop(sender);

    let started: Vec<Instant> = specs.iter().map(|_| Instant::now()).collect();
    let mut statuses: Vec<Option<SuiteStatus>> = vec![None; specs.len()];
    let mut remaining = specs.len();
    let mut frame = 0;
    let mut drawn = false;
    if progress {
        eprint!("\x1b[?25l");
    }
    while remaining > 0 {
        match receiver.recv_timeout(Duration::from_millis(90)) {
            Ok((index, spec, elapsed, result)) => {
                statuses[index] = Some(SuiteStatus {
                    label: spec.label,
                    mode: spec.mode,
                    command: spec.command,
                    elapsed,
                    result,
                });
                remaining -= 1;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if progress {
            render_progress(&specs, &statuses, &started, &log_dir_path, frame, drawn);
            drawn = true;
            frame = (frame + 1) % FRAMES.len();
        }
    }
    if progress {
        clear_progress(specs.len());
        eprint!("\x1b[?25h");
    }

    for handle in handles {
        if handle.join().is_err() {
            bail!("suite worker thread panicked");
        }
    }

    Ok(SuiteSummary {
        statuses: statuses
            .into_iter()
            .enumerate()
            .map(|(index, status)| {
                status.unwrap_or_else(|| SuiteStatus {
                    label: specs[index].label.clone(),
                    mode: specs[index].mode,
                    command: specs[index].command.clone(),
                    elapsed: started[index].elapsed(),
                    result: Err("suite worker did not return a result".to_string()),
                })
            })
            .collect(),
    })
}

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn render_progress(
    specs: &[RunSpec],
    statuses: &[Option<SuiteStatus>],
    started: &[Instant],
    log_dir: &std::path::Path,
    frame: usize,
    drawn: bool,
) {
    if drawn {
        eprint!("\x1b[{}A", specs.len());
    }
    let name_width = specs
        .iter()
        .map(|spec| spec.label.len())
        .max()
        .unwrap_or_default();
    for (index, spec) in specs.iter().enumerate() {
        let (glyph, elapsed, detail) = match &statuses[index] {
            Some(status) => {
                let glyph = match &status.result {
                    Ok(result) if result.exit_code == 0 => "✓".to_string(),
                    _ => "✖".to_string(),
                };
                (glyph, elapsed_seconds(status.elapsed), String::new())
            }
            None => {
                let log_path = log_dir.join(format!("{}.log", spec.label));
                (
                    FRAMES[frame].to_string(),
                    elapsed_seconds(started[index].elapsed()),
                    latest_log_line(&log_path),
                )
            }
        };
        eprintln!(
            "\x1b[2K{} {:<width$}  {} {}",
            glyph,
            spec.label,
            elapsed,
            detail,
            width = name_width
        );
    }
}

fn clear_progress(lines: usize) {
    if lines > 0 {
        eprint!("\x1b[{}A", lines);
    }
    for _ in 0..lines {
        eprintln!("\x1b[2K");
    }
    if lines > 0 {
        eprint!("\x1b[{}A", lines);
    }
}

fn latest_log_line(path: &std::path::Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    text.lines()
        .rev()
        .map(strip_ansi)
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
}

fn strip_ansi(line: &str) -> String {
    let mut output = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn render_suite_summary(summary: &SuiteSummary, limits: &SummaryLimits) {
    for status in &summary.statuses {
        match &status.result {
            Ok(result) if result.exit_code == 0 => {
                eprintln!("ok {} ({})", status.label, elapsed_seconds(status.elapsed));
            }
            Ok(_) => {
                eprintln!(
                    "fail {} ({})",
                    status.label,
                    elapsed_seconds(status.elapsed)
                );
            }
            Err(_) => {
                eprintln!(
                    "error {} ({})",
                    status.label,
                    elapsed_seconds(status.elapsed)
                );
            }
        }
    }

    let failed = summary
        .statuses
        .iter()
        .filter(|status| {
            status
                .result
                .as_ref()
                .map(|result| result.exit_code != 0)
                .unwrap_or(true)
        })
        .count();

    for status in &summary.statuses {
        match &status.result {
            Ok(result) if result.exit_code != 0 => {
                let parsed = summarize_for_command(status.mode, &status.command, &result.output);
                let report = render_report(&parsed, &result.log_path, limits, &result.output);
                eprintln!("\n{}", report.trim_end());
            }
            Err(error) => {
                eprintln!("\n{}\n  {}", status.label, error);
            }
            _ => {}
        }
    }

    if failed > 0 {
        eprintln!("fail {}/{} checks failed", failed, summary.statuses.len());
    }
}

fn elapsed_seconds(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        format!("{}ms", duration.as_millis())
    } else if duration.as_secs() == 1 {
        "1s".to_string()
    } else {
        format!("{}s", duration.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn lists_builtin_checks() {
        let list = list_checks();
        assert!(list.contains("format-check"));
        assert!(list.contains("clippy"));
        assert!(list.contains("test"));
        assert!(list.contains("static-analysis"));
    }

    #[test]
    fn rejects_unknown_checks() {
        let err = resolve_checks(&["wat".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unknown check: wat"));
    }

    #[test]
    fn rejects_duplicate_resolved_labels() {
        let err = resolve_checks(&["clippy".to_string(), "clippy".to_string()]).unwrap_err();
        assert!(err.to_string().contains("duplicate check label: clippy"));
    }

    #[test]
    fn runs_specs_in_parallel_and_preserves_order() {
        let dir = tempdir().unwrap();
        let summary = execute_specs(
            vec![
                RunSpec {
                    label: "slow".to_string(),
                    mode: Mode::Auto,
                    command: vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "sleep 1; exit 0".to_string(),
                    ],
                },
                RunSpec {
                    label: "fast".to_string(),
                    mode: Mode::Auto,
                    command: vec!["sh".to_string(), "-c".to_string(), "exit 1".to_string()],
                },
            ],
            dir.path().join("logs").display().to_string(),
            SummaryLimits::default(),
        )
        .unwrap();

        assert_eq!(summary.statuses[0].label, "slow");
        assert_eq!(summary.statuses[1].label, "fast");
        assert_eq!(summary.statuses[0].result.as_ref().unwrap().exit_code, 0);
        assert_eq!(summary.statuses[1].result.as_ref().unwrap().exit_code, 1);
    }

    #[test]
    fn writes_each_spec_log() {
        let dir = tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        let _summary = execute_specs(
            vec![RunSpec {
                label: "sample".to_string(),
                mode: Mode::Auto,
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf hello".to_string(),
                ],
            }],
            log_dir.display().to_string(),
            SummaryLimits::default(),
        )
        .unwrap();

        let log = std::fs::read_to_string(log_dir.join("sample.log")).unwrap();
        assert_eq!(log, "[stdout] hello\n");
    }
}
