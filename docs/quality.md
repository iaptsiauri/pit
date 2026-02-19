# Quality Tracker

## Current state

| Area | Grade | Notes |
|------|-------|-------|
| Core (task, tmux, reap) | A | Well tested, clean separation |
| DB/migrations | A | 3 migrations, WAL mode, works |
| TUI list view | A | Split pane, two-level diff nav |
| TUI kanban | B | Functional, no scroll on overflow |
| Modal / task creation | A | Issue picker, auto-approve, agents |
| Linear integration | B | Works, but no error retry |
| GitHub integration | B | Works for public repos, token optional |
| Config system | B | Works, but config tests are flaky (global state) |
| CLI commands | A | All tested, good error messages |
| README | A | Comprehensive — install, keybindings, CLI, agent table |
| Test reliability | A- | 185 tests, 0 flakes (former config race fixed) |

## Known debt

### High priority
- [ ] No clippy/fmt in CI — run locally with `cargo clippy && cargo fmt --check`
- [ ] Kanban doesn't scroll when column has more tasks than screen height
- [ ] No `pit pr` command to create PRs from task branches
- [ ] No log capture — can't see what agents did after they exit

### Medium priority
- [ ] `build_agent_cmd()` lives in `tui/app.rs` but is used by `main.rs` too — should be in core
- [ ] `cargo test --test cli` deprecated warnings from `assert_cmd::Command::cargo_bin` — migrate to `cargo::cargo_bin_cmd!`
- [ ] `G_selects_last_file` test uses non-snake-case name (1 warning in test profile)

### Low priority
- [ ] `site/index.html` exists — 835-line landing page, not linked from anywhere
- [ ] No crates.io publish token configured

### Recently fixed
- [x] README rewritten with full feature coverage, keybindings, install instructions
- [x] All clippy warnings resolved (0 warnings in main build)
- [x] `cargo fmt` applied across all source files
- [x] Flaky `config_list_empty` test fixed (renamed `config_list_succeeds`, no race)
- [x] Dead code removed: `try_fetch_issue()`, `active_text()`, `issue_fetched_url`
