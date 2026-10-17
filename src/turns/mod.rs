//! Job queue and Telegram turn commands.
//!
//! One writer per checkout (worktree or read overlap). Queue at most one extra
//! prompt per job. `/stop` never kills the harness. Runners spawn
//! `claude -p` / `codex exec` as child processes.

mod brief;
mod live;

use std::path::Path;

use anyhow::{bail, Result};

use crate::integrations::git;
use crate::models::{now_unix, Engine, Job, State};
use crate::util::{clip_chars, clip_telegram, normalize, under_root, TELEGRAM_MAX};

pub use brief::{resolve_cwd, resolve_job_cwd, wrap_job_prompt, wrap_prompt};
pub use live::{JobEvent, Live};

pub const STATUS_GAP_SECS: u64 = 2;
pub const DEFAULT_JOBS: usize = 2;
pub const MAX_JOBS: usize = 4;

#[derive(Debug)]
pub enum Submit {
    Started { job_id: String, text: String },
    Queued {
        job_id: String,
        confirm: String,
        status: String,
    },
    StartedMany {
        jobs: Vec<(String, String)>,
    },
}

pub fn handle_line(state: &mut State, text: &str) -> Option<Result<String>> {
    let mut parts = text.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    match cmd {
        "/use" => Some(use_engine(state, parts.next())),
        "/new" => Some(new_session(state, parts.next())),
        "/stop" => Some(stop(state, parts.next())),
        "/perm" => Some(set_perm(state, parts.next())),
        "/jobs" => Some(Ok(jobs_block(state))),
        _ => None,
    }
}

pub fn use_engine(state: &mut State, name: Option<&str>) -> Result<String> {
    let Some(name) = name else {
        bail!("usage: /use claude|codex");
    };
    let engine = Engine::parse(name)?;
    if !engine_ready(state, engine) {
        bail!(
            "{} is not authenticated. run raz auth {}",
            engine.as_str(),
            engine.as_str()
        );
    }
    state.current.engine = Some(engine);
    state.current.handoff = true;
    let mut reply = format!("using {}", engine.as_str());
    if let Some(name) = state.current.project.as_deref() {
        if let Some(project) = state.projects.get(name) {
            if let Some(summary) = project.last_summary.as_deref() {
                reply.push_str("\nlast: ");
                reply.push_str(summary);
            } else if let Some(pr) = project.last_pr.as_deref() {
                reply.push_str("\nlast: ");
                reply.push_str(pr);
            }
        }
    }
    Ok(reply)
}

pub fn new_session(state: &mut State, id: Option<&str>) -> Result<String> {
    match id {
        Some(id) => new_job_session(state, id),
        None => new_current_session(state),
    }
}

fn new_current_session(state: &mut State) -> Result<String> {
    let name = require_project(state)?;
    let engine = require_engine(state)?;
    {
        let project = state
            .projects
            .get_mut(&name)
            .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
        *project.session_mut(engine) = None;
    }
    for job in state.jobs.values_mut() {
        if job.project == name
            && job.engine == engine
            && job.worktree.is_none()
            && job.state == "queued"
        {
            job.session = None;
        }
    }
    Ok(format!("new {} session on {name}", engine.as_str()))
}

fn new_job_session(state: &mut State, id: &str) -> Result<String> {
    let job = state
        .jobs
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {id}"))?;
    let engine = job.engine;
    let project_name = job.project.clone();
    let session = job.session.clone();
    let occupying = job.occupies_checkout();
    let main = job.worktree.is_none();
    if let Some(job) = state.jobs.get_mut(id) {
        job.session = None;
    }
    if main {
        if let Some(project) = state.projects.get_mut(&project_name) {
            if project.session(engine) == session.as_deref() {
                *project.session_mut(engine) = None;
            }
        }
    }
    if !occupying {
        prune_job_worktree(state, id)?;
    }
    Ok(format!("new {} session on {id}", engine.as_str()))
}

pub fn set_perm(state: &mut State, perm: Option<&str>) -> Result<String> {
    let perm = perm.unwrap_or("").to_ascii_lowercase();
    if perm != "read" && perm != "write" {
        bail!("usage: /perm read|write");
    }
    let name = require_project(state)?;
    let project = state
        .projects
        .get_mut(&name)
        .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
    project.perm = perm.clone();
    Ok(format!("{name} perm {perm}"))
}

