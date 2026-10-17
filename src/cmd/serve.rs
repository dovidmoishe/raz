use anyhow::{bail, Result};

use crate::cmd::bot::{self, Reply};
use crate::integrations::{engines, github, telegram};
use crate::models::{now_unix, Paths, State, TelegramUpdate};
use crate::turns::{self, JobEvent, Live};
use crate::util::{is_root, report, Limiter};

const SLASH_WINDOW_SECS: u64 = 20;
const SLASH_MAX: usize = 8;
const RUN_WINDOW_SECS: u64 = 20;
const RUN_MAX: usize = 4;

pub fn run() -> Result<()> {
    // Long-poll Telegram only. No inbound bind, no agent ports.
    // If a health file is ever added, it stays on localhost.
    if is_root() {
        bail!("do not run raz as root — use a dedicated user");
    }
    let paths = Paths::resolve()?;
    if !paths.state.exists() {
        bail!("no state.json — run raz setup");
    }
    let mut state = State::load(&paths.state)?;
    let token = paths
        .telegram_token()?
        .ok_or_else(|| anyhow::anyhow!("no telegram token — run raz setup"))?;
    if let Err(err) = telegram::install_menu(&token) {
        report::warn("telegram", format!("command menu: {err:#}"));
    }

    if !state.any_engine_ready() {
        engines::refresh_engines(&mut state);
        state.save(&paths.state)?;
    }
    if !state.any_engine_ready() {
        bail!("no engine is authenticated — run raz auth");
    }

    Live::recover_orphans(&mut state);
    state.save(&paths.state)?;
    let mut live = Live::with_events(Some(paths.events.clone()));
    let mut github_login: Option<github::LoginChild> = None;
    let mut slash_limit = Limiter::new(SLASH_WINDOW_SECS, SLASH_MAX);
    let mut run_limit = Limiter::new(RUN_WINDOW_SECS, RUN_MAX);

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
        let events = {
            let mut events = live.tick(&mut state);
            events.extend(live.kick(&mut state));
            events
        };
        emit(&token, &mut state, &events);
        tick_github(&mut github_login, &mut state, &token);
        state.save(&paths.state)?;

        let timeout = if live.is_empty() && github_login.is_none() {
            25
        } else {
            1
        };
        let updates = match telegram::get_updates(&token, state.telegram.last_update_id + 1, timeout)
        {
            Ok(u) => u,
            Err(err) => {
                report::warn("serve", err.to_string());
                continue;
            }
        };
        for update in updates {
            state.telegram.last_update_id = update.update_id;
            handle(
                &paths,
                &mut state,
                &mut live,
                &mut github_login,
                &mut slash_limit,
                &mut run_limit,
                &token,
                &update,
            )?;
        }
        live.reap_stopped(&state);
        state.save(&paths.state)?;
    }
}

fn handle(
    paths: &Paths,
    state: &mut State,
    live: &mut Live,
    github_login: &mut Option<github::LoginChild>,
    slash_limit: &mut Limiter,
    run_limit: &mut Limiter,
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

    let chat_id = message.chat.id;
    if let Some(run) = classify_rate(text) {
        let now = now_unix();
        let allowed = if run {
            run_limit.allow(now)
        } else {
            slash_limit.allow(now)
        };
        if !allowed {
            telegram::send_message(
                token,
                chat_id,
                "slow down — try again in a few seconds",
            )?;
            return Ok(());
        }
    }
    let reply_to = message
        .reply_to_message
        .as_ref()
        .map(|m| m.message_id);
    let stopping = text == "/stop" || text.starts_with("/stop ");
    match bot::dispatch(state, text, chat_id, reply_to) {
        Ok(Reply::Message(text)) => {
            telegram::send_message(token, chat_id, &text)?;
        }
        Ok(Reply::BindStatus { job_id, text }) => {
            let mid = telegram::send_message(token, chat_id, &text)?;
            if let Some(job) = state.jobs.get_mut(&job_id) {
                job.telegram_status_id = Some(mid);
                job.chat_id = Some(chat_id);
            }
        }
        Ok(Reply::BindMany { jobs }) => {
            for (job_id, text) in jobs {
                let mid = telegram::send_message(token, chat_id, &text)?;
                if let Some(job) = state.jobs.get_mut(&job_id) {
                    job.telegram_status_id = Some(mid);
                    job.chat_id = Some(chat_id);
                }
            }
        }
        Ok(Reply::EditStatus {
            job_id,
            confirm,
            text,
        }) => {
            if let Some(job) = state.jobs.get(&job_id) {
                if let (Some(cid), Some(mid)) = (job.chat_id, job.telegram_status_id) {
                    if let Err(err) = telegram::edit_message(token, cid, mid, &text) {
                        report::warn("telegram", err.to_string());
                    }
                }
            }
            telegram::send_message(token, chat_id, &confirm)?;
        }
        Ok(Reply::GithubLogin) => start_github_login(github_login, token, chat_id)?,
        Err(err) => {
            let hide = stopping
                && github_login.is_some()
                && err.to_string().contains("nothing to stop");
            if !hide {
                report::warn("serve", format!("{err:#}"));
                telegram::send_message(token, chat_id, &turns::humanize(&err))?;
            }
        }
    }
    if stopping && github_login.is_some() {
        stop_github_login(github_login);
        telegram::send_message(token, chat_id, "stopped github login")?;
    }
    live.reap_stopped(state);
    let events = live.kick(state);
    emit(token, state, &events);
    tick_github(github_login, state, token);
    state.save(&paths.state)?;
    Ok(())
}

