use anyhow::Result;
use clap::{Parser, ValueEnum};

use checkle::{Mode, RunOptions};

#[derive(Debug, Parser)]
#[command(about = "Run checks with compact, agent-friendly failure output")]
struct Cli {
    #[arg(long, default_value = "check")]
    label: String,

    #[arg(long, value_enum, default_value_t = CliMode::Auto)]
    mode: CliMode,

    #[arg(long, default_value = "target/check-logs")]
    log_dir: String,

    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliMode {
    Auto,
    Cargo,
    Nextest,
    Rustfmt,
    CargoDeny,
    CargoMachete,
}

impl From<CliMode> for Mode {
    fn from(value: CliMode) -> Self {
        match value {
            CliMode::Auto => Mode::Auto,
            CliMode::Cargo => Mode::Cargo,
            CliMode::Nextest => Mode::Nextest,
            CliMode::Rustfmt => Mode::Rustfmt,
            CliMode::CargoDeny => Mode::CargoDeny,
            CliMode::CargoMachete => Mode::CargoMachete,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let code = checkle::run(RunOptions {
        label: cli.label,
        mode: cli.mode.into(),
        log_dir: cli.log_dir,
        command: cli.command,
    })?;
    std::process::exit(code);
}
