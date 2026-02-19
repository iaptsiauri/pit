use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::fmt;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Running,
    Done,
    Error,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Idle => write!(f, "idle"),
            Status::Running => write!(f, "running"),
            Status::Done => write!(f, "done"),
            Status::Error => write!(f, "error"),
        }
    }
}

impl Status {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "idle" => Ok(Status::Idle),
            "running" => Ok(Status::Running),
            "done" => Ok(Status::Done),
            "error" => Ok(Status::Error),
            _ => bail!("invalid status: {}", s),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub prompt: String,
    pub issue_url: String,
    pub agent: String,
    pub branch: String,
    pub worktree: String,
    pub status: Status,
    pub session_id: Option<String>,
    pub tmux_session: Option<String>,
    pub pid: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Options for creating a new task.
#[derive(Debug, Default)]
pub struct CreateOpts<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub prompt: &'a str,
    pub issue_url: &'a str,
    pub agent: &'a str,
}

/// Create a new task: git branch + worktree + DB row.
pub fn create(
    db: &Connection,
    repo_root: &Path,
    opts: &CreateOpts,
) -> Result<Task> {
    let name = opts.name;
    let description = opts.description;
    // Validate name: no spaces, no slashes, reasonable length
    if name.is_empty() {
        bail!("task name cannot be empty");
    }
    if name.len() > 100 {
        bail!("task name too long (max 100 chars)");
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        bail!("task name can only contain alphanumeric characters, hyphens, and underscores");
    }

    let branch = format!("pit/{}", name);
    let worktree_path = repo_root.join(".pit").join("worktrees").join(name);
    let worktree_str = worktree_path
        .to_str()
        .context("worktree path is not valid UTF-8")?
        .to_string();

    // Create the git branch (from current HEAD)
    let output = Command::new("git")
        .args(["branch", &branch])
        .current_dir(repo_root)
        .output()
        .context("failed to run git branch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already exists") {
            bail!("branch '{}' already exists", branch);
        }
        bail!("git branch failed: {}", stderr.trim());
    }

    // Create the git worktree
    let output = Command::new("git")
        .args(["worktree", "add", &worktree_str, &branch])
        .current_dir(repo_root)
        .output()
        .context("failed to run git worktree add")?;

    if !output.status.success() {
        // Clean up the branch we just created
        let _ = Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(repo_root)
            .output();
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git worktree add failed: {}", stderr.trim());
    }

    let agent = if opts.agent.is_empty() { "claude" } else { opts.agent };

    // Insert into database
    db.execute(
        "INSERT INTO tasks (name, description, prompt, issue_url, agent, branch, worktree)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![name, description, opts.prompt, opts.issue_url, agent, branch, worktree_str],
    )
    .with_context(|| format!("failed to insert task '{}'", name))?;

    let id = db.last_insert_rowid();
    get(db, id)?.context("task disappeared after insert")
}