pub fn stop(state: &mut State, id: Option<&str>) -> Result<String> {
    let id = match id {
        Some(id) => id.to_string(),
        None => default_job_id(state).ok_or_else(|| anyhow::anyhow!("nothing to stop"))?,
    };
    let job = state
        .jobs
        .get_mut(&id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {id}"))?;
    if !job.occupies_checkout() {
        bail!("{id} is already {}", job.state);
    }
    job.state = "stopped".into();
    job.queued_prompt = None;
    job.last_line = Some("stopped".into());
    job.last_status_unix = Some(now_unix());
    Ok(format!("stopped {id}"))
}

pub fn enable_verbose(state: &mut State) -> Result<String> {
    let id = default_job_id(state)
        .ok_or_else(|| anyhow::anyhow!("usage: /verbose <prompt> — or /verbose while a job is running"))?;
    let job = state
        .jobs
        .get_mut(&id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {id}"))?;
    job.verbose = true;
    Ok(format!("{id} verbose on for this turn"))
}

pub fn submit(state: &mut State, prompt: &str, chat_id: i64) -> Result<Submit> {
    submit_inner(state, prompt, chat_id, false, SubmitKind::Default)
}

pub fn submit_verbose(state: &mut State, prompt: &str, chat_id: i64) -> Result<Submit> {
    submit_inner(state, prompt, chat_id, true, SubmitKind::Default)
}

pub fn submit_run(
    state: &mut State,
    engine: Engine,
    prompt: &str,
    chat_id: i64,
) -> Result<Submit> {
    submit_inner(state, prompt, chat_id, false, SubmitKind::Run { engine })
}

pub fn submit_both(state: &mut State, prompt: &str, chat_id: i64) -> Result<Submit> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        bail!("usage: /both <prompt>");
    }
    let name = require_project(state)?;
    if !engine_ready(state, Engine::Claude) || !engine_ready(state, Engine::Codex) {
        bail!("/both needs both engines authenticated. run raz auth");
    }
    let perm = {
        let project = state
            .projects
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
        brief::resolve_cwd(project)?;
        project.perm.clone()
    };
    ensure_capacity(state, 2)?;
    let write = perm == "write";
    if !write {
        if let Some(id) = blocks_on(
            state,
            &project_path_key(state, &name).ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?,
            &perm,
        ) {
            bail!(
                "{name} is writing ({id}). wait, /stop, or reply to that job."
            );
        }
    }
    let id_claude = new_job_id(state);
    let mut used = std::collections::BTreeSet::from([id_claude.clone()]);
    let id_codex = loop {
        let id = new_job_id(state);
        if used.insert(id.clone()) {
            break id;
        }
    };
    let mut started = Vec::new();
    for (id, engine) in [
        (id_claude, Engine::Claude),
        (id_codex, Engine::Codex),
    ] {
        match start_new_job(
            state,
            &id,
            &name,
            engine,
            prompt,
            chat_id,
            false,
            write,
            true,
        ) {
            Ok(text) => started.push((id, text)),
            Err(err) => {
                for (prev, _) in &started {
                    let _ = prune_job_worktree(state, prev);
                    state.jobs.remove(prev);
                }
                return Err(err);
            }
        }
    }
    Ok(Submit::StartedMany { jobs: started })
}

pub fn follow_job(
    state: &mut State,
    job_id: &str,
    prompt: &str,
    chat_id: i64,
) -> Result<Submit> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        bail!("empty message");
    }
    let occupies = state
        .jobs
        .get(job_id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {job_id}"))?
        .occupies_checkout();
    if occupies {
        return queue_follow(state, job_id, prompt);
    }
    ensure_capacity(state, 1)?;
    let job = state
        .jobs
        .get_mut(job_id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {job_id}"))?;
    job.prompt = prompt.to_string();
    job.queued_prompt = None;
    job.state = "queued".into();
    job.verbose = false;
    job.chat_id = Some(chat_id);
    apply_status(job, "waiting to run", now_unix(), true);
    let text = format_status(job_id, job);
    Ok(Submit::Started {
        job_id: job_id.to_string(),
        text,
    })
}

pub fn find_job_by_status_id(state: &State, message_id: i64) -> Option<String> {
    state.jobs.iter().find_map(|(id, job)| {
        (job.telegram_status_id == Some(message_id)).then(|| id.clone())
    })
}

enum SubmitKind {
    Default,
    Run { engine: Engine },
}

fn submit_inner(
    state: &mut State,
    prompt: &str,
    chat_id: i64,
    verbose: bool,
    kind: SubmitKind,
) -> Result<Submit> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        bail!("empty message");
    }
    let name = require_project(state)?;
    let engine = match &kind {
        SubmitKind::Default => require_engine(state)?,
        SubmitKind::Run { engine } => *engine,
    };
    if !engine_ready(state, engine) {
        bail!(
            "{} is not authenticated. run raz auth {}",
            engine.as_str(),
            engine.as_str()
        );
    }
    let perm = {
        let project = state
            .projects
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
        brief::resolve_cwd(project)?;
        project.perm.clone()
    };

    if matches!(kind, SubmitKind::Default) {
        if let Some(id) = default_follow_job(state, &name, engine) {
            return queue_follow(state, &id, prompt);
        }
    }

    ensure_capacity(state, 1)?;
    let write = perm == "write";
    let main_key = project_path_key(state, &name)
        .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
    let blocked = blocks_on(state, &main_key, &perm);
    let need_worktree = write && blocked.is_some();
    if !write {
        if let Some(id) = blocked {
            bail!(
                "{name} is writing ({id}). wait, /stop, or reply to that job."
            );
        }
    }
    let engine_busy = occupying_engine_on_project(state, &name, engine);
    let fresh_session =
        need_worktree || (matches!(kind, SubmitKind::Run { .. }) && engine_busy);
    let id = new_job_id(state);
    let text = start_new_job(
        state,
        &id,
        &name,
        engine,
        prompt,
        chat_id,
        verbose,
        need_worktree,
        fresh_session,
    )?;
    Ok(Submit::Started { job_id: id, text })
}

