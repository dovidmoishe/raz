use anyhow::{bail, Result};

use crate::integrations::{engines, github};
use crate::models::{Engine, State};
use crate::projects;
use crate::turns::{self, Submit};

pub enum Reply {
    Message(String),
    BindStatus { job_id: String, text: String },
    BindMany { jobs: Vec<(String, String)> },
    EditStatus {
        job_id: String,
        confirm: String,
        text: String,
    },
    GithubLogin,
}

pub fn dispatch(
    state: &mut State,
    text: &str,
    chat_id: i64,
    reply_to: Option<i64>,
) -> Result<Reply> {
    let text = text.trim();
    if let Some(result) = projects::handle_line(state, text) {
        return result.map(Reply::Message);
    }
    if let Some(result) = turns::handle_line(state, text) {
        return result.map(Reply::Message);
    }
    if let Some(rest) = strip_cmd(text, "/run") {
        return submit_reply(parse_run(state, rest, chat_id));
    }
    if let Some(rest) = strip_cmd(text, "/both") {
        if rest.is_empty() {
            bail!("usage: /both <prompt>");
        }
        return submit_reply(turns::submit_both(state, rest, chat_id));
    }
    if let Some(rest) = strip_verbose(text) {
        if rest.is_empty() {
            return turns::enable_verbose(state).map(Reply::Message);
        }
        return submit_reply(turns::submit_verbose(state, rest, chat_id));
    }
    if text == "/github" || text.starts_with("/github ") {
        engines::refresh_github(state)?;
        if state.github.ready {
            return Ok(Reply::Message(github::ready_message(state)));
        }
        return Ok(Reply::GithubLogin);
    }
    if text.starts_with('/') {
        return Ok(Reply::Message("unknown command. /help".into()));
    }
    if let Some(mid) = reply_to {
        if let Some(job_id) = turns::find_job_by_status_id(state, mid) {
            return submit_reply(turns::follow_job(state, &job_id, text, chat_id));
        }
    }
    submit_reply(turns::submit(state, text, chat_id))
}

fn parse_run(state: &mut State, rest: &str, chat_id: i64) -> Result<Submit> {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let engine = parts.next().unwrap_or("");
    let prompt = parts.next().unwrap_or("").trim();
    if engine.is_empty() || prompt.is_empty() {
        bail!("usage: /run claude|codex <prompt>");
    }
    let engine = Engine::parse(engine)
        .map_err(|_| anyhow::anyhow!("usage: /run claude|codex <prompt>"))?;
    turns::submit_run(state, engine, prompt, chat_id)
}

fn strip_cmd<'a>(text: &'a str, cmd: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(cmd)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

fn strip_verbose(text: &str) -> Option<&str> {
    strip_cmd(text, "/verbose")
}

