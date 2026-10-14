# Agent notes

pocket-code is a VPS supervisor. Telegram is the client. Claude and Codex own chat memory. This repo only stores routing state.

## Crate layout

- `src/models` — structs and enums (`State`, `Engine`, `Project`, `Job`, Telegram DTOs)
- `src/store` — `state.json` load/save, atomic writes, secret files
- `src/cmd` — `setup`, `auth`, `doctor`, `serve`, `up`
- `src/integrations` — Claude / Codex / GitHub / Telegram / systemd
- `src/util` — logging and the workspace path jail

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