/// List all tasks, ordered by creation time.
pub fn list(db: &Connection) -> Result<Vec<Task>> {
    let mut stmt = db.prepare(
        "SELECT id, name, description, prompt, issue_url, agent, branch, worktree, status,
                session_id, tmux_session, pid, created_at, updated_at
         FROM tasks ORDER BY created_at ASC",
    )?;

    let tasks = stmt
        .query_map([], row_to_task)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get a task by ID.
pub fn get(db: &Connection, id: i64) -> Result<Option<Task>> {
    let mut stmt = db.prepare(
        "SELECT id, name, description, prompt, issue_url, agent, branch, worktree, status,
                session_id, tmux_session, pid, created_at, updated_at
         FROM tasks WHERE id = ?1",
    )?;

    let task = stmt.query_row(params![id], row_to_task).optional()?;
    Ok(task)
}

/// Get a task by name.
pub fn get_by_name(db: &Connection, name: &str) -> Result<Option<Task>> {
    let mut stmt = db.prepare(
        "SELECT id, name, description, prompt, issue_url, agent, branch, worktree, status,
                session_id, tmux_session, pid, created_at, updated_at
         FROM tasks WHERE name = ?1",
    )?;

    let task = stmt.query_row(params![name], row_to_task).optional()?;
    Ok(task)
}

/// Delete a task: remove worktree, branch, and DB row.
pub fn delete(db: &Connection, repo_root: &Path, id: i64) -> Result<()> {
    let task = get(db, id)?.context("task not found")?;

    if task.status == Status::Running {
        bail!("cannot delete a running task — stop it first");
    }

    // Remove the git worktree
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", &task.worktree])
        .current_dir(repo_root)
        .output();

    // Delete the git branch
    let _ = Command::new("git")
        .args(["branch", "-D", &task.branch])
        .current_dir(repo_root)
        .output();

    // Remove the DB row
    db.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;

    Ok(())
}

/// Update task status.
pub fn set_status(db: &Connection, id: i64, status: &Status) -> Result<()> {
    let rows = db.execute(
        "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![status.to_string(), id],
    )?;
    if rows == 0 {
        bail!("task {} not found", id);
    }
    Ok(())
}

/// Store the tmux session name and PID for a running task.
pub fn set_running(
    db: &Connection,
    id: i64,
    tmux_session: &str,
    pid: Option<i64>,
    session_id: Option<&str>,
) -> Result<()> {
    db.execute(
        "UPDATE tasks SET status = 'running', tmux_session = ?1, pid = ?2,
         session_id = ?3, updated_at = datetime('now')
         WHERE id = ?4",
        params![tmux_session, pid, session_id, id],
    )?;
    Ok(())
}

/// Use rusqlite's optional extension.
use rusqlite::OptionalExtension;

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    let status_str: String = row.get(8)?;
    let status = Status::from_str(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        )
    })?;

    Ok(Task {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        prompt: row.get(3)?,
        issue_url: row.get(4)?,
        agent: row.get(5)?,
        branch: row.get(6)?,
        worktree: row.get(7)?,
        status,
        session_id: row.get(9)?,
        tmux_session: row.get(10)?,
        pid: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    fn make_git_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn setup() -> (TempDir, Connection) {
        let repo = make_git_repo();
        let db = crate::db::open_memory().unwrap();
        (repo, db)
    }

    fn opts<'a>(name: &'a str, desc: &'a str) -> CreateOpts<'a> {
        CreateOpts {
            name,
            description: desc,
            ..Default::default()
        }
    }

    #[test]
    fn create_and_get() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &CreateOpts {
            name: "fix-bug",
            description: "Fix the login bug",
            prompt: "find and fix the login timeout",
            issue_url: "https://linear.app/123",
            agent: "claude",
        }).unwrap();

        assert_eq!(task.name, "fix-bug");
        assert_eq!(task.description, "Fix the login bug");
        assert_eq!(task.prompt, "find and fix the login timeout");
        assert_eq!(task.issue_url, "https://linear.app/123");
        assert_eq!(task.agent, "claude");
        assert_eq!(task.branch, "pit/fix-bug");
        assert_eq!(task.status, Status::Idle);
        assert!(task.worktree.contains("fix-bug"));
        assert!(Path::new(&task.worktree).exists());

        let output = StdCommand::new("git")
            .args(["branch", "--list", "pit/fix-bug"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("pit/fix-bug"));
    }

    #[test]
    fn create_with_agent() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &CreateOpts {
            name: "codex-task",
            description: "",
            prompt: "",
            issue_url: "",
            agent: "codex",
        }).unwrap();
        assert_eq!(task.agent, "codex");

        // Re-read from DB to confirm persistence
        let t = get(&db, task.id).unwrap().unwrap();
        assert_eq!(t.agent, "codex");
    }

    #[test]
    fn create_empty_agent_defaults_to_claude() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &CreateOpts {
            name: "default-agent",
            description: "",
            prompt: "",
            issue_url: "",
            agent: "",
        }).unwrap();
        assert_eq!(task.agent, "claude");
    }

    #[test]
    fn create_with_custom_agent() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &CreateOpts {
            name: "amp-task",
            description: "",
            prompt: "do the thing",
            issue_url: "",
            agent: "amp",
        }).unwrap();
        assert_eq!(task.agent, "amp");
        assert_eq!(task.prompt, "do the thing");
    }

    #[test]
    fn create_rejects_empty_name() {
        let (repo, db) = setup();
        let result = create(&db, repo.path(), &opts("", "desc"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn create_rejects_invalid_chars() {
        let (repo, db) = setup();
        let result = create(&db, repo.path(), &opts("has spaces", "desc"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("alphanumeric"));
    }

    #[test]
    fn create_rejects_duplicate_name() {
        let (repo, db) = setup();
        create(&db, repo.path(), &opts("task-1", "")).unwrap();
        let result = create(&db, repo.path(), &opts("task-1", ""));
        assert!(result.is_err());
    }

    #[test]
    fn list_returns_all_tasks() {
        let (repo, db) = setup();
        create(&db, repo.path(), &opts("task-a", "")).unwrap();
        create(&db, repo.path(), &opts("task-b", "")).unwrap();
        create(&db, repo.path(), &opts("task-c", "")).unwrap();

        let tasks = list(&db).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].name, "task-a");
        assert_eq!(tasks[1].name, "task-b");
        assert_eq!(tasks[2].name, "task-c");
    }

    #[test]
    fn list_empty() {
        let (_repo, db) = setup();
        let tasks = list(&db).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn get_by_name_works() {
        let (repo, db) = setup();
        create(&db, repo.path(), &opts("my-task", "hello")).unwrap();

        let task = get_by_name(&db, "my-task").unwrap().unwrap();
        assert_eq!(task.name, "my-task");
        assert_eq!(task.description, "hello");

        let missing = get_by_name(&db, "nope").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn delete_removes_everything() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &opts("doomed", "")).unwrap();
        let worktree_path = task.worktree.clone();
        let task_id = task.id;

        delete(&db, repo.path(), task_id).unwrap();

        assert!(get(&db, task_id).unwrap().is_none());
        assert!(!Path::new(&worktree_path).exists());

        let output = StdCommand::new("git")
            .args(["branch", "--list", "pit/doomed"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());
    }

    #[test]
    fn delete_rejects_running_task() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &opts("busy", "")).unwrap();
        set_status(&db, task.id, &Status::Running).unwrap();

        let result = delete(&db, repo.path(), task.id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("running"));
    }

    #[test]
    fn set_status_works() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &opts("flip", "")).unwrap();

        set_status(&db, task.id, &Status::Running).unwrap();
        assert_eq!(get(&db, task.id).unwrap().unwrap().status, Status::Running);

        set_status(&db, task.id, &Status::Done).unwrap();
        assert_eq!(get(&db, task.id).unwrap().unwrap().status, Status::Done);
    }

    #[test]
    fn set_running_stores_tmux_info() {
        let (repo, db) = setup();
        let task = create(&db, repo.path(), &opts("runner", "")).unwrap();

        set_running(&db, task.id, "pit-runner", Some(12345), Some("sess-abc")).unwrap();

        let t = get(&db, task.id).unwrap().unwrap();
        assert_eq!(t.status, Status::Running);
        assert_eq!(t.tmux_session.as_deref(), Some("pit-runner"));
        assert_eq!(t.pid, Some(12345));
        assert_eq!(t.session_id.as_deref(), Some("sess-abc"));
    }
}