fn start_new_job(
    state: &mut State,
    id: &str,
    name: &str,
    engine: Engine,
    prompt: &str,
    chat_id: i64,
    verbose: bool,
    need_worktree: bool,
    fresh_session: bool,
) -> Result<String> {
    let (perm, session, default_branch, repo) = {
        let project = state
            .projects
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown project {name}"))?;
        (
            project.perm.clone(),
            if fresh_session {
                None
            } else {
                project.session(engine).map(str::to_string)
            },
            project.default_branch.clone(),
            project.path.clone(),
        )
    };
    let worktree = if need_worktree {
        Some(attach_worktree(state, name, id, &repo, &default_branch)?)
    } else {
        None
    };
    let mut job = Job {
        project: name.to_string(),
        engine,
        session,
        worktree,
        perm,
        kind: "ship".into(),
        state: "queued".into(),
        telegram_status_id: None,
        chat_id: Some(chat_id),
        prompt: prompt.to_string(),
        queued_prompt: None,
        last_line: Some("waiting to run".into()),
        last_status_unix: Some(now_unix()),
        verbose,
    };
    apply_status(&mut job, "waiting to run", now_unix(), true);
    let text = format_status(id, &job);
    state.jobs.insert(id.to_string(), job);
    Ok(text)
}

fn attach_worktree(
    state: &State,
    project: &str,
    job_id: &str,
    repo: &str,
    default_branch: &str,
) -> Result<String> {
    let dest = worktree_dest(state, project, job_id)?;
    git::worktree_add(
        Path::new(repo),
        &dest,
        &format!("raz/{job_id}"),
        default_branch,
    )?;
    Ok(dest.to_string_lossy().replace('\\', "/"))
}

fn worktree_dest(state: &State, project: &str, job_id: &str) -> Result<std::path::PathBuf> {
    let root = Path::new(&state.workspace_root);
    let rel = Path::new(".worktrees").join(project).join(job_id);
    under_root(root, &rel)
}

fn prune_job_worktree(state: &mut State, id: &str) -> Result<()> {
    let (repo, wt) = {
        let Some(job) = state.jobs.get(id) else {
            return Ok(());
        };
        let Some(wt) = job.worktree.clone() else {
            return Ok(());
        };
        let repo = state
            .projects
            .get(&job.project)
            .map(|p| p.path.clone())
            .unwrap_or_default();
        (repo, wt)
    };
    git::worktree_remove(Path::new(&repo), Path::new(&wt))?;
    if let Some(job) = state.jobs.get_mut(id) {
        job.worktree = None;
    }
    Ok(())
}

fn queue_follow(state: &mut State, id: &str, prompt: &str) -> Result<Submit> {
    let job = state
        .jobs
        .get_mut(id)
        .ok_or_else(|| anyhow::anyhow!("unknown job {id}"))?;
    let name = job.project.clone();
    if job.queued_prompt.is_some() {
        bail!(
            "{name} is already writing ({id}) with a message queued. /stop, wait, or switch project."
        );
    }
    job.queued_prompt = Some(prompt.to_string());
    let line = match job.last_line.as_deref() {
        Some(existing) if existing.contains("+1 queued") => existing.to_string(),
        Some(existing) => format!("{existing} · +1 queued"),
        None => "+1 queued".into(),
    };
    apply_status(job, &line, now_unix(), true);
    let status = format_status(id, job);
    Ok(Submit::Queued {
        confirm: format!("queued behind {id} (queue of 1)"),
        status,
        job_id: id.to_string(),
    })
}

