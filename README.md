# pit

Run multiple coding agents in parallel with git worktree isolation.

```
┌─ pit ────────────────────────────────────┐
│                                          │
│  ▶ fix-login-bug    running              │
│  ▶ add-pagination   running              │
│  ○ refactor-api     idle                 │
│                                          │
│  Enter:open  b:background  d:delete  q:quit │
└──────────────────────────────────────────┘
```

**pit** is a terminal-native orchestrator for coding agents (Claude Code, etc.). Each task gets its own git worktree and branch. Agents run in tmux sessions — switch between them without killing running agents.

## Install

```bash
cargo install --path .
```

## Quick Start

```bash
# Initialize pit in your repo
pit init

# Create tasks
pit new fix-login-bug -d "Fix the login timeout issue"
pit new add-pagination -d "Add cursor pagination to API"

# Launch the dashboard
pit
```

### Dashboard Keys

| Key | Action |
|-----|--------|
| `Enter` | Open task in tmux (creates session + launches Claude Code) |
| `b` | Run task in background (detached tmux) |
| `d` | Delete task (removes worktree + branch) |
| `j/k` or `↑/↓` | Navigate |
| `r` | Refresh |
| `q` | Quit |

### Switching Tasks

1. Press `Enter` on a task → you're inside Claude Code
2. Press `Ctrl-b d` → detach from tmux, back to pit dashboard
3. Press `Enter` on another task → Claude Code in a different worktree
4. Both agents keep running simultaneously

## How It Works

- **Git worktrees**: Each task gets an isolated working directory on its own branch
- **tmux sessions**: Agents run inside tmux, so they persist when you switch away
- **SQLite**: Task state is tracked in `.pit/pit.db`
- **Claude Code sessions**: Session IDs are stored and reused with `--session-id` / `-r` for resume

## Requirements

- Git
- tmux (`brew install tmux` on macOS)
- A coding agent CLI (claude, etc.)

## CLI Commands

```bash
pit              # Launch TUI dashboard
pit init         # Initialize pit in current git repo
pit new <name>   # Create a new task
pit list         # List all tasks (alias: pit ls)
pit delete <name> # Delete a task (alias: pit rm)
```

## License

MIT
