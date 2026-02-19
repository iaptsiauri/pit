use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// A checkpoint is a git commit + tag capturing the full worktree state.
/// Format: pit/checkpoint/<task-name>/<N>
///
/// Unlike plain tags, checkpoints auto-commit uncommitted work so nothing is lost.
/// Rollback creates a safety tag before resetting so you can undo mistakes.

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub index: usize,
    pub tag: String,
    pub commit_hash: String,
    pub message: String,
    pub timestamp: String,
}

/// Create a checkpoint for a task. Commits any uncommitted work first.
/// Returns the checkpoint index.
pub fn create(repo_root: &Path, task_name: &str, branch: &str, worktree: &Path) -> Result<usize> {
    // Auto-commit any uncommitted changes in the worktree
    auto_commit_if_dirty(worktree, task_name)?;

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

/// Rollback a task's worktree to a checkpoint.
/// Creates a safety tag (pit/pre-rollback/<task>) before resetting so you can undo.
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

    // Save current state before rollback (safety net)
    auto_commit_if_dirty(worktree, task_name)?;
    save_pre_rollback_tag(repo_root, task_name, worktree)?;

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

/// Check if the last commit on the task branch is newer than the latest checkpoint.
/// Used for auto-checkpoint: returns true if there are new commits since the last checkpoint.
pub fn has_new_commits(repo_root: &Path, task_name: &str, branch: &str) -> bool {
    let checkpoints = match list(repo_root, task_name) {
        Ok(c) => c,
        Err(_) => return false,
    };

    if checkpoints.is_empty() {
        // No checkpoints yet — check if there are any commits beyond main
        return has_commits_beyond_main(repo_root, branch);
    }

    let last_tag = &checkpoints.last().unwrap().tag;

    // Check if branch HEAD is ahead of the last checkpoint
    let output = Command::new("git")
        .args(["rev-list", &format!("{}..{}", last_tag, branch), "--count"])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let count: usize = String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            count > 0
        }
        _ => false,
    }
}

// ── Internal helpers ──

/// Auto-commit all uncommitted changes (staged + unstaged + untracked) in the worktree.
/// Returns Ok(true) if a commit was made, Ok(false) if worktree was clean.
fn auto_commit_if_dirty(worktree: &Path, task_name: &str) -> Result<bool> {
    // Stage everything (including untracked)
    let _ = Command::new("git")
        .args(["add", "-A"])
        .current_dir(worktree)
        .output();

    // Check if there's anything to commit
    let output = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree)
        .output()
        .context("failed to check staged changes")?;

    if output.status.success() {
        // Exit code 0 = no changes
        return Ok(false);
    }

    // Commit with a pit checkpoint message
    let msg = format!("[pit checkpoint] auto-save for {}", task_name);
    let output = Command::new("git")
        .args(["commit", "-m", &msg])
        .current_dir(worktree)
        .output()
        .context("failed to auto-commit")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("auto-commit failed: {}", stderr.trim());
    }

    Ok(true)
}

/// Tag the current worktree HEAD as a pre-rollback safety point.
/// Overwrites any previous pre-rollback tag for this task.
fn save_pre_rollback_tag(repo_root: &Path, task_name: &str, worktree: &Path) -> Result<()> {
    let tag = format!("pit/pre-rollback/{}", task_name);

    // Get current HEAD of the worktree
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree)
        .output()
        .context("failed to get worktree HEAD")?;

    if !output.status.success() {
        return Ok(()); // silently skip if we can't resolve HEAD
    }

    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Delete existing tag if present (force overwrite)
    let _ = Command::new("git")
        .args(["tag", "-d", &tag])
        .current_dir(repo_root)
        .output();

    // Create the safety tag
    let _ = Command::new("git")
        .args(["tag", &tag, &commit])
        .current_dir(repo_root)
        .output();

    Ok(())
}

/// Check if the branch has any commits beyond main.
fn has_commits_beyond_main(repo_root: &Path, branch: &str) -> bool {
    // Try to detect main branch
    for main in &["main", "master"] {
        let output = Command::new("git")
            .args(["rev-list", &format!("{}..{}", main, branch), "--count"])
            .current_dir(repo_root)
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                let count: usize = String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse()
                    .unwrap_or(0);
                return count > 0;
            }
        }
    }
    false
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

    fn write_file(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn create_and_list_checkpoint() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "first change");

        let idx = create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();
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
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        add_commit(repo.path(), "pit/test-task", "change 2");
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        let checkpoints = list(repo.path(), "test-task").unwrap();
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].index, 1);
        assert_eq!(checkpoints[1].index, 2);
    }

    #[test]
    fn checkpoint_auto_commits_dirty_worktree() {
        let repo = make_git_repo();
        StdCommand::new("git")
            .args(["checkout", "pit/test-task"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        // Create an uncommitted file
        write_file(repo.path(), "dirty.txt", "uncommitted work");

        let idx = create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();
        assert_eq!(idx, 1);

        // Verify the file was committed
        let output = StdCommand::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let msg = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert!(msg.contains("[pit checkpoint]"));

        // File should be in the checkpoint
        let output = StdCommand::new("git")
            .args(["show", "HEAD:dirty.txt"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn rollback_creates_pre_rollback_tag() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "good");
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        add_commit(repo.path(), "pit/test-task", "bad");

        // Get current HEAD before rollback
        let output = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let pre_rollback_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

        rollback(repo.path(), "test-task", repo.path(), None).unwrap();

        // Pre-rollback tag should point to the old HEAD
        let output = StdCommand::new("git")
            .args(["rev-parse", "pit/pre-rollback/test-task"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let tag_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(tag_commit, pre_rollback_commit);
    }

    #[test]
    fn rollback_to_last_checkpoint() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "good change");
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        let cp_hash = list(repo.path(), "test-task").unwrap()[0]
            .commit_hash
            .clone();

        add_commit(repo.path(), "pit/test-task", "bad change");

        let idx = rollback(repo.path(), "test-task", repo.path(), None).unwrap();
        assert_eq!(idx, 1);

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
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        add_commit(repo.path(), "pit/test-task", "v2");
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        add_commit(repo.path(), "pit/test-task", "v3");

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
    fn has_new_commits_detects_changes() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "work");
        create(repo.path(), "test-task", "pit/test-task", repo.path()).unwrap();

        // No new commits
        assert!(!has_new_commits(repo.path(), "test-task", "pit/test-task"));

        // Add a new commit
        add_commit(repo.path(), "pit/test-task", "more work");
        assert!(has_new_commits(repo.path(), "test-task", "pit/test-task"));
    }

    #[test]
    fn list_empty_returns_empty_vec() {
        let repo = make_git_repo();
        let checkpoints = list(repo.path(), "nonexistent-task").unwrap();
        assert!(checkpoints.is_empty());
    }
}
