use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::models::{State, TelegramApiError, TelegramUpdate};
use crate::util::report;

const API: &str = "https://api.telegram.org";

#[derive(Debug, Deserialize)]
struct ApiResult<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<u16>,
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
    let response = match agent().post(&url).send_json(body) {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            let error: serde_json::Value = response.into_json().unwrap_or_default();
            return Err(TelegramApiError {
                method: method.into(),
                status,
                description: error["description"]
                    .as_str()
                    .unwrap_or("request rejected")
                    .into(),
            }
            .into());
        }
        Err(_) => bail!("telegram {method} connection failed; check network access"),
    };
    let parsed: ApiResult<T> = response.into_json().context("telegram json")?;
    if !parsed.ok {
        return Err(TelegramApiError {
            method: method.into(),
            status: parsed.error_code.unwrap_or(0),
            description: parsed.description.unwrap_or_else(|| "unknown error".into()),
        }
        .into());
    }
    parsed
        .result
        .ok_or_else(|| anyhow::anyhow!("telegram {method} returned no result"))
}

pub fn is_poll_conflict(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TelegramApiError>()
        .is_some_and(|error| error.status == 409 && error.method == "getUpdates")
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
    let _: bool = call(token, "setMyCommands", json!({ "commands": commands }))?;
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

pub fn send_message_reply(token: &str, chat_id: i64, text: &str, reply_to: i64) -> Result<i64> {
    send_json(token, chat_id, text, Some(reply_to))
}

/// Send complete agent output; record each delivered part immediately so a
/// reply to any part continues the same job, even after a later send fails.
pub fn send_agent_reply(
    token: &str,
    chat_id: i64,
    text: &str,
    reply_to: i64,
    mut delivered: impl FnMut(i64),
) -> Result<()> {
    let parts = super::telegram_format::reply_parts(text);
    let total = parts.len();
    for (index, (html, plain)) in parts.into_iter().enumerate() {
        if index > 0 {
            // Telegram limits sustained messages to a chat. Only final reply
            // parts are paced; progress messages retain their existing path.
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let prefix = if total > 1 {
            format!("({}/{total})\n", index + 1)
        } else {
            String::new()
        };
        let body = json!({
            "chat_id": chat_id,
            "text": format!("{prefix}{html}"),
            "parse_mode": "HTML",
            "reply_to_message_id": reply_to,
        });
        let sent = send_formatted_or_plain(body, &format!("{prefix}{plain}"), |body| {
            call::<SentMessage>(token, "sendMessage", body).map(|sent| sent.message_id)
        })
        .with_context(|| format!("reply delivery stopped at part {} of {total}", index + 1))?;
        delivered(sent);
    }
    Ok(())
}

fn send_formatted_or_plain(
    mut body: serde_json::Value,
    plain: &str,
    mut send: impl FnMut(serde_json::Value) -> Result<i64>,
) -> Result<i64> {
    match send(body.clone()) {
        Ok(id) => Ok(id),
        Err(err)
            if err.to_string().contains("(400)")
                && (err.to_string().contains("parse entities")
                    || err.to_string().contains("Unsupported start tag")) =>
        {
            body.as_object_mut()
                .expect("message object")
                .remove("parse_mode");
            body["text"] = json!(plain);
            send(body)
        }
        Err(err) => Err(err),
    }
}

fn send_json(token: &str, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<i64> {
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
    if code.is_empty() { None } else { Some(code) }
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
    use super::{BOT_COMMANDS, start_payload};

    #[test]
    fn poll_conflict_uses_status_and_method_even_with_context() {
        let conflict = anyhow::Error::new(crate::models::TelegramApiError {
            method: "getUpdates".into(),
            status: 409,
            description: "Conflict".into(),
        })
        .context("poll failed");
        assert!(super::is_poll_conflict(&conflict));
        assert!(!super::is_poll_conflict(&anyhow::anyhow!(
            "409 unrelated text"
        )));
        let send = anyhow::Error::new(crate::models::TelegramApiError {
            method: "sendMessage".into(),
            status: 409,
            description: "Conflict".into(),
        });
        assert!(!super::is_poll_conflict(&send));
    }

    #[test]
    fn formatting_rejection_retries_plain_but_network_failure_does_not() {
        let mut calls = 0;
        let id = super::send_formatted_or_plain(
            serde_json::json!({"text":"<b>hello</b>", "parse_mode":"HTML"}),
            "hello",
            |body| {
                calls += 1;
                if calls == 1 {
                    anyhow::bail!("telegram sendMessage failed (400): can't parse entities");
                }
                assert!(body.get("parse_mode").is_none());
                assert_eq!(body["text"], "hello");
                Ok(42)
            },
        )
        .unwrap();
        assert_eq!(id, 42);
        assert_eq!(calls, 2);
        calls = 0;
        assert!(
            super::send_formatted_or_plain(serde_json::json!({"text":"hi"}), "hi", |_| {
                calls += 1;
                anyhow::bail!("network failed")
            })
            .is_err()
        );
        assert_eq!(calls, 1);
    }

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
