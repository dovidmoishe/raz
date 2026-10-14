use std::process::ExitCode;

use anyhow::Result;

use crate::integrations::{detect, engines};
use crate::models::{Paths, State};
use crate::store;

struct Check {
    name: &'static str,
    ok: bool,
    required: bool,
    detail: String,
}

pub fn run() -> Result<ExitCode> {
    let paths = Paths::resolve()?;
    let tools = detect::probe();
    let mut checks = Vec::new();

    let state = if paths.state.exists() {
        match State::load(&paths.state) {
            Ok(s) => {
                checks.push(ok("state", true, format!("{}", paths.state.display())));
                Some(s)
            }
            Err(err) => {
                checks.push(ok("state", false, err.to_string()));
                None
            }
        }
    } else {
        checks.push(ok(
            "state",
            false,
            format!("missing {} — run pocket-code setup", paths.state.display()),
        ));
        None
    };

    checks.push(Check {
        name: "state-mode",
        ok: paths.state.exists() && !store::is_world_readable(&paths.state).unwrap_or(true),
        required: cfg!(unix),
        detail: if cfg!(unix) {
            "want 0600".into()
        } else {
            "n/a on this OS".into()
        },
    });

    let workspace = state
        .as_ref()
        .map(|s| std::path::PathBuf::from(&s.workspace_root))
        .unwrap_or_else(|| paths.workspace.clone());
    checks.push(ok(
        "workspace",
        workspace.is_dir(),
        workspace.display().to_string(),
    ));

    checks.push(bin("claude", &tools));
    checks.push(bin("codex", &tools));
    checks.push(bin_required("gh", &tools, false));
    checks.push(bin_required("git", &tools, true));

    let mut claude_ready = false;
    let mut codex_ready = false;
    if tools.claude.is_some() {
        claude_ready = engines::claude_logged_in();
        checks.push(ok(
            "claude-login",
            claude_ready,
            if claude_ready {
                "claude auth status ok"
            } else {
                "run pocket-code auth claude"
            }
            .into(),
        ));
    }
    if tools.codex.is_some() {
        codex_ready = engines::codex_logged_in();
        checks.push(ok(
            "codex-login",
            codex_ready,
            if codex_ready {
                "codex login status ok"
            } else {
                "run pocket-code auth codex"
            }
            .into(),
        ));
    }
    checks.push(ok(
        "engine",
        claude_ready || codex_ready,
        "need at least one authenticated engine".into(),
    ));

    if tools.gh.is_some() {
        let ready = engines::gh_logged_in();
        checks.push(Check {
            name: "github-login",
            ok: ready,
            required: false,
            detail: if ready {
                engines::gh_user().unwrap_or_else(|| "gh auth status ok".into())
            } else {
                "run pocket-code auth github".into()
            },
        });
    }

    let token = paths.telegram_token()?;
    checks.push(ok(
        "telegram-token",
        token.is_some(),
        if token.is_some() {
            paths.token.display().to_string()
        } else {
            "set POCKET_CODE_TELEGRAM_TOKEN or run setup".into()
        },
    ));
    if token.is_some() && paths.token.exists() {
        checks.push(Check {
            name: "token-mode",
            ok: !store::is_world_readable(&paths.token).unwrap_or(true),
            required: cfg!(unix),
            detail: "want 0600".into(),
        });
    }

    let paired = state.as_ref().map(|s| s.telegram.paired).unwrap_or(false);
    checks.push(Check {
        name: "telegram-pair",
        ok: paired,
        required: false,
        detail: if paired {
            state
                .as_ref()
                .map(|s| format!("user {:?}", s.telegram.allowed_user_ids))
                .unwrap_or_default()
        } else {
            "send /start <code> to the bot".into()
        },
    });

    let mut failed = 0;
    for check in &checks {
        let mark = if check.ok {
            "ok "
        } else if check.required {
            "ERR"
        } else {
            "wrn"
        };
        println!("{mark}  {:16}  {}", check.name, check.detail);
        if !check.ok && check.required {
            failed += 1;
        }
    }

    if failed > 0 {
        println!("\ndoctor failed ({failed} required)");
        Ok(ExitCode::from(1))
    } else {
        println!("\ndoctor ok");
        Ok(ExitCode::SUCCESS)
    }
}

fn ok(name: &'static str, pass: bool, detail: String) -> Check {
    Check {
        name,
        ok: pass,
        required: true,
        detail,
    }
}

fn bin(name: &'static str, tools: &crate::models::Tools) -> Check {
    bin_required(name, tools, false)
}

fn bin_required(name: &'static str, tools: &crate::models::Tools, required: bool) -> Check {
    let present = tools.has(name);
    Check {
        name,
        ok: present,
        required,
        detail: if present {
            "on PATH".into()
        } else {
            format!("missing — {}", detect::install_hint(name))
        },
    }
}
