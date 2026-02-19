use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

use super::checkpoint;
use super::task::{self, Status};
use super::tmux;

/// Check all "running" tasks and mark as "idle" if their tmux session is gone.
/// Auto-checkpoints when an agent exits with new commits.
/// Returns the number of tasks reaped.
pub fn reap_dead(db: &Connection, repo_root: &Path) -> Result<usize> {
    let tasks = task::list(db)?;
    let mut reaped = 0;

    for t in &tasks {
        if t.status != Status::Running {
            continue;
        }

        let is_alive = match &t.tmux_session {
            Some(name) => tmux::session_exists(name),
            None => false,
        };

        if !is_alive {
            // Auto-checkpoint if the agent made new commits
            let worktree = Path::new(&t.worktree);
            if checkpoint::has_new_commits(repo_root, &t.name, &t.branch) {
                let _ = checkpoint::create(repo_root, &t.name, &t.branch, worktree);
            }

            task::set_status(db, t.id, &Status::Idle)?;
            reaped += 1;
        }
    }

    Ok(reaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_root() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp")
    }

    #[test]
    fn reap_marks_orphaned_tasks_as_idle() {
        let db = crate::db::open_memory().unwrap();

        db.execute(
            "INSERT INTO tasks (name, branch, worktree, status, tmux_session)
             VALUES ('orphan', 'pit/orphan', '/tmp/wt', 'running', 'pit-orphan-nonexistent')",
            [],
        )
        .unwrap();

        let reaped = reap_dead(&db, &dummy_root()).unwrap();
        assert_eq!(reaped, 1);

        let t = task::get(&db, 1).unwrap().unwrap();
        assert_eq!(t.status, Status::Idle);
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

        let reaped = reap_dead(&db, &dummy_root()).unwrap();
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

        let reaped = reap_dead(&db, &dummy_root()).unwrap();
        assert_eq!(reaped, 0);
    }

    #[test]
    fn reap_running_with_no_tmux_session() {
        let db = crate::db::open_memory().unwrap();

        db.execute(
            "INSERT INTO tasks (name, branch, worktree, status, tmux_session)
             VALUES ('ghost', 'pit/ghost', '/tmp/wt', 'running', NULL)",
            [],
        )
        .unwrap();

        let reaped = reap_dead(&db, &dummy_root()).unwrap();
        assert_eq!(reaped, 1);

        let t = task::get(&db, 1).unwrap().unwrap();
        assert_eq!(t.status, Status::Idle);
    }
}
