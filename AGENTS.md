# Agent notes

raz is a harness. Telegram is the client. Claude and Codex own chat memory. This repo only stores routing state.

## Crate layout

- `src/models` — structs and enums (`State`, `Engine`, `Project`, `Job`, Telegram DTOs)
- `src/store` — `state.json` load/save, atomic writes, secret files
- `src/cmd` — `setup`, `auth`, `doctor`, `serve`, `up`, `project`
- `src/projects` — workspace catalog (register, clone, switch, cwd, status)
- `src/turns` — job queue, `/use` `/new` `/stop` `/perm`, one writer per checkout, engine spawn
- `src/integrations` — Claude / Codex / GitHub (`gh` login + child probe) / Telegram / systemd
- `src/util` — logging, workspace path jail, rate limit, unix identity

Keep new types in `models`. Keep CLI verbs in `cmd`. Do not dump structs back into `main.rs`.

## Never commit internal build documents

Do **not** stage or commit internal build artifacts:

- `build-board.html` and any other local planning HTML
- scratch or phase-tracking markdown written for the build
- notes, dumps, or checklists that exist only to drive this project’s construction

Those files stay on the machine. `.gitignore` already excludes the board. If you create another planning `.md` or `.html` for the build, add it to `.gitignore` instead of committing it.

Product files that **may** be committed: `AGENTS.md` and source.

## Update the build board

After you finish work, open `build-board.html` locally and check off the matching tasks. The board is gitignored; do not try to commit checkbox state.

## Operations

Run raz as a dedicated Linux user. Do not run as root. The workspace (`workspace_root`, default `~/.raz/projects`) must be owned by that user. Enable the user unit with `systemctl --user enable --now raz`.

`raz serve` long-polls Telegram outbound only. There is no inbound bind and no agent port. If a health file is ever added, it stays on localhost.

Backup these paths (same user on restore, then `raz doctor`):

- `~/.raz` — `state.json`, `telegram.token`, `turns.jsonl`, `projects/`
- `~/.claude` — Claude credentials and chat memory
- `~/.codex` — Codex credentials and chat memory
- `~/.config/gh` — GitHub CLI login (agents inherit this)
