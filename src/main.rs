mod config;
mod env;
mod executor;
mod history;
mod parser;
mod plugin;
mod prompt;
mod repl;
mod scripting;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// zish — A blazing-fast, fully customizable Linux shell
#[derive(Parser, Debug)]
#[command(name = "zish", version, about)]
struct Args {
    /// Run a command string and exit
    #[arg(short = 'c', long)]
    command: Option<String>,

    /// Script file to execute
    script: Option<PathBuf>,

    /// Arguments passed to the script
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,

    /// Start in login shell mode
    #[arg(short = 'l', long)]
    login: bool,

    /// Start in interactive mode (default when no script/command)
    #[arg(short = 'i', long)]
    interactive: bool,

    /// Don't read startup files
    #[arg(long)]
    no_rc: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize config
    let config = config::Config::load()?;

    // Initialize environment
    env::init();

    // Decide execution mode
    if let Some(cmd) = args.command {
        // -c mode: run a single command string
        let mut exec = executor::Executor::new(config);
        exec.run_string(&cmd)?;
    } else if let Some(script) = args.script {
        // Script mode
        let mut exec = executor::Executor::new(config);
        exec.run_file(&script, &args.args)?;
    } else {
        // Interactive REPL mode
        let shell = repl::Shell::new(config)?;
        shell.run()?;
    }

    Ok(())
}