pub fn format_status(id: &str, job: &Job) -> String {
    let mut lines = vec![
        format!("{id} {}", job.state),
        format!(
            "{} · {} · {}",
            job.engine.as_str(),
            job.project,
            job.perm
        ),
    ];
    if let Some(line) = job.last_line.as_deref() {
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines.join("\n")
}

pub fn apply_status(job: &mut Job, line: &str, now: u64, force: bool) -> bool {
    let line = clip_chars(line, 200);
    if !force {
        if job.last_line.as_deref() == Some(line.as_str()) {
            return false;
        }
        if let Some(prev) = job.last_status_unix {
            if now.saturating_sub(prev) < STATUS_GAP_SECS {
                return false;
            }
        }
    }
    job.last_line = Some(line);
    job.last_status_unix = Some(now);
    true
}

pub fn final_reply(summary: &str, pr: Option<&str>) -> String {
    match pr {
        Some(url) if !url.is_empty() => {
            let suffix = format!("\n{url}");
            let budget = TELEGRAM_MAX.saturating_sub(suffix.chars().count());
            let mut body = clip_chars(summary, budget);
            body.push_str(&suffix);
            body
        }
        _ => clip_telegram(summary),
    }
}

pub fn humanize(err: &anyhow::Error) -> String {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if (lower.contains("auth") || lower.contains("401") || lower.contains("403"))
        && (lower.contains("expir")
            || lower.contains("unauthor")
            || lower.contains("forbidden")
            || lower.contains("401")
            || lower.contains("403"))
    {
        return "engine login expired. run raz auth claude or raz auth codex"
            .into();
    }
    if lower.contains("missing binary")
        || ((lower.contains("not found") || lower.contains("cannot find"))
            && (lower.contains("claude")
                || lower.contains("codex")
                || lower.contains("gh ")
                || lower.contains("git")))
    {
        return "a required binary is missing. run raz doctor".into();
    }
    clip_telegram(&msg)
}

pub fn jobs_block(state: &State) -> String {
    let mut live: Vec<_> = state
        .jobs
        .iter()
        .filter(|(_, job)| job.occupies_checkout())
        .collect();
    let mut recent: Vec<_> = state
        .jobs
        .iter()
        .filter(|(_, job)| !job.occupies_checkout())
        .collect();
    recent.sort_by_key(|(_, job)| std::cmp::Reverse(job.last_status_unix.unwrap_or(0)));
    recent.truncate(8);
    if live.is_empty() && recent.is_empty() {
        return "jobs: none".into();
    }
    live.sort_by_key(|(id, _)| *id);
    let mut lines = vec!["jobs:".to_string()];
    for (id, job) in live.into_iter().chain(recent) {
        lines.push(format_job_line(id, job));
    }
    lines.join("\n")
}

fn format_job_line(id: &str, job: &Job) -> String {
    let extra = if job.queued_prompt.is_some() {
        " +1 queued"
    } else {
        ""
    };
    let last = job.last_line.as_deref().unwrap_or("-");
    format!(
        "  {id} · {} · {} · {} · {last}{extra}",
        job.engine.as_str(),
        job.project,
        job.state
    )
}

pub fn worktree_jail_ok(state: &State) -> Result<()> {
    let root = Path::new(&state.workspace_root);
    for (id, job) in &state.jobs {
        let Some(wt) = job.worktree.as_deref() else {
            continue;
        };
        under_root(root, Path::new(wt)).map_err(|_| {
            anyhow::anyhow!("{id} worktree escapes workspace")
        })?;
    }
    Ok(())
}

pub fn job_cap() -> usize {
    DEFAULT_JOBS.min(MAX_JOBS)
}

pub fn occupying_count(state: &State) -> usize {
    state.jobs.values().filter(|job| job.occupies_checkout()).count()
}

fn engine_ready(state: &State, engine: Engine) -> bool {
    match engine {
        Engine::Claude => state.engines.claude.ready,
        Engine::Codex => state.engines.codex.ready,
    }
}

fn require_project(state: &State) -> Result<String> {
    state.current.project.clone().ok_or_else(|| {
        anyhow::anyhow!("no current project — /add or /clone first")
    })
}

fn require_engine(state: &mut State) -> Result<Engine> {
    if let Some(engine) = state.current.engine {
        if engine_ready(state, engine) {
            return Ok(engine);
        }
        bail!(
            "{} is not authenticated. run raz auth {}",
            engine.as_str(),
            engine.as_str()
        );
    }
    if let Some(engine) = state.pick_default_engine() {
        state.current.engine = Some(engine);
        return Ok(engine);
    }
    bail!("no engine is authenticated. run raz auth")
}

fn path_key(path: &str) -> String {
    normalize(Path::new(path))
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

pub fn job_checkout_key(state: &State, job: &Job) -> Option<String> {
    if let Some(wt) = job.worktree.as_deref() {
        return Some(path_key(wt));
    }
    let path = &state.projects.get(&job.project)?.path;
    Some(path_key(path))
}

fn project_path_key(state: &State, project: &str) -> Option<String> {
    Some(path_key(&state.projects.get(project)?.path))
}

fn blocks_on(state: &State, path_key: &str, incoming_perm: &str) -> Option<String> {
    state.jobs.iter().find_map(|(id, job)| {
        if !job.occupies_checkout() {
            return None;
        }
        let key = job_checkout_key(state, job)?;
        if key != path_key {
            return None;
        }
        if incoming_perm == "write" || job.perm == "write" {
            Some(id.clone())
        } else {
            None
        }
    })
}

fn default_follow_job(state: &State, project: &str, engine: Engine) -> Option<String> {
    state.jobs.iter().find_map(|(id, job)| {
        if job.occupies_checkout()
            && job.project == project
            && job.engine == engine
            && job.worktree.is_none()
        {
            Some(id.clone())
        } else {
            None
        }
    })
}

fn occupying_engine_on_project(state: &State, project: &str, engine: Engine) -> bool {
    state.jobs.values().any(|job| {
        job.occupies_checkout() && job.project == project && job.engine == engine
    })
}

fn ensure_capacity(state: &State, extra: usize) -> Result<()> {
    let cap = job_cap();
    let used = occupying_count(state);
    if used + extra > cap {
        bail!("at capacity ({used}/{cap}). wait or /stop");
    }
    Ok(())
}

fn default_job_id(state: &State) -> Option<String> {
    let project = state.current.project.as_deref();
    let mut running = None;
    let mut queued = None;
    let mut any_running = None;
    let mut any_queued = None;
    for (id, job) in &state.jobs {
        if !job.occupies_checkout() {
            continue;
        }
        let same = project == Some(job.project.as_str());
        if job.state == "running" {
            if same {
                running = Some(id.clone());
            }
            any_running = Some(id.clone());
        } else if job.state == "queued" {
            if same {
                queued = Some(id.clone());
            }
            any_queued = Some(id.clone());
        }
    }
    running.or(queued).or(any_running).or(any_queued)
}

fn new_job_id(state: &State) -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    loop {
        let mut id = String::from("j_");
        for _ in 0..3 {
            id.push(CHARS[rng.gen_range(0..CHARS.len())] as char);
        }
        if !state.jobs.contains_key(&id) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects;
    use std::fs;
    use tempfile::TempDir;

    fn primed() -> (TempDir, State) {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shop")).unwrap();
        fs::create_dir_all(dir.path().join("blog")).unwrap();
        let mut state = State::new(dir.path());
        state.engines.claude.ready = true;
        state.engines.codex.ready = true;
        state.current.engine = Some(Engine::Claude);
        projects::register(&mut state, "shop", "shop").unwrap();
        (dir, state)
    }

    fn occupying(state: &State) -> Vec<String> {
        state
            .jobs
            .iter()
            .filter(|(_, job)| job.occupies_checkout())
            .map(|(id, _)| id.clone())
            .collect()
    }

    #[test]
    fn use_and_perm_and_new() {
        let (_dir, mut state) = primed();
        state.projects.get_mut("shop").unwrap().sessions.claude = Some("cla-1".into());
        assert_eq!(use_engine(&mut state, Some("codex")).unwrap(), "using codex");
        assert_eq!(state.current.engine, Some(Engine::Codex));
        assert_eq!(set_perm(&mut state, Some("write")).unwrap(), "shop perm write");
        assert_eq!(state.projects["shop"].perm, "write");
        assert!(set_perm(&mut state, Some("yolo")).is_err());
        assert_eq!(
            new_session(&mut state, None).unwrap(),
            "new codex session on shop"
        );
        assert!(state.projects["shop"].sessions.codex.is_none());
        assert_eq!(
            state.projects["shop"].sessions.claude.as_deref(),
            Some("cla-1")
        );
        state.engines.codex.ready = false;
        let err = use_engine(&mut state, Some("codex"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not authenticated"), "{err}");
    }

    #[test]
    fn free_text_queues_one_then_refuses() {
        let (_dir, mut state) = primed();
        let first = submit(&mut state, "fix the cart", 9).unwrap();
        let Submit::Started { job_id, text } = first else {
            panic!("expected started");
        };
        assert!(text.contains(&job_id), "{text}");
        assert_eq!(state.jobs[&job_id].state, "queued");
        assert_eq!(state.jobs[&job_id].prompt, "fix the cart");
        assert_eq!(state.jobs[&job_id].perm, "read");

        let queued = submit(&mut state, "also the checkout", 9).unwrap();
        let Submit::Queued { confirm, .. } = queued else {
            panic!("expected queued");
        };
        assert!(confirm.contains(&job_id), "{confirm}");
        assert_eq!(
            state.jobs[&job_id].queued_prompt.as_deref(),
            Some("also the checkout")
        );

        let err = submit(&mut state, "third", 9).unwrap_err().to_string();
        assert!(err.contains("already writing"), "{err}");
        assert!(err.contains("queued"), "{err}");
        assert_eq!(occupying(&state).len(), 1);
    }

    #[test]
    fn two_projects_can_write_at_once() {
        let (_dir, mut state) = primed();
        projects::register(&mut state, "blog", "blog").unwrap();
        projects::switch(&mut state, "shop").unwrap();
        submit(&mut state, "shop task", 1).unwrap();
        projects::switch(&mut state, "blog").unwrap();
        submit(&mut state, "blog task", 1).unwrap();
        assert_eq!(occupying(&state).len(), 2);
    }

    #[test]
    fn stop_default_and_by_id() {
        let (_dir, mut state) = primed();
        let Submit::Started { job_id, .. } = submit(&mut state, "one", 1).unwrap() else {
            panic!("started");
        };
        assert_eq!(stop(&mut state, None).unwrap(), format!("stopped {job_id}"));
        assert_eq!(state.jobs[&job_id].state, "stopped");
        assert!(state.jobs[&job_id].queued_prompt.is_none());
        assert!(stop(&mut state, None).unwrap_err().to_string().contains("nothing"));

        let Submit::Started { job_id: second, .. } =
            submit(&mut state, "two", 1).unwrap()
        else {
            panic!("started");
        };
        assert!(stop(&mut state, Some(&second))
            .unwrap()
            .contains(&second));
        assert_eq!(state.jobs[&second].state, "stopped");
    }

    #[test]
    fn throttle_status_edits() {
        let mut job = Job {
            project: "shop".into(),
            engine: Engine::Claude,
            session: None,
            worktree: None,
            perm: "read".into(),
            kind: "ship".into(),
            state: "running".into(),
            telegram_status_id: None,
            chat_id: None,
            prompt: "go".into(),
            queued_prompt: None,
            last_line: None,
            last_status_unix: None,
            verbose: false,
        };
        assert!(apply_status(&mut job, "started", 100, true));
        assert!(!apply_status(&mut job, "working", 101, false));
        assert!(apply_status(&mut job, "working", 102, false));
        assert!(apply_status(&mut job, "done", 102, true));
        assert_eq!(job.last_line.as_deref(), Some("done"));
    }

    #[test]
    fn final_summary_fits_telegram_and_keeps_pr() {
        let pr = "https://github.com/acme/shop/pull/4";
        let long = "x".repeat(TELEGRAM_MAX);
        let out = final_reply(&long, Some(pr));
        assert!(out.chars().count() <= TELEGRAM_MAX);
        assert!(out.ends_with(pr), "{out}");
        assert!(out.contains('…'));
    }

    #[test]
    fn human_errors() {
        assert!(humanize(&anyhow::anyhow!("claude auth expired 401"))
            .contains("login expired"));
        assert!(humanize(&anyhow::anyhow!("codex: binary not found"))
            .contains("missing"));
        assert_eq!(
            humanize(&anyhow::anyhow!("no current project — /add or /clone first")),
            "no current project — /add or /clone first"
        );
    }

    #[test]
    fn new_clears_queued_job_session() {
        let (_dir, mut state) = primed();
        state.projects.get_mut("shop").unwrap().sessions.claude = Some("cla-9".into());
        let Submit::Started { job_id, .. } = submit(&mut state, "task", 1).unwrap() else {
            panic!("started");
        };
        assert_eq!(state.jobs[&job_id].session.as_deref(), Some("cla-9"));
        new_session(&mut state, None).unwrap();
        assert!(state.projects["shop"].sessions.claude.is_none());
        assert!(state.jobs[&job_id].session.is_none());
    }

    #[test]
    fn bad_cwd_is_human() {
        let (_dir, mut state) = primed();
        state.projects.get_mut("shop").unwrap().cwd = "nope".into();
        let err = submit(&mut state, "hello", 1).unwrap_err().to_string();
        assert!(err.contains("cwd"), "{err}");
        assert!(err.contains("/cwd"), "{err}");
    }

    fn git_init_commit(dir: &std::path::Path) {
        use std::process::Command;
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("README"), "shop").unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "raz")
                .env("GIT_AUTHOR_EMAIL", "raz@test")
                .env("GIT_COMMITTER_NAME", "raz")
                .env("GIT_COMMITTER_EMAIL", "raz@test")
                .status()
                .unwrap()
                .success());
        };
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "init"]);
    }

    #[test]
    fn two_read_jobs_share_main_checkout() {
        let (_dir, mut state) = primed();
        let Submit::Started { job_id: first, .. } = submit(&mut state, "read a", 1).unwrap() else {
            panic!("started");
        };
        let Submit::Started { job_id: second, .. } =
            submit_run(&mut state, Engine::Codex, "read b", 1).unwrap()
        else {
            panic!("started");
        };
        assert_ne!(first, second);
        assert!(state.jobs[&first].worktree.is_none());
        assert!(state.jobs[&second].worktree.is_none());
        assert_eq!(occupying(&state).len(), 2);
        let keys: std::collections::HashSet<_> = state
            .jobs
            .values()
            .filter_map(|job| job_checkout_key(&state, job))
            .collect();
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn writer_blocks_readers_on_same_path() {
        let (_dir, mut state) = primed();
        set_perm(&mut state, Some("write")).unwrap();
        git_init_commit(_dir.path().join("shop").as_path());
        let Submit::Started { job_id, .. } = submit(&mut state, "write", 1).unwrap() else {
            panic!("started");
        };
        assert_eq!(state.jobs[&job_id].perm, "write");
        state.projects.get_mut("shop").unwrap().perm = "read".into();
        let err = submit_run(&mut state, Engine::Codex, "read after", 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("writing"), "{err}");
        assert!(err.contains(&job_id), "{err}");
    }

    #[test]
    fn second_write_job_gets_a_worktree() {
        let (_dir, mut state) = primed();
        git_init_commit(_dir.path().join("shop").as_path());
        set_perm(&mut state, Some("write")).unwrap();
        let Submit::Started { job_id: first, .. } = submit(&mut state, "one", 1).unwrap() else {
            panic!("started");
        };
        assert!(state.jobs[&first].worktree.is_none());
        let Submit::Started { job_id: second, .. } =
            submit_run(&mut state, Engine::Claude, "two", 1).unwrap()
        else {
            panic!("started");
        };
        assert!(state.jobs[&second].worktree.is_some(), "expected worktree");
        assert_ne!(
            job_checkout_key(&state, &state.jobs[&first]),
            job_checkout_key(&state, &state.jobs[&second])
        );
        assert!(state.jobs[&second].session.is_none());
        assert_eq!(occupying(&state).len(), 2);
    }

    #[test]
    fn capacity_refuses_a_third_occupying_job() {
        let (_dir, mut state) = primed();
        projects::register(&mut state, "blog", "blog").unwrap();
        fs::create_dir_all(_dir.path().join("notes")).unwrap();
        projects::register(&mut state, "notes", "notes").unwrap();
        projects::switch(&mut state, "shop").unwrap();
        submit(&mut state, "shop task", 1).unwrap();
        projects::switch(&mut state, "blog").unwrap();
        submit(&mut state, "blog task", 1).unwrap();
        projects::switch(&mut state, "notes").unwrap();
        let err = submit(&mut state, "notes task", 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("capacity"), "{err}");
        assert_eq!(occupying(&state).len(), 2);
    }

    #[test]
    fn follow_up_still_works_at_capacity() {
        let (_dir, mut state) = primed();
        projects::register(&mut state, "blog", "blog").unwrap();
        projects::switch(&mut state, "shop").unwrap();
        let Submit::Started { job_id, .. } = submit(&mut state, "shop", 1).unwrap() else {
            panic!("started");
        };
        projects::switch(&mut state, "blog").unwrap();
        submit(&mut state, "blog", 1).unwrap();
        projects::switch(&mut state, "shop").unwrap();
        let queued = submit(&mut state, "follow shop", 1).unwrap();
        let Submit::Queued { job_id: qid, .. } = queued else {
            panic!("expected queued");
        };
        assert_eq!(qid, job_id);
        assert_eq!(occupying(&state).len(), 2);
    }

    #[test]
    fn both_starts_two_fresh_jobs() {
        let (_dir, mut state) = primed();
        state.projects.get_mut("shop").unwrap().sessions.claude = Some("cla-main".into());
        state.projects.get_mut("shop").unwrap().sessions.codex = Some("cod-main".into());
        let Submit::StartedMany { jobs } = submit_both(&mut state, "review", 1).unwrap() else {
            panic!("expected many");
        };
        assert_eq!(jobs.len(), 2);
        let engines: Vec<_> = jobs
            .iter()
            .map(|(id, _)| state.jobs[id].engine)
            .collect();
        assert!(engines.contains(&Engine::Claude));
        assert!(engines.contains(&Engine::Codex));
        for (id, _) in &jobs {
            assert!(state.jobs[id].session.is_none());
            assert!(state.jobs[id].worktree.is_none());
        }
        assert_eq!(
            state.projects["shop"].sessions.claude.as_deref(),
            Some("cla-main")
        );
    }

    #[test]
    fn new_by_id_clears_only_that_job() {
        let (_dir, mut state) = primed();
        state.projects.get_mut("shop").unwrap().sessions.claude = Some("cla-1".into());
        state.projects.get_mut("shop").unwrap().sessions.codex = Some("cod-1".into());
        let Submit::Started { job_id, .. } = submit(&mut state, "task", 1).unwrap() else {
            panic!("started");
        };
        stop(&mut state, Some(&job_id)).unwrap();
        assert_eq!(
            new_session(&mut state, Some(&job_id)).unwrap(),
            format!("new claude session on {job_id}")
        );
        assert!(state.jobs[&job_id].session.is_none());
        assert!(state.projects["shop"].sessions.claude.is_none());
        assert_eq!(
            state.projects["shop"].sessions.codex.as_deref(),
            Some("cod-1")
        );
    }

    #[test]
    fn jobs_lists_recently_finished() {
        let (_dir, mut state) = primed();
        let Submit::Started { job_id, .. } = submit(&mut state, "task", 1).unwrap() else {
            panic!("started");
        };
        state.jobs.get_mut(&job_id).unwrap().state = "done".into();
        apply_status(state.jobs.get_mut(&job_id).unwrap(), "done", 50, true);
        let block = jobs_block(&state);
        assert!(block.contains(&job_id), "{block}");
        assert!(block.contains("done"), "{block}");
        assert!(block.contains("claude"), "{block}");
    }

    #[test]
    fn worktree_outside_workspace_is_rejected() {
        let (_dir, mut state) = primed();
        let Submit::Started { job_id, .. } = submit(&mut state, "task", 1).unwrap() else {
            panic!("started");
        };
        state.jobs.get_mut(&job_id).unwrap().worktree = Some("/tmp/escape".into());
        let err = worktree_jail_ok(&state).unwrap_err().to_string();
        assert!(err.contains(&job_id), "{err}");
        assert!(err.contains("escapes"), "{err}");
    }

    #[test]
    fn reply_to_routes_to_that_job() {
        let (_dir, mut state) = primed();
        let Submit::Started { job_id: first, .. } = submit(&mut state, "one", 1).unwrap() else {
            panic!("started");
        };
        stop(&mut state, Some(&first)).unwrap();
        state.jobs.get_mut(&first).unwrap().telegram_status_id = Some(44);
        state.jobs.get_mut(&first).unwrap().session = Some("cla-keep".into());
        state.current.engine = Some(Engine::Codex);
        assert_eq!(find_job_by_status_id(&state, 44).as_deref(), Some(first.as_str()));
        let Submit::Started { job_id, .. } =
            follow_job(&mut state, &first, "continue", 1).unwrap()
        else {
            panic!("started");
        };
        assert_eq!(job_id, first);
        assert_eq!(state.jobs[&first].prompt, "continue");
        assert_eq!(state.jobs[&first].state, "queued");
        assert_eq!(state.jobs[&first].engine, Engine::Claude);
        assert_eq!(state.jobs[&first].session.as_deref(), Some("cla-keep"));
    }
}
