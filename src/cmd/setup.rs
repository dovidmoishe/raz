use std::io::IsTerminal;

use anyhow::{bail, Result};
use dialoguer::{Confirm, Password, Select};

use crate::integrations::{detect, engines, systemd, telegram};
use crate::models::{Paths, State, Tools};
use crate::store::write_secret;
use crate::util::report;

pub fn run() -> Result<()> {
    println!("raz setup");
    println!(
        "Claude and Codex keep their own logins. We only store ready flags and session ids.\n"
    );

    let paths = Paths::resolve()?;
    paths.ensure()?;
    report::info("setup", format!("home {}", paths.home.display()));

    let mut state = State::load_or_init(&paths)?;
    state.workspace_root = paths.workspace.to_string_lossy().into_owned();
    state.save(&paths.state)?;

    let tools = detect::probe();
    print_tools(&tools);

    let engines_pick = select_engines(&tools)?;
    if engines_pick.claude {
        engines::login_claude(&mut state)?;
        state.save(&paths.state)?;
        println!(
            "\nIf a later `claude -p` run asks to trust a folder, do that once in a TTY.\n\
             Headless serve cannot answer that prompt.\n"
        );
    }
    if engines_pick.codex {
        engines::login_codex(&mut state)?;
        state.save(&paths.state)?;
    }
    if !state.any_engine_ready() {
        bail!("need at least one of Claude or Codex authenticated");
    }

    if tools.gh.is_some()
        && Confirm::new()
            .with_prompt("authenticate GitHub now (gh auth login)?")
            .default(true)
            .interact()?
    {
        engines::login_github(&mut state)?;
        state.save(&paths.state)?;
    } else if tools.gh.is_none() {
        println!(
            "gh is missing. install it later, then run: raz auth github\n  {}\n",
            detect::install_hint("gh")
        );
    }

    setup_telegram(&paths, &mut state)?;

    if cfg!(unix)
        && Confirm::new()
            .with_prompt("write a systemd --user unit for raz serve?")
            .default(true)
            .interact()?
    {
        let unit = systemd::write_user_unit(&paths)?;
        println!(
            "\nenable it with:\n  systemctl --user daemon-reload\n  systemctl --user enable --now raz\n\nunit: {}\n",
            unit.display()
        );
    }

    println!("setup complete. next: raz doctor   then   raz serve");
    println!("then open build-board.html locally and check off the setup tasks.");
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

fn setup_telegram(paths: &Paths, state: &mut State) -> Result<()> {
    if let Some(existing) = paths.telegram_token()? {
        println!("telegram token already present ({})", mask_token(&existing));
        if !Confirm::new()
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

    let code = crate::models::new_pairing_code();
    state.set_pairing_code(code.clone());
    state.save(&paths.state)?;

    println!("\nmessage your bot:\n");
    println!("  /start {code}\n");
    println!("code expires in 20 minutes.\n");

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
    setup_telegram(&paths, &mut state)
}
