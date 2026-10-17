use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;

use crate::models::{State, TelegramUpdate};
use crate::util::report;

const API: &str = "https://api.telegram.org";

#[derive(Debug, Deserialize)]
struct ApiResult<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(40))
        .build()
}

fn call<T: serde::de::DeserializeOwned>(
    token: &str,
    method: &str,
    body: serde_json::Value,
) -> Result<T> {
    let url = format!("{API}/bot{token}/{method}");
    let response = agent()
        .post(&url)
        .send_json(body)
        .context("telegram request")?;
    let parsed: ApiResult<T> = response.into_json().context("telegram json")?;
    if !parsed.ok {
        bail!(
            "telegram {method} failed: {}",
            parsed.description.unwrap_or_else(|| "unknown error".into())
        );
    }
    parsed
        .result
        .ok_or_else(|| anyhow::anyhow!("telegram {method} returned no result"))
}

pub fn get_updates(token: &str, offset: i64, timeout: u64) -> Result<Vec<TelegramUpdate>> {
    call(
        token,
        "getUpdates",
        json!({
            "offset": offset,
            "timeout": timeout,
            "allowed_updates": ["message"],
        }),
    )
}

#[derive(Debug, Deserialize)]
struct SentMessage {
    message_id: i64,
}

fn outbound_text(text: &str) -> String {
    let clipped = crate::util::clip_telegram(text);
    if clipped.trim().is_empty() {
        "(empty)".into()
    } else {
        clipped
    }
}

/// Slash commands shown in Telegram's Menu / `/` picker.
pub const BOT_COMMANDS: &[(&str, &str)] = &[
    ("help", "Show available commands"),
    ("commands", "List all slash commands"),
    ("status", "Engine, project, git, sessions, jobs"),
    ("project", "List projects, or switch with a name"),
    ("add", "Register workspace/<name>"),
    ("clone", "Clone owner/repo into the workspace"),
    ("cwd", "Set cwd inside the project"),
    ("use", "Switch engine: claude or codex"),
    ("new", "New session for this engine, or a job id"),
    ("stop", "Stop the default job, or a job id"),
    ("perm", "Set sandbox: read or write"),
    ("jobs", "Running and recent jobs"),
    ("run", "Start a job without flipping /use"),
    ("both", "Send the same prompt to Claude and Codex"),
    ("verbose", "Noisier status for this turn"),
    ("pr", "Last pull request URL"),
    ("github", "GitHub login from Telegram"),
];

pub fn install_menu(token: &str) -> Result<()> {
    let commands: Vec<serde_json::Value> = BOT_COMMANDS
        .iter()
        .map(|(command, description)| {
            json!({
                "command": command,
                "description": description,
            })
        })
        .collect();
    let _: bool = call(
        token,
        "setMyCommands",
        json!({ "commands": commands }),
    )?;
    let _: bool = call(
        token,
        "setChatMenuButton",
        json!({ "menu_button": { "type": "commands" } }),
    )?;
    report::info("telegram", "installed command menu");
    Ok(())
}

pub fn send_message(token: &str, chat_id: i64, text: &str) -> Result<i64> {
    send_json(token, chat_id, text, None)
}

pub fn send_message_reply(
    token: &str,
    chat_id: i64,
    text: &str,
    reply_to: i64,
) -> Result<i64> {
    send_json(token, chat_id, text, Some(reply_to))
}

