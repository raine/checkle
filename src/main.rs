use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use checkle::{Mode, RunOptions, SuiteOptions, SummaryLimits};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Run checks with compact, agent-friendly failure output"
)]
struct Cli {
    #[command(subcommand)]
    action: Option<Action>,

    #[arg(long, default_value = "check")]
    label: String,

    #[arg(long, value_enum, default_value_t = Mode::default())]
    mode: Mode,

    #[arg(long, default_value = "target/check-logs", global = true)]
    log_dir: String,

    #[arg(long, default_value_t = 20)]
    max_diagnostics: usize,

    #[arg(long, default_value_t = 20)]
    max_failures: usize,

    #[arg(long, default_value_t = 12)]
    max_lines: usize,

    #[arg(long, default_value_t = 240)]
    max_line_chars: usize,

    #[arg(long, default_value_t = 80)]
    tail: usize,

    #[arg(trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Action {
    Run {
        #[arg(value_name = "CHECK")]
        checks: Vec<String>,
    },
    PreCommit {
        #[arg(value_name = "CHECK")]
        checks: Vec<String>,
    },
    FormatStaged,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let limits = SummaryLimits {
        max_diagnostics: cli.max_diagnostics,
        max_failures: cli.max_failures,
        max_lines: cli.max_lines,
        max_line_chars: cli.max_line_chars,
        max_fallback_lines: cli.tail,
    };
    let code = match cli.action {
        Some(Action::Run { checks }) => checkle::run_suite(SuiteOptions {
            checks,
            log_dir: cli.log_dir,
            limits,
        })?,
        Some(Action::PreCommit { checks }) => checkle::run_pre_commit(checkle::PreCommitOptions {
            checks,
            log_dir: cli.log_dir,
            limits,
        })?,
        Some(Action::FormatStaged) => checkle::format_staged_from_git_root()?,
        None => {
            if cli.command.is_empty() {
                Cli::command().print_help()?;
                eprintln!();
                2
            } else {
                checkle::run(RunOptions {
                    label: cli.label,
                    mode: cli.mode,
                    log_dir: cli.log_dir,
                    limits,
                    command: cli.command,
                })?
            }
        }
    };
    std::process::exit(code);
}
