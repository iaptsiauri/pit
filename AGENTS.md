# pit — Agent Guidelines

## What is pit

A terminal-native orchestrator for running multiple coding agents in parallel.
Each task gets its own git worktree, branch, and tmux session.
Built in Rust with ratatui TUI, SQLite storage, tmux multiplexing.

## Quick reference

```bash
cargo test                 # run all tests (unit + integration)
cargo build                # debug build
cargo install --path .     # install to ~/.cargo/bin/pit
cargo clippy               # lint
```

## Architecture

See [docs/architecture.md](docs/architecture.md) for the full map.

```
src/
  main.rs              CLI entry point (clap). Dispatches to commands or TUI.
  db/
    mod.rs             SQLite open + WAL mode
    migrations.rs      Versioned schema migrations (3 so far)
  core/
    project.rs         .pit/ directory management, repo root detection
    task.rs            Task CRUD, git worktree+branch lifecycle
    tmux.rs            Dedicated tmux socket, session management, custom config
    reap.rs            Reap dead tmux sessions → mark tasks idle
    git_info.rs        Commits, diff stats, file diffs for detail pane
    config.rs          Persistent config (~/.../pit/config.toml), env var fallback
    linear.rs          Linear GraphQL API: fetch, search, my_issues
    github.rs          GitHub REST API: fetch issues
    issues.rs          Unified issue dispatcher (Linear/GitHub)
    names.rs           Auto-generated task names (adjective-noun)
  tui/
    app.rs             App state, all key handlers, actions, modal, kanban
    ui.rs              Ratatui rendering: list view, detail pane, kanban, modal, picker
    mod.rs             Exports
tests/
  cli.rs               Integration tests for all CLI commands
```

## Conventions

- **No `unwrap()` in production code** — use `anyhow::Result` and `?` everywhere
- **Tests next to code** — `#[cfg(test)] mod tests` at bottom of each file
- **Integration tests** in `tests/cli.rs` — use `assert_cmd` + `tempfile`
- **One concern per file** — `task.rs` does task CRUD, `tmux.rs` does tmux, etc.
- **Snake_case** for files and functions, **PascalCase** for types
- **Config via `core::config::get()`** — never read env vars directly in feature code
- **Status flow**: idle → running → idle (agent exited) or idle (manual stop). Done is explicit.

## Key decisions

See [docs/decisions.md](docs/decisions.md) for rationale on major choices.

- **tmux over PTY**: Claude Code opens /dev/tty directly, defeating PTY passthrough
- **Dedicated tmux socket** (`tmux -L pit`): isolates from user's normal sessions
- **F1 for detach**: bound in tmux root table — works even with Claude's raw mode
- **Session resume**: `claude -r <session-id>` stored in DB, reused on re-entry
- **Agent command as tmux session process**: when agent exits, session dies, reaper marks idle
- **SQLite with WAL**: fast concurrent reads, single-writer is fine for our use case

## Adding a new feature

1. Add core logic in `src/core/<module>.rs` with unit tests
2. Wire into CLI in `src/main.rs` (add clap subcommand + handler)
3. Wire into TUI in `src/tui/app.rs` (add Action variant + key handler)
4. Update rendering in `src/tui/ui.rs` if needed
5. Add integration test in `tests/cli.rs`
6. Update this file and docs if the feature changes architecture

## Known debt

See [docs/quality.md](docs/quality.md) for tracked issues.