fn start_github_login(
    github_login: &mut Option<github::LoginChild>,
    token: &str,
    chat_id: i64,
) -> Result<()> {
    if github_login.is_some() {
        telegram::send_message(
            token,
            chat_id,
            "github login already waiting — use the code I sent",
        )?;
        return Ok(());
    }
    match github::LoginChild::spawn(chat_id) {
        Ok(mut child) => {
            let mid = telegram::send_message(token, chat_id, "starting github login")?;
            child.message_id = Some(mid);
            *github_login = Some(child);
        }
        Err(err) => {
            report::warn("github", format!("{err:#}"));
            telegram::send_message(token, chat_id, &turns::humanize(&err))?;
        }
    }
    Ok(())
}

fn stop_github_login(github_login: &mut Option<github::LoginChild>) {
    if let Some(mut child) = github_login.take() {
        child.stop();
        report::info("github", "login stopped");
    }
}

fn tick_github(
    github_login: &mut Option<github::LoginChild>,
    state: &mut State,
    token: &str,
) {
    let event = match github_login.as_mut() {
        Some(child) => child.tick(),
        None => return,
    };
    match event {
        Some(github::LoginTick::Prompt(prompt)) => {
            let (chat_id, message_id) = github_login
                .as_ref()
                .map(|child| (child.chat_id, child.message_id))
                .expect("login child");
            let text = github::format_device_prompt(&prompt);
            if let Some(mid) = message_id {
                if let Err(err) = telegram::edit_message(token, chat_id, mid, &text) {
                    report::warn("telegram", err.to_string());
                }
            } else if let Ok(mid) = telegram::send_message(token, chat_id, &text) {
                if let Some(child) = github_login.as_mut() {
                    child.message_id = Some(mid);
                }
            }
        }
        Some(github::LoginTick::Done { ok, detail }) => {
            let chat_id = github_login
                .as_ref()
                .map(|child| child.chat_id)
                .expect("login child");
            *github_login = None;
            let body = if ok {
                github::finish_success(state).unwrap_or_else(|err| turns::humanize(&err))
            } else {
                detail
            };
            if let Err(err) = telegram::send_message(token, chat_id, &body) {
                report::warn("telegram", err.to_string());
            }
        }
        None => {}
    }
}

fn classify_rate(text: &str) -> Option<bool> {
    if is_cmd(text, "/stop") {
        return None;
    }
    if is_cmd(text, "/run") || is_cmd(text, "/both") {
        return Some(true);
    }
    if is_cmd(text, "/verbose") {
        let prompt = text
            .strip_prefix("/verbose")
            .unwrap_or("")
            .trim();
        if !prompt.is_empty() {
            return Some(true);
        }
    }
    if text.starts_with('/') {
        return Some(false);
    }
    None
}

fn is_cmd(text: &str, cmd: &str) -> bool {
    text == cmd || text.starts_with(&format!("{cmd} "))
}

fn emit(token: &str, state: &mut State, events: &[JobEvent]) {
    for event in events {
        match event {
            JobEvent::Status { job_id } => {
                let Some(job) = state.jobs.get(job_id) else {
                    continue;
                };
                let Some(cid) = job.chat_id else {
                    continue;
                };
                let Some(mid) = job.telegram_status_id else {
                    continue;
                };
                let text = turns::format_status(job_id, job);
                if let Err(err) = telegram::edit_message(token, cid, mid, &text) {
                    report::warn("telegram", err.to_string());
                }
            }
            JobEvent::Finished {
                job_id,
                ok: _,
                summary,
                pr,
            } => {
                let Some(job) = state.jobs.get(job_id) else {
                    continue;
                };
                if let (Some(cid), Some(mid)) = (job.chat_id, job.telegram_status_id) {
                    let text = turns::format_status(job_id, job);
                    if let Err(err) = telegram::edit_message(token, cid, mid, &text) {
                        report::warn("telegram", err.to_string());
                    }
                    let body = turns::final_reply(summary, pr.as_deref());
                    if let Err(err) = telegram::send_message_reply(token, cid, &body, mid) {
                        report::warn("telegram", err.to_string());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_rate, is_cmd};

    #[test]
    fn stop_is_not_rate_limited() {
        assert_eq!(classify_rate("/stop"), None);
        assert_eq!(classify_rate("/stop j_ab3"), None);
        assert!(is_cmd("/stop j_ab3", "/stop"));
    }

    #[test]
    fn run_and_both_use_tight_limit() {
        assert_eq!(classify_rate("/run claude go"), Some(true));
        assert_eq!(classify_rate("/both go"), Some(true));
        assert_eq!(classify_rate("/verbose look closer"), Some(true));
        assert_eq!(classify_rate("/verbose"), Some(false));
        assert_eq!(classify_rate("/status"), Some(false));
        assert_eq!(classify_rate("free text task"), None);
        assert!(!is_cmd("/runway", "/run"));
    }
}
