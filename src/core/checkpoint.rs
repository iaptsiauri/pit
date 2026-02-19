use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// A checkpoint is a lightweight git tag at a specific commit in a task's branch.
/// Format: pit/checkpoint/<task-name>/<N>
///
/// Checkpoints let you save known-good states and rollback if an agent goes off the rails.

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub index: usize,
    pub tag: String,
    pub commit_hash: String,
    pub message: String,
    pub timestamp: String,
}

/// Create a checkpoint for a task. Returns the checkpoint index.
pub fn create(repo_root: &Path, task_name: &str, branch: &str) -> Result<usize> {
    // Find the next checkpoint index
    let existing = list(repo_root, task_name)?;
    let next_idx = existing.last().map(|c| c.index + 1).unwrap_or(1);

    let tag = format!("pit/checkpoint/{}/{}", task_name, next_idx);

    // Get the current HEAD of the branch
    let output = Command::new("git")
        .args(["rev-parse", branch])
        .current_dir(repo_root)
        .output()
        .context("failed to run git rev-parse")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to resolve branch '{}': {}", branch, stderr.trim());
    }

    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Create the tag
    let output = Command::new("git")
        .args(["tag", &tag, &commit])
        .current_dir(repo_root)
        .output()
        .context("failed to create git tag")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git tag failed: {}", stderr.trim());
    }

    Ok(next_idx)
}

/// List all checkpoints for a task, sorted by index.
pub fn list(repo_root: &Path, task_name: &str) -> Result<Vec<Checkpoint>> {
    let prefix = format!("pit/checkpoint/{}/", task_name);

    let output = Command::new("git")
        .args(["tag", "--list", &format!("{}*", prefix)])
        .current_dir(repo_root)
        .output()
        .context("failed to list git tags")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut checkpoints: Vec<Checkpoint> = Vec::new();

    for tag_name in stdout.lines().filter(|l| !l.is_empty()) {
        let idx_str = tag_name.strip_prefix(&prefix).unwrap_or("0");
        let index: usize = idx_str.parse().unwrap_or(0);

        // Get commit info for this tag
        let output = Command::new("git")
            .args(["log", "-1", "--format=%h|%s|%cr", tag_name])
            .current_dir(repo_root)
            .output();

        let (commit_hash, message, timestamp) = match output {
            Ok(o) if o.status.success() => {
                let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let parts: Vec<&str> = line.splitn(3, '|').collect();
                (
                    parts.first().unwrap_or(&"").to_string(),
                    parts.get(1).unwrap_or(&"").to_string(),
                    parts.get(2).unwrap_or(&"").to_string(),
                )
            }
            _ => (String::new(), String::new(), String::new()),
        };

        checkpoints.push(Checkpoint {
            index,
            tag: tag_name.to_string(),
            commit_hash,
            message,
            timestamp,
        });
    }

    checkpoints.sort_by_key(|c| c.index);
    Ok(checkpoints)
}

/// Rollback a task's worktree to the last checkpoint (or a specific index).
pub fn rollback(
    repo_root: &Path,
    task_name: &str,
    worktree: &Path,
    target: Option<usize>,
) -> Result<usize> {
    let checkpoints = list(repo_root, task_name)?;
    if checkpoints.is_empty() {
        bail!("no checkpoints for task '{}'", task_name);
    }

    let checkpoint = match target {
        Some(idx) => checkpoints
            .iter()
            .find(|c| c.index == idx)
            .ok_or_else(|| anyhow::anyhow!("checkpoint {} not found", idx))?,
        None => checkpoints.last().unwrap(),
    };

    // Reset the worktree to the checkpoint commit
    let output = Command::new("git")
        .args(["reset", "--hard", &checkpoint.tag])
        .current_dir(worktree)
        .output()
        .context("failed to git reset")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git reset failed: {}", stderr.trim());
    }

    Ok(checkpoint.index)
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
        // Initial commit
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Create a branch
        StdCommand::new("git")
            .args(["branch", "pit/test-task"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn add_commit(dir: &Path, branch: &str, msg: &str) {
        StdCommand::new("git")
            .args(["checkout", branch])
            .current_dir(dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", msg])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn create_and_list_checkpoint() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "first change");

        let idx = create(repo.path(), "test-task", "pit/test-task").unwrap();
        assert_eq!(idx, 1);

        let checkpoints = list(repo.path(), "test-task").unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].index, 1);
        assert_eq!(checkpoints[0].message, "first change");
    }

    #[test]
    fn multiple_checkpoints_increment() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "change 1");
        let idx1 = create(repo.path(), "test-task", "pit/test-task").unwrap();
        assert_eq!(idx1, 1);

        add_commit(repo.path(), "pit/test-task", "change 2");
        let idx2 = create(repo.path(), "test-task", "pit/test-task").unwrap();
        assert_eq!(idx2, 2);

        let checkpoints = list(repo.path(), "test-task").unwrap();
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].index, 1);
        assert_eq!(checkpoints[1].index, 2);
    }

    #[test]
    fn rollback_to_last_checkpoint() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "good change");
        create(repo.path(), "test-task", "pit/test-task").unwrap();

        // Get the checkpoint commit
        let cp_hash = list(repo.path(), "test-task").unwrap()[0]
            .commit_hash
            .clone();

        // Make another commit (the "bad" one)
        add_commit(repo.path(), "pit/test-task", "bad change");

        // Rollback (worktree = repo root since we're on the branch)
        let idx = rollback(repo.path(), "test-task", repo.path(), None).unwrap();
        assert_eq!(idx, 1);

        // Verify HEAD matches checkpoint
        let output = StdCommand::new("git")
            .args(["log", "-1", "--format=%h"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(head, cp_hash);
    }

    #[test]
    fn rollback_to_specific_index() {
        let repo = make_git_repo();

        add_commit(repo.path(), "pit/test-task", "v1");
        create(repo.path(), "test-task", "pit/test-task").unwrap();

        add_commit(repo.path(), "pit/test-task", "v2");
        create(repo.path(), "test-task", "pit/test-task").unwrap();

        add_commit(repo.path(), "pit/test-task", "v3");

        // Rollback to checkpoint 1
        let idx = rollback(repo.path(), "test-task", repo.path(), Some(1)).unwrap();
        assert_eq!(idx, 1);

        let output = StdCommand::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let msg = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(msg, "v1");
    }

    #[test]
    fn rollback_no_checkpoints_fails() {
        let repo = make_git_repo();
        let result = rollback(repo.path(), "test-task", repo.path(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no checkpoints"));
    }

    #[test]
    fn list_empty_returns_empty_vec() {
        let repo = make_git_repo();
        let checkpoints = list(repo.path(), "nonexistent-task").unwrap();
        assert!(checkpoints.is_empty());
    }
}
