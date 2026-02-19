# Architecture

## Overview

pit is a single Rust binary with three layers:

```
┌─────────────────────────────────┐
│           TUI (ratatui)         │  User-facing dashboard
│   app.rs (state + keys)        │
│   ui.rs  (rendering)           │
├─────────────────────────────────┤
│           Core logic            │  Business rules, no UI
│   task, tmux, reap, git_info   │
│   config, linear, github       │
│   issues, names, project       │
├─────────────────────────────────┤
│           Storage               │  SQLite + git
│   db (migrations, WAL)         │
│   .pit/ directory              │
│   git worktrees + branches     │
└─────────────────────────────────┘
```

## Data flow

1. User runs `pit` → `main.rs` dispatches to TUI or CLI command
2. TUI creates `App` struct, opens DB, loads tasks
3. Key press → `handle_key()` → returns `Action`
4. Action dispatched: launch tmux, create task, delete, etc.
5. `reap::reap_dead()` runs on each tick: checks tmux sessions, updates status

## Storage

### SQLite (`.pit/pit.db`)

Single table `tasks` with columns:
- id, name, description, prompt, issue_url, agent
- branch, worktree (filesystem paths)
- status (idle/running/done), session_id, tmux_session, pid
- created_at, updated_at

WAL mode for concurrent reads. Migrations versioned in `db/migrations.rs`.

### Git

- Each task creates branch `pit/<name>` from current HEAD
- Worktree at `<repo>/.pit/worktrees/<name>`
- Branches and worktrees cleaned up on task delete

### Config

- `~/Library/Application Support/pit/config.toml` (macOS)
- `~/.local/share/pit/config.toml` (Linux)
- TOML sections: `[linear]`, `[github]`
- Env vars override config file (e.g. `LINEAR_API_KEY`)

## tmux

- Dedicated socket: `tmux -L pit`
- Custom config at data dir: F1=detach, Ctrl-\=detach, Ctrl-]=prefix
- Agent runs as session command → session dies when agent exits
- Shell sessions: `pit-shell-<name>` for `t` key / `pit shell`

## Views

### List view (default)
- 30/70 split: task list (left) + detail pane (right)
- Detail: header, commits vs main, file diff stats, inline expandable diffs
- Two-level navigation: file headers → diff lines

### Kanban view (`v` to toggle)
- Three columns: Idle | Running | Done
- Tasks auto-sorted by status
- Navigate: ←/→ between columns, ↑/↓ within

## Agent dispatch

`build_agent_cmd()` in `app.rs` dispatches per agent type:
- **claude**: `claude -p '<prompt>'` (first run), `claude -r <session-id>` (resume)
- **codex**: `codex '<prompt>'`
- **aider**: `aider --message '<prompt>'`
- **amp**: `amp --prompt '<prompt>'`
- **custom**: raw command string
- **unknown**: falls back to claude
