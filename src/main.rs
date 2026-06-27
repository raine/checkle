use anyhow::Result;
use clap::Parser;

use checkle::{Mode, RunOptions, SummaryLimits};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Run checks with compact, agent-friendly failure output"
)]
struct Cli {
    #[arg(long, default_value = "check")]
    label: String,

    #[arg(long, value_enum, default_value_t = Mode::default())]
    mode: Mode,

    #[arg(long, default_value = "target/check-logs")]
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

    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let code = checkle::run(RunOptions {
        label: cli.label,
        mode: cli.mode,
        log_dir: cli.log_dir,
        limits: SummaryLimits {
            max_diagnostics: cli.max_diagnostics,
            max_failures: cli.max_failures,
            max_lines: cli.max_lines,
            max_line_chars: cli.max_line_chars,
            max_fallback_lines: cli.tail,
        },
        command: cli.command,
    })?;
    std::process::exit(code);
}
