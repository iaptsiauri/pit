# Key Decisions

## tmux over PTY passthrough

**Date:** 2025-02  
**Status:** Accepted  

Claude Code opens `/dev/tty` directly in raw mode, bypassing any PTY we create.
pit v1 tried to embed a PTY-based terminal emulator — it failed because Claude's
input never came through our PTY. tmux intercepts at the kernel level (it owns the
PTY master), so F1 detach works even in Claude's raw mode.

**Tradeoff:** External dependency (tmux), but universally available.

## Dedicated tmux socket

**Date:** 2025-02  
**Status:** Accepted  

`tmux -L pit` isolates pit's sessions from the user's normal tmux server.
Prevents name collisions, makes cleanup easier, and avoids interfering
with user's tmux config.

## Agent command as tmux session process

**Date:** 2025-02  
**Status:** Accepted  

Previously: `create_session()` + `send_keys(cmd, Enter)` — this left a shell
running after the agent exited, so tasks stayed "running" forever.

Now: `create_session_with_cmd(cmd)` — the agent IS the session process. When it
exits, tmux destroys the session. The reaper detects this and marks the task idle.

## SQLite over filesystem state

**Date:** 2025-02  
**Status:** Accepted  

Task state in SQLite instead of individual files. Single source of truth,
atomic operations, easy queries. WAL mode gives concurrent reads.
The DB is disposable — it can be rebuilt from git branches.

## Idle as default post-exit status

**Date:** 2025-02  
**Status:** Accepted  

When an agent exits (or crashes), the task goes to "idle" not "done".
"Done" is reserved for explicit user completion. This way re-entering
a task with Enter automatically resumes the session.

## Multi-agent dispatch via build_agent_cmd()

**Date:** 2025-02  
**Status:** Accepted  

Single function that maps agent name → command string. Each agent has
different flag conventions (claude uses -p/-r, codex uses positional,
aider uses --message). Unknown agents fall back to claude.
