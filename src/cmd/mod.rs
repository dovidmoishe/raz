mod doctor;
mod serve;
mod setup;
mod up;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::models::{Paths, State};
use crate::util::report;

#[derive(Parser)]
#[command(
    name = "raz",
    about = "raz: harness for Claude Code and Codex, driven from Telegram.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// First-run wizard: engines, GitHub, Telegram pair, optional systemd unit.
    Setup,
    /// Re-run one login without the full wizard.
    Auth {
        #[command(subcommand)]
        target: AuthTarget,
    },
    /// Check binaries, logins, state file, and Telegram pairing.
    Doctor,
    /// Long-poll Telegram. Completes pairing and answers /status.
    Serve,
    /// Write a systemd --user unit, or print how to run serve.
    Up,
}

#[derive(Subcommand)]
enum AuthTarget {
    Claude,
    Codex,
    Github,
    Telegram,
}

pub fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Setup => {
            setup::run()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Auth { target } => {
            auth_cmd(target)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor => doctor::run(),
        Command::Serve => {
            serve::run()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Up => {
            up::run()?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn auth_cmd(target: AuthTarget) -> Result<()> {
    let paths = Paths::resolve()?;
    paths.ensure()?;
    let mut state = State::load_or_init(&paths)?;
    match target {
        AuthTarget::Claude => crate::integrations::engines::login_claude(&mut state)?,
        AuthTarget::Codex => crate::integrations::engines::login_codex(&mut state)?,
        AuthTarget::Github => crate::integrations::engines::login_github(&mut state)?,
        AuthTarget::Telegram => {
            setup::auth_telegram()?;
            return Ok(());
        }
    }
    if state.current.engine.is_none() {
        state.current.engine = state.pick_default_engine();
    }
    state.save(&paths.state)?;
    Ok(())
}

pub fn abort(err: anyhow::Error) -> ExitCode {
    report::error("cli", format!("{err:#}"));
    ExitCode::from(1)
}
