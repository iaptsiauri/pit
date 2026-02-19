use anyhow::Result;
use rusqlite::Connection;

use super::task::{self, Status};
use super::tmux;

/// Check all "running" tasks and mark as "done" if their tmux session is gone.
/// Returns the number of tasks reaped.
pub fn reap_dead(db: &Connection) -> Result<usize> {
    let tasks = task::list(db)?;
    let mut reaped = 0;

    for t in &tasks {
        if t.status != Status::Running {
            continue;
        }

        let is_alive = match &t.tmux_session {
            Some(name) => tmux::session_exists(name),
            None => false, // No tmux session recorded — can't be running
        };

        if !is_alive {
            task::set_status(db, t.id, &Status::Done)?;
            reaped += 1;
        }
    }

    Ok(reaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reap_marks_orphaned_tasks_as_done() {
        let db = crate::db::open_memory().unwrap();

        // Insert a task that claims to be running with a tmux session that doesn't exist
        db.execute(
            "INSERT INTO tasks (name, branch, worktree, status, tmux_session)
             VALUES ('orphan', 'pit/orphan', '/tmp/wt', 'running', 'pit-orphan-nonexistent')",
            [],
        )
        .unwrap();

        let reaped = reap_dead(&db).unwrap();
        assert_eq!(reaped, 1);

        let t = task::get(&db, 1).unwrap().unwrap();
        assert_eq!(t.status, Status::Done);
    }

    #[test]
    fn reap_ignores_idle_tasks() {
        let db = crate::db::open_memory().unwrap();

        db.execute(
            "INSERT INTO tasks (name, branch, worktree, status)
             VALUES ('idle-task', 'pit/idle', '/tmp/wt', 'idle')",
            [],
        )
        .unwrap();

        let reaped = reap_dead(&db).unwrap();
        assert_eq!(reaped, 0);

        let t = task::get(&db, 1).unwrap().unwrap();
        assert_eq!(t.status, Status::Idle);
    }

    #[test]
    fn reap_ignores_done_tasks() {
        let db = crate::db::open_memory().unwrap();

        db.execute(
            "INSERT INTO tasks (name, branch, worktree, status)
             VALUES ('done-task', 'pit/done', '/tmp/wt', 'done')",
            [],
        )
        .unwrap();

        let reaped = reap_dead(&db).unwrap();
        assert_eq!(reaped, 0);
    }

    #[test]
    fn reap_running_with_no_tmux_session() {
        let db = crate::db::open_memory().unwrap();

        // Running but no tmux_session recorded — should be reaped
        db.execute(
            "INSERT INTO tasks (name, branch, worktree, status, tmux_session)
             VALUES ('ghost', 'pit/ghost', '/tmp/wt', 'running', NULL)",
            [],
        )
        .unwrap();

        let reaped = reap_dead(&db).unwrap();
        assert_eq!(reaped, 1);

        let t = task::get(&db, 1).unwrap().unwrap();
        assert_eq!(t.status, Status::Done);
    }
}
