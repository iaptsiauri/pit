# pit

Run multiple coding agents in parallel with git worktree isolation.

```
┌─ pit ──────────────────────┬──────────────────────────────────────────┐
│                            │ ▸ fix-auth                               │
│  ▶ fix-auth       running  │   branch: pit/fix-auth · claude         │
│  ▶ add-tests      running  │   Fix the SSO login timeout bug         │
│  ○ refactor-api   idle     │                                         │
│  ✓ update-deps    done     │   Commits (3 vs main)                   │
│                            │   abc1234 fix timeout handling           │
│                            │   def5678 add retry logic                │
│                            │   ghi9012 update tests                   │
│                            │                                         │
│                            │   Changes                               │
│                            │   ▾ src/auth.rs          +42 -8         │
│                            │     @@ -15,6 +15,12 @@                  │
│                            │     +  let timeout = Duration::from_..   │
│                            │   ▸ src/tests/auth.rs    +18 -0         │
├────────────────────────────┴──────────────────────────────────────────┤
│ Enter:open  t:shell  b:bg  n:new  d:del  v:kanban  r:refresh  q:quit │
└───────────────────────────────────────────────────────────────────────┘
```

## Features

- **Parallel agents** — run Claude Code, Codex, Aider, Amp, or any custom command simultaneously
- **Git worktree isolation** — each task gets its own branch and working directory
- **Split-pane dashboard** — task list + rich detail with commits, diffs, inline hunks
- **Kanban board** — press `v` to toggle between list and kanban view
- **Linear issue picker** — `Ctrl+L` to search and select issues, auto-fills prompt
- **Session resume** — Claude sessions persist across detach/reattach
- **Shell access** — press `t` to open a terminal in any task's worktree
- **Config system** — `pit config set linear.api_key ...` for persistent API keys

## Install

```bash
# Homebrew (macOS)
brew install iaptsiauri/tap/pit

# Cargo (from source)
cargo install --git https://github.com/iaptsiauri/pit --tag v0.2.1

# Binary (Apple Silicon)
curl -L https://github.com/iaptsiauri/pit/releases/latest/download/pit-0.2.1-aarch64-apple-darwin.tar.gz | tar xz
sudo mv pit /usr/local/bin/
```

Requires: **tmux** (auto-installed by Homebrew formula)

## Quick Start

```bash
cd your-repo
pit                    # launch dashboard (auto-inits if needed)
```

Press `n` to create a task:

```
┌─ New Task ────────────────────────────────────────┐
│ ▸ Task name                                       │
│   eager-spark                                     │
│                                                   │
│   Agent prompt                                    │
│   Fix the login timeout bug and add tests         │
│                                                   │
│   Agent                                           │
│   ◂ claude ▸                                      │
│   [✓] Auto-approve — skip permission prompts      │
│ ──────────────────────────────────────────────── │
│   Issue                                           │
│   ✓ ENG-42 · Fix login timeout [In Progress]      │
└───────────────────────────────────────────────────┘
```

Press `Enter` to attach to a running agent. Press `F1` to detach back to the dashboard.

## Keybindings

### Dashboard (list view)

| Key | Action |
|-----|--------|
| `Enter` | Attach to task (launches agent if idle) |
| `t` | Open shell in task's worktree |
| `b` | Run task in background |
| `n` | New task modal |
| `d` | Delete task |
| `r` | Refresh |
| `v` | Toggle kanban view |
| `l` / `→` | Focus detail pane |
| `j` / `k` | Navigate tasks |
| `q` | Quit |

### Detail pane

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate files / diff lines |
| `Enter` | Expand/collapse file diff |
| `g` / `G` | Jump to top / bottom |
| `PageUp` / `PageDown` | Scroll |
| `h` / `←` | Back to task list |
| `Esc` | Layered escape (diff → file → pane) |

### Kanban view

| Key | Action |
|-----|--------|
| `←` / `→` | Move between columns |
| `↑` / `↓` | Select within column |
| `Enter` | Attach to task |
| `t` | Open shell |
| `v` | Toggle back to list |

### Inside agent (tmux)

| Key | Action |
|-----|--------|
| `F1` | Detach back to dashboard |
| `Ctrl+\` | Detach (alternative) |
| `Ctrl+]` | tmux prefix (for power users) |

## CLI

```bash
pit                          # TUI dashboard (auto-init)
pit init                     # Initialize pit in current repo
pit new <name> [-p prompt]   # Create task
pit list                     # List tasks (alias: pit ls)
pit status                   # Show status with live reaping
pit run <name>               # Run task in background
pit stop <name>              # Stop running task
pit shell <name>             # Open shell in worktree (alias: pit sh)
pit diff <name>              # Show diff vs main
pit delete <name>            # Delete task (alias: pit rm)
pit config set <key> <val>   # Set config value
pit config get <key>         # Get config value
pit config list              # List all config
pit config path              # Show config file path
```

## Configuration

```bash
# Linear integration (issue picker)
pit config set linear.api_key lin_api_...

# GitHub integration (private repo issues)
pit config set github.token ghp_...
```

Config stored at `~/Library/Application Support/pit/config.toml` (macOS)
or `~/.local/share/pit/config.toml` (Linux).

Environment variables take priority (e.g. `LINEAR_API_KEY`).

## Supported Agents

| Agent | Command | Resume |
|-------|---------|--------|
| Claude Code | `claude -p '<prompt>'` | `claude -r <session-id>` |
| Codex | `codex '<prompt>'` | — |
| Aider | `aider --message '<prompt>'` | — |
| Amp | `amp --prompt '<prompt>'` | — |
| Custom | raw command string | — |

## How It Works

1. `pit init` creates `.pit/` directory with SQLite database
2. `pit new` creates a git branch (`pit/<name>`) and worktree (`.pit/worktrees/<name>`)
3. Launching a task starts a tmux session with the agent as the session process
4. When the agent exits, tmux destroys the session → reaper marks task as idle
5. Re-entering with `Enter` resumes the Claude session (same `session-id`)
6. All agents run in a dedicated tmux server (`tmux -L pit`) — isolated from your normal tmux

## Contributing

See [AGENTS.md](AGENTS.md) for development guidelines and architecture overview.

```bash
cargo test              # run all tests
cargo clippy            # lint
cargo build             # debug build
cargo install --path .  # install locally
```

## License

MIT
