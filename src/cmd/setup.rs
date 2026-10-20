use std::io::IsTerminal;

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Password, Select};

use crate::integrations::{detect, engines, systemd, telegram};
use crate::models::{Paths, State, Tools};
use crate::store::write_secret;
use crate::util::report;

pub fn run() -> Result<()> {
    run_setup().context(
        "Setup paused. Resolve the issue below and rerun raz setup; saved steps will be reused",
    )
}

fn run_setup() -> Result<()> {
    println!("raz setup — progress is saved; rerun this command to resume.\n");
    println!("[1/5] Check this machine");
    if crate::util::is_root() {
        bail!("use a dedicated non-root Linux user for both raz setup and raz serve");
    }
    if !std::io::stdin().is_terminal() {
        bail!("setup needs an interactive terminal; reconnect with SSH and run raz setup");
    }
    let tools = detect::probe();
    print_tools(&tools);
    if tools.git.is_none() {
        bail!(
            "Git is required. Install it, then rerun setup: {}",
            detect::install_hint("git")
        );
    }
    if tools.claude.is_none() && tools.codex.is_none() {
        bail!(
            "install an agent, then rerun setup:\n  Claude: {}\n  Codex: {}",
            detect::install_hint("claude"),
            detect::install_hint("codex")
        );
    }
    let paths = Paths::resolve()?;
    paths
        .ensure()
        .context("cannot create raz directories; check RAZ_HOME and RAZ_WORKSPACE permissions")?;
    let mut state = State::load_or_init(&paths)?;
    let workspace = std::path::Path::new(&state.workspace_root);
    std::fs::create_dir_all(workspace)
        .context("cannot create saved workspace; check its path and permissions")?;
    if !crate::util::owns_path(workspace) {
        bail!(
            "workspace {} must be owned by the current user; fix ownership before resuming",
            workspace.display()
        );
    }
    let probe = workspace.join(format!(".raz-setup-{}", crate::models::new_pairing_code()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .context("workspace is not writable; grant the current user write access")?;
    std::fs::remove_file(probe).context("cannot remove workspace write probe")?;
    state.save(&paths.state)?;

    println!("\n[2/5] Connect an agent");
    engines::refresh_engines(&mut state);
    state.save(&paths.state)?;
    if state.any_engine_ready() {
        if state.engines.claude.ready {
            println!("Claude: using existing login");
        }
        if state.engines.codex.ready {
            println!("Codex: using existing login");
        }
        println!("Add another agent later with raz auth claude or raz auth codex.");
    } else {
        let pick = select_engines(&tools)?;
        if pick.claude {
            engines::login_claude(&mut state)
                .context("Claude login failed; retry with raz auth claude")?;
            state.save(&paths.state)?;
            println!(
                "If Claude asks to trust a project folder, open claude there once in a terminal."
            );
        }
        if pick.codex {
            engines::login_codex(&mut state)
                .context("Codex login failed; retry with raz auth codex")?;
            state.save(&paths.state)?;
        }
    }

    println!("\n[3/5] Connect GitHub (optional)");
    engines::refresh_github(&mut state)?;
    state.save(&paths.state)?;
    if state.github.ready {
        println!("GitHub: using existing login");
    } else if state.setup.github_skipped {
        println!("Skipped previously. Connect later with raz auth github to ship PRs.");
    } else if tools.gh.is_none() {
        println!(
            "To ship PRs, install GitHub CLI and run raz auth github:\n  {}",
            detect::install_hint("gh")
        );
    } else if Confirm::new()
        .with_prompt("Connect GitHub to clone private repos and ship PRs?")
        .default(true)
        .interact()?
    {
        engines::login_github(&mut state)
            .context("GitHub login failed; retry with raz auth github")?;
        state.save(&paths.state)?;
    } else {
        state.setup.github_skipped = true;
        state.save(&paths.state)?;
        println!("Skipped. Connect later with raz auth github.");
    }

    println!("\n[4/5] Connect Telegram");
    if state.telegram.paired && paths.telegram_token()?.is_some() {
        println!("Already paired. Use raz auth telegram to change the bot or pairing.");
    } else {
        setup_telegram(&paths, &mut state, true)
            .context("Telegram setup failed; check your token and connection")?;
    }

    println!("\n[5/5] Background service");
    if !cfg!(target_os = "linux") {
        println!("Service setup requires Linux. Run raz serve in the foreground here.");
    } else if state.setup.service_configured && systemd::unit_path()?.is_file() {
        println!("Service unit already written. Start it with systemctl --user enable --now raz.");
    } else if state.setup.service_skipped {
        println!("Skipped previously. Run raz up later to write the service unit.");
    } else if Confirm::new()
        .with_prompt("Write a systemd user service for raz serve?")
        .default(true)
        .interact()?
    {
        let unit = systemd::write_user_unit(&paths)
            .context("cannot write service unit; check ~/.config/systemd/user permissions")?;
        state.setup.service_configured = true;
        state.save(&paths.state)?;
        println!(
            "Unit: {}\nStart it with:\n  systemctl --user daemon-reload\n  systemctl --user enable --now raz",
            unit.display()
        );
    } else {
        state.setup.service_skipped = true;
        state.save(&paths.state)?;
    }
    if state.telegram.paired {
        println!("\nSetup saved. Next: raz doctor, then start the service or run raz serve.");
    } else {
        println!("\nSetup saved; Telegram pairing is pending. Rerun raz setup to finish pairing.");
    }
    Ok(())
}
fn print_tools(tools: &Tools) {
    println!("binaries on PATH:");
    for name in ["claude", "codex", "gh", "git"] {
        if tools.has(name) {
            println!("  {name:8} ok");
        } else {
            println!("  {name:8} missing");
            println!("           {}", detect::install_hint(name));
        }
    }
    println!();
}

struct EnginePick {
    claude: bool,
    codex: bool,
}

fn select_engines(tools: &Tools) -> Result<EnginePick> {
    let mut items = Vec::new();
    if tools.claude.is_some() {
        items.push("claude");
    }
    if tools.codex.is_some() {
        items.push("codex");
    }
    if tools.claude.is_some() && tools.codex.is_some() {
        items.push("both");
    }
    if items.is_empty() {
        bail!(
            "neither claude nor codex is on PATH.\n  claude: {}\n  codex:  {}",
            detect::install_hint("claude"),
            detect::install_hint("codex")
        );
    }
    if items.len() == 1 {
        println!("Connecting {} (the installed agent).", items[0]);
        return Ok(EnginePick {
            claude: tools.claude.is_some(),
            codex: tools.codex.is_some(),
        });
    }
    let idx = Select::new()
        .with_prompt("which engines to authenticate")
        .items(&items)
        .default(0)
        .interact()?;
    Ok(match items[idx] {
        "claude" => EnginePick {
            claude: true,
            codex: false,
        },
        "codex" => EnginePick {
            claude: false,
            codex: true,
        },
        _ => EnginePick {
            claude: true,
            codex: true,
        },
    })
}

fn setup_telegram(paths: &Paths, state: &mut State, resume: bool) -> Result<()> {
    if let Some(existing) = paths.telegram_token()? {
        println!("telegram token already present ({})", mask_token(&existing));
        if resume
            || !Confirm::new()
                .with_prompt("replace the telegram bot token?")
                .default(false)
                .interact()?
        {
            return finish_pairing(paths, state, &existing);
        }
    }

    let token: String = if let Some(env) = telegram_token_env() {
        if Confirm::new()
            .with_prompt("use RAZ_TELEGRAM_TOKEN from the environment?")
            .default(true)
            .interact()?
        {
            env
        } else {
            prompt_token()?
        }
    } else {
        prompt_token()?
    };

    write_secret(&paths.token, token.trim())?;
    report::info("setup", format!("wrote {}", paths.token.display()));
    finish_pairing(paths, state, token.trim())
}

fn telegram_token_env() -> Option<String> {
    for key in ["RAZ_TELEGRAM_TOKEN", "POCKET_CODE_TELEGRAM_TOKEN"] {
        if let Ok(env) = std::env::var(key) {
            let trimmed = env.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn prompt_token() -> Result<String> {
    let token: String = Password::new()
        .with_prompt("Telegram bot token from BotFather")
        .interact()?;
    if token.trim().is_empty() {
        bail!("telegram token is empty");
    }
    Ok(token)
}

fn finish_pairing(paths: &Paths, state: &mut State, token: &str) -> Result<()> {
    if state.telegram.paired {
        println!("already paired to {:?}", state.telegram.allowed_user_ids);
        if !Confirm::new()
            .with_prompt("pair again with a new code?")
            .default(false)
            .interact()?
        {
            return Ok(());
        }
        state.telegram.paired = false;
        state.telegram.allowed_user_ids.clear();
    }

    let code = if state.pairing_is_live() {
        state
            .telegram
            .pairing_code
            .clone()
            .expect("live pairing has a code")
    } else {
        let code = crate::models::new_pairing_code();
        state.set_pairing_code(code.clone());
        state.save(&paths.state)?;
        code
    };

    println!("\nmessage your bot:\n");
    println!("  /start {code}\n");
    println!(
        "Codes expire 20 minutes after creation. Rerun raz setup to resume or renew an expired code.\n"
    );

    let wait = if std::io::stdin().is_terminal() {
        Confirm::new()
            .with_prompt("wait here for that /start now?")
            .default(true)
            .interact()?
    } else {
        false
    };

    if wait {
        telegram::wait_for_pair(state, token, |s| s.save(&paths.state))?;
        println!("telegram paired.");
    } else {
        println!("run raz serve and send /start {code}");
    }
    Ok(())
}

fn mask_token(token: &str) -> String {
    let t = token.trim();
    if t.len() < 8 {
        return "(set)".into();
    }
    format!("{}…{}", &t[..4], &t[t.len() - 4..])
}

pub fn auth_telegram() -> Result<()> {
    let paths = Paths::resolve()?;
    paths.ensure()?;
    let mut state = State::load_or_init(&paths)?;
    setup_telegram(&paths, &mut state, false)
}