fn send_json(
    token: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<i64> {
    let mut body = json!({
        "chat_id": chat_id,
        "text": outbound_text(text),
    });
    if let Some(id) = reply_to {
        body["reply_to_message_id"] = json!(id);
    }
    let sent: SentMessage = call(token, "sendMessage", body)?;
    Ok(sent.message_id)
}

pub fn edit_message(token: &str, chat_id: i64, message_id: i64, text: &str) -> Result<()> {
    let result = call::<serde_json::Value>(
        token,
        "editMessageText",
        json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": outbound_text(text),
        }),
    );
    match result {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("message is not modified") => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn drain(token: &str, state: &mut State) -> Result<()> {
    let updates = get_updates(token, state.telegram.last_update_id + 1, 0)?;
    if let Some(last) = updates.last() {
        state.telegram.last_update_id = last.update_id;
    }
    Ok(())
}

pub fn start_payload(text: &str) -> Option<&str> {
    let mut parts = text.trim().split_whitespace();
    let cmd = parts.next()?;
    if cmd != "/start" && !cmd.starts_with("/start@") {
        return None;
    }
    let code = parts.next()?.trim();
    if code.is_empty() {
        None
    } else {
        Some(code)
    }
}

pub fn try_pair(state: &mut State, token: &str, update: &TelegramUpdate) -> Result<bool> {
    let Some(message) = update.message.as_ref() else {
        return Ok(false);
    };
    let Some(from) = message.from.as_ref() else {
        return Ok(false);
    };
    let Some(text) = message.text.as_deref() else {
        return Ok(false);
    };
    let Some(code) = start_payload(text) else {
        return Ok(false);
    };
    if !state.pairing_is_live() {
        send_message(
            token,
            message.chat.id,
            "pairing code is missing or expired. run raz auth telegram",
        )?;
        return Ok(false);
    }
    let expected = state.telegram.pairing_code.as_deref().unwrap_or_default();
    if !code.eq_ignore_ascii_case(expected) {
        send_message(token, message.chat.id, "wrong pairing code")?;
        return Ok(false);
    }

    state.consume_pairing(from.id);
    if let Err(err) = install_menu(token) {
        report::warn("telegram", format!("command menu: {err:#}"));
    }
    let who = from.username.as_deref().unwrap_or("user");
    send_message(
        token,
        message.chat.id,
        &format!(
            "paired as {who} ({id}). you are the only user this bot will answer.",
            id = from.id
        ),
    )?;
    report::info("telegram", format!("paired telegram user {}", from.id));
    Ok(true)
}

pub fn wait_for_pair(
    state: &mut State,
    token: &str,
    persist: impl Fn(&State) -> Result<()>,
) -> Result<()> {
    drain(token, state)?;
    persist(state)?;
    report::info("telegram", "waiting for /start <code> from Telegram");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20 * 60);
    while std::time::Instant::now() < deadline {
        if !state.pairing_is_live() {
            bail!("pairing code expired");
        }
        let updates = match get_updates(token, state.telegram.last_update_id + 1, 25) {
            Ok(u) => u,
            Err(err) => {
                report::warn("telegram", err.to_string());
                continue;
            }
        };
        for update in updates {
            state.telegram.last_update_id = update.update_id;
            if try_pair(state, token, &update)? {
                persist(state)?;
                return Ok(());
            }
        }
        persist(state)?;
    }
    bail!("timed out waiting for Telegram /start")
}

#[cfg(test)]
mod tests {
    use super::{start_payload, BOT_COMMANDS};

    #[test]
    fn parse_start() {
        assert_eq!(start_payload("/start XK4P2M"), Some("XK4P2M"));
        assert_eq!(start_payload("/start@mybot XK4P2M"), Some("XK4P2M"));
        assert_eq!(start_payload("/start"), None);
        assert_eq!(start_payload("/status"), None);
    }

    #[test]
    fn menu_commands_fit_telegram() {
        let mut names = std::collections::BTreeSet::new();
        for (command, description) in BOT_COMMANDS {
            assert!(
                command
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{command}"
            );
            assert!((1..=32).contains(&command.len()), "{command}");
            assert!(
                (3..=256).contains(&description.len()),
                "{command}: {description}"
            );
            assert!(names.insert(*command), "duplicate {command}");
        }
        assert!(names.contains("help"));
        assert!(names.contains("status"));
        assert!(names.contains("jobs"));
    }
}