fn submit_reply(result: Result<Submit>) -> Result<Reply> {
    match result? {
        Submit::Started { job_id, text } => Ok(Reply::BindStatus { job_id, text }),
        Submit::Queued {
            job_id,
            confirm,
            status,
        } => Ok(Reply::EditStatus {
            job_id,
            confirm,
            text: status,
        }),
        Submit::StartedMany { jobs } => Ok(Reply::BindMany { jobs }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Engine;
    use crate::projects;
    use std::fs;
    use tempfile::TempDir;

    fn primed() -> (TempDir, State) {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        let mut state = State::new(dir.path());
        state.engines.claude.ready = true;
        state.current.engine = Some(Engine::Claude);
        projects::register(&mut state, "shop", "shop").unwrap();
        (dir, state)
    }

    #[test]
    fn unknown_slash_is_help_not_a_turn() {
        let (_dir, mut state) = primed();
        let Reply::Message(text) = dispatch(&mut state, "/nope", 1, None).unwrap() else {
            panic!("expected message");
        };
        assert!(text.contains("/help"), "{text}");
        assert!(state.jobs.is_empty());
    }

    #[test]
    fn free_text_binds_a_job_status() {
        let (_dir, mut state) = primed();
        let Reply::BindStatus { job_id, text } =
            dispatch(&mut state, "ship the cart", 7, None).unwrap()
        else {
            panic!("expected bind");
        };
        assert!(text.contains(&job_id));
        assert_eq!(state.jobs[&job_id].prompt, "ship the cart");
        assert!(!state.jobs[&job_id].verbose);
    }

    #[test]
    fn verbose_prompt_flags_the_job() {
        let (_dir, mut state) = primed();
        let Reply::BindStatus { job_id, .. } =
            dispatch(&mut state, "/verbose look closer", 7, None).unwrap()
        else {
            panic!("expected bind");
        };
        assert!(state.jobs[&job_id].verbose);
        assert_eq!(state.jobs[&job_id].prompt, "look closer");
    }

    #[test]
    fn github_is_not_a_turn() {
        let (_dir, mut state) = primed();
        match dispatch(&mut state, "/github", 1, None).unwrap() {
            Reply::GithubLogin | Reply::Message(_) => {}
            Reply::BindStatus { .. } | Reply::EditStatus { .. } | Reply::BindMany { .. } => {
                panic!("github must not start an engine job")
            }
        }
        assert!(state.jobs.is_empty());
    }

    #[test]
    fn pr_command_is_not_a_turn() {
        let (_dir, mut state) = primed();
        let Reply::Message(text) = dispatch(&mut state, "/pr", 1, None).unwrap() else {
            panic!("expected message");
        };
        assert_eq!(text, "none yet");
    }

    #[test]
    fn run_does_not_flip_current_engine() {
        let (_dir, mut state) = primed();
        state.engines.codex.ready = true;
        state.current.engine = Some(Engine::Claude);
        let Reply::BindStatus { job_id, .. } =
            dispatch(&mut state, "/run codex ship it", 7, None).unwrap()
        else {
            panic!("expected bind");
        };
        assert_eq!(state.current.engine, Some(Engine::Claude));
        assert_eq!(state.jobs[&job_id].engine, Engine::Codex);
        assert_eq!(state.jobs[&job_id].prompt, "ship it");
    }

    #[test]
    fn both_binds_two_status_cards() {
        let (_dir, mut state) = primed();
        state.engines.codex.ready = true;
        let Reply::BindMany { jobs } =
            dispatch(&mut state, "/both review the cart", 7, None).unwrap()
        else {
            panic!("expected bind many");
        };
        assert_eq!(jobs.len(), 2);
        assert_eq!(state.jobs.len(), 2);
    }

    #[test]
    fn reply_to_status_continues_that_job() {
        let (_dir, mut state) = primed();
        state.engines.codex.ready = true;
        let Reply::BindStatus { job_id, .. } =
            dispatch(&mut state, "first", 7, None).unwrap()
        else {
            panic!("expected bind");
        };
        state.jobs.get_mut(&job_id).unwrap().telegram_status_id = Some(99);
        state.current.engine = Some(Engine::Codex);
        let Reply::EditStatus { job_id: followed, .. } =
            dispatch(&mut state, "keep going", 7, Some(99)).unwrap()
        else {
            panic!("expected edit");
        };
        assert_eq!(followed, job_id);
        assert_eq!(
            state.jobs[&job_id].queued_prompt.as_deref(),
            Some("keep going")
        );
        assert_eq!(state.jobs[&job_id].engine, Engine::Claude);
    }

    #[test]
    fn slash_is_not_stolen_by_reply_to() {
        let (_dir, mut state) = primed();
        let Reply::BindStatus { job_id, .. } =
            dispatch(&mut state, "first", 7, None).unwrap()
        else {
            panic!("expected bind");
        };
        state.jobs.get_mut(&job_id).unwrap().telegram_status_id = Some(99);
        let Reply::Message(text) =
            dispatch(&mut state, "/status", 7, Some(99)).unwrap()
        else {
            panic!("expected status");
        };
        assert!(text.contains("project: shop"), "{text}");
        assert!(state.jobs[&job_id].queued_prompt.is_none());
    }
}
