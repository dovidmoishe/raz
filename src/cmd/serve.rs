use anyhow::{bail, Result};

use crate::integrations::{detect, engines, telegram};
use crate::models::{Paths, State, TelegramUpdate};
use crate::util::report;

pub fn run() -> Result<()> {
    let paths = Paths::resolve()?;
    if !paths.state.exists() {
        bail!("no state.json — run pocket-code setup");
    }
    let mut state = State::load(&paths.state)?;
    let token = paths
        .telegram_token()?
        .ok_or_else(|| anyhow::anyhow!("no telegram token — run pocket-code setup"))?;

    if !state.any_engine_ready() {
        engines::refresh_engines(&mut state);
        state.save(&paths.state)?;
    }
    if !state.any_engine_ready() {
        bail!("no engine is authenticated — run pocket-code auth");
    }

    report::info("serve", format!("home {}", paths.home.display()));
    report::info(
        "serve",
        format!(
            "engines claude={} codex={} paired={}",
            state.engines.claude.ready, state.engines.codex.ready, state.telegram.paired
        ),
    );
    if state.pairing_is_live() {
        if let Some(code) = state.telegram.pairing_code.as_deref() {
            report::info("serve", format!("waiting for /start {code}"));
        }
    }

    loop {
        let updates = match telegram::get_updates(&token, state.telegram.last_update_id + 1, 25) {
            Ok(u) => u,
            Err(err) => {
                report::warn("serve", err.to_string());
                continue;
            }
        };
        for update in updates {
            state.telegram.last_update_id = update.update_id;
            handle(&paths, &mut state, &token, &update)?;
        }
        state.save(&paths.state)?;
    }
}

fn handle(
    paths: &Paths,
    state: &mut State,
    token: &str,
    update: &TelegramUpdate,
) -> Result<()> {
    let Some(message) = update.message.as_ref() else {
        return Ok(());
    };
    let Some(from) = message.from.as_ref() else {
        return Ok(());
    };
    let text = message.text.as_deref().unwrap_or("").trim();
    if text.is_empty() {
        return Ok(());
    }

    if !state.is_allowed(from.id) {
        if telegram::try_pair(state, token, update)? {
            state.save(&paths.state)?;
        }
        return Ok(());
    }

    let cmd = text.split_whitespace().next().unwrap_or("");
    match cmd {
        "/status" | "/start" => {
            telegram::send_message(token, message.chat.id, &status_text(state))?;
        }
        "/help" => {
            telegram::send_message(
                token,
                message.chat.id,
                "pocket-code is paired.\n\
                 /status — engines, github, current project\n\
                 project and /use commands come next.",
            )?;
        }
        _ => {
            telegram::send_message(
                token,
                message.chat.id,
                "setup is done. project and engine commands are not wired yet.\ntry /status",
            )?;
        }
    }
    Ok(())
}

fn status_text(state: &State) -> String {
    let tools = detect::probe();
    let engine = state
        .current
        .engine
        .as_ref()
        .map(|e| e.as_str())
        .unwrap_or("none");
    format!(
        "pocket-code\n\
         engine: {engine}\n\
         claude: {c}\n\
         codex: {x}\n\
         github: {g}\n\
         project: {p}\n\
         workspace: {w}\n\
         binaries: claude={cb} codex={xb} gh={gh} git={git}",
        c = ready(state.engines.claude.ready),
        x = ready(state.engines.codex.ready),
        g = state
            .github
            .user
            .as_deref()
            .map(|u| format!("ok @{u}"))
            .unwrap_or_else(|| ready(state.github.ready).into()),
        p = state.current.project.as_deref().unwrap_or("none"),
        w = state.workspace_root,
        cb = tools.claude.is_some(),
        xb = tools.codex.is_some(),
        gh = tools.gh.is_some(),
        git = tools.git.is_some(),
    )
}

fn ready(ok: bool) -> &'static str {
    if ok {
        "ready"
    } else {
        "no"
    }
}
