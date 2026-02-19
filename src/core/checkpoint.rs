use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// A checkpoint is an annotated git tag capturing the full worktree state
/// plus context about what was done and what the agent's last output was.
///
/// Tag format: pit/checkpoint/<task-name>/<N>
/// Tag message contains structured sections: Done, Agent Context, Files Changed.

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub index: usize,
    pub tag: String,
    pub commit_hash: String,
    pub message: String,
    pub timestamp: String,
    /// The full annotated tag message (Done + Context + Files).
    pub annotation: String,
}

/// Create a checkpoint for a task. Commits uncommitted work first.
/// `agent_output` is optional captured terminal output from the agent.
pub fn create(
    repo_root: &Path,
    task_name: &str,
    branch: &str,
    worktree: &Path,
    agent_output: Option<&str>,
) -> Result<usize> {
    // Auto-commit any uncommitted changes
    auto_commit_if_dirty(worktree, task_name)?;

    let existing = list(repo_root, task_name)?;
    let next_idx = existing.last().map(|c| c.index + 1).unwrap_or(1);
    let tag = format!("pit/checkpoint/{}/{}", task_name, next_idx);

    // Get current HEAD
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

    // Build the annotated tag message
    let annotation = build_annotation(repo_root, task_name, branch, worktree, agent_output);

    // Create annotated tag (--cleanup=verbatim preserves ## headers)
    let output = Command::new("git")
        .args([
            "tag",
            "-a",
            &tag,
            &commit,
            "-m",
            &annotation,
            "--cleanup=verbatim",
        ])
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

        // Get commit info
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

        // Read the annotated tag message
        let annotation = read_tag_message(repo_root, tag_name);

        checkpoints.push(Checkpoint {
            index,
            tag: tag_name.to_string(),
            commit_hash,
            message,
            timestamp,
            annotation,
        });
    }

    checkpoints.sort_by_key(|c| c.index);
    Ok(checkpoints)
}

/// Rollback a task's worktree to a checkpoint.
/// Creates a safety tag before resetting.
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

    auto_commit_if_dirty(worktree, task_name)?;
    save_pre_rollback_tag(repo_root, task_name, worktree)?;

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

/// Check if the branch has new commits since the last checkpoint.
pub fn has_new_commits(repo_root: &Path, task_name: &str, branch: &str) -> bool {
    let checkpoints = match list(repo_root, task_name) {
        Ok(c) => c,
        Err(_) => return false,
    };

    if checkpoints.is_empty() {
        return has_commits_beyond_main(repo_root, branch);
    }

    let last_tag = &checkpoints.last().unwrap().tag;

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

// ── Annotation builder ──

/// Build the structured annotation for a checkpoint tag.
fn build_annotation(
    repo_root: &Path,
    task_name: &str,
    branch: &str,
    worktree: &Path,
    agent_output: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    parts.push(format!("[pit checkpoint] {}", task_name));
    parts.push(String::new());

    // ## Done — from commit messages since last checkpoint or main
    let done = gather_done_section(repo_root, task_name, branch);
    if !done.is_empty() {
        parts.push("## Done".to_string());
        for line in &done {
            parts.push(format!("- {}", line));
        }
        parts.push(String::new());
    }

    // ## Agent Context — last meaningful lines from agent output
    if let Some(output) = agent_output {
        let context = extract_agent_context(output);
        if !context.is_empty() {
            parts.push("## Agent Context".to_string());
            for line in &context {
                parts.push(line.clone());
            }
            parts.push(String::new());
        }
    }

    // ## Files Changed — diff stat
    let files = gather_files_section(worktree);
    if !files.is_empty() {
        parts.push("## Files Changed".to_string());
        for f in &files {
            parts.push(f.clone());
        }
        parts.push(String::new());
    }

    parts.join("\n")
}

/// Get commit messages since the last checkpoint (or since main).
fn gather_done_section(repo_root: &Path, task_name: &str, branch: &str) -> Vec<String> {
    // Find the base: last checkpoint tag, or main branch
    let base = match list(repo_root, task_name) {
        Ok(cps) if !cps.is_empty() => cps.last().unwrap().tag.clone(),
        _ => {
            // Fall back to main/master
            for name in &["main", "master"] {
                let output = Command::new("git")
                    .args(["rev-parse", "--verify", &format!("refs/heads/{}", name)])
                    .current_dir(repo_root)
                    .output();
                if let Ok(o) = output {
                    if o.status.success() {
                        return gather_commits_since(repo_root, name, branch);
                    }
                }
            }
            return vec![];
        }
    };

    gather_commits_since(repo_root, &base, branch)
}

fn gather_commits_since(repo_root: &Path, base: &str, branch: &str) -> Vec<String> {
    let range = format!("{}..{}", base, branch);
    let output = Command::new("git")
        .args(["log", &range, "--format=%s", "--reverse"])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            // Skip auto-save commits from pit itself
            .filter(|l| !l.starts_with("[pit checkpoint]"))
            .map(|l| l.to_string())
            .collect(),
        _ => vec![],
    }
}

/// Extract meaningful context from agent terminal output.
/// Looks for the last substantive block — skips blank lines, shell prompts,
/// and keeps the last ~20 meaningful lines.
fn extract_agent_context(raw_output: &str) -> Vec<String> {
    let lines: Vec<&str> = raw_output.lines().collect();

    // Walk backwards to find meaningful content
    let meaningful: Vec<String> = lines
        .iter()
        .rev()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('$')
                && !trimmed.starts_with('%')
                && !trimmed.starts_with("~/")
                && !trimmed.starts_with("❯")
        })
        .take(20)
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    meaningful
}

/// Get the list of changed files in the worktree (vs HEAD).
fn gather_files_section(worktree: &Path) -> Vec<String> {
    // Use diff --stat for a compact summary
    let output = Command::new("git")
        .args(["diff", "HEAD~1", "--stat", "--stat-width=80"])
        .current_dir(worktree)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        _ => vec![],
    }
}

/// Read the message from an annotated git tag.
fn read_tag_message(repo_root: &Path, tag_name: &str) -> String {
    // git tag -l --format='%(contents)' <tag> gives the full annotation
    let output = Command::new("git")
        .args(["tag", "-l", "--format=%(contents)", tag_name])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

// ── Other helpers ──

fn auto_commit_if_dirty(worktree: &Path, task_name: &str) -> Result<bool> {
    let _ = Command::new("git")
        .args(["add", "-A"])
        .current_dir(worktree)
        .output();

    let output = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree)
        .output()
        .context("failed to check staged changes")?;

    if output.status.success() {
        return Ok(false);
    }

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

fn save_pre_rollback_tag(repo_root: &Path, task_name: &str, worktree: &Path) -> Result<()> {
    let tag = format!("pit/pre-rollback/{}", task_name);

    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree)
        .output()
        .context("failed to get worktree HEAD")?;

    if !output.status.success() {
        return Ok(());
    }

    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let _ = Command::new("git")
        .args(["tag", "-d", &tag])
        .current_dir(repo_root)
        .output();

    let _ = Command::new("git")
        .args(["tag", &tag, &commit])
        .current_dir(repo_root)
        .output();

    Ok(())
}

fn has_commits_beyond_main(repo_root: &Path, branch: &str) -> bool {
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

        let idx = create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();
        assert_eq!(idx, 1);

        let checkpoints = list(repo.path(), "test-task").unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].index, 1);
        assert_eq!(checkpoints[0].message, "first change");
    }

    #[test]
    fn checkpoint_has_annotation() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "implement feature X");

        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

        let checkpoints = list(repo.path(), "test-task").unwrap();
        assert!(!checkpoints[0].annotation.is_empty());
        assert!(checkpoints[0].annotation.contains("[pit checkpoint]"));
        assert!(checkpoints[0].annotation.contains("## Done"));
        assert!(checkpoints[0].annotation.contains("implement feature X"));
    }

    #[test]
    fn checkpoint_with_agent_context() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "add login endpoint");

        let agent_output = "I've implemented the login endpoint.\n\
                            Next steps:\n\
                            - Add rate limiting\n\
                            - Write integration tests\n\
                            ~/project (pit/test-task) $";

        create(
            repo.path(),
            "test-task",
            "pit/test-task",
            repo.path(),
            Some(agent_output),
        )
        .unwrap();

        let checkpoints = list(repo.path(), "test-task").unwrap();
        let ann = &checkpoints[0].annotation;
        assert!(ann.contains("## Agent Context"), "annotation: {}", ann);
        assert!(ann.contains("rate limiting"), "annotation: {}", ann);
    }

    #[test]
    fn checkpoint_auto_commits_dirty_worktree() {
        let repo = make_git_repo();
        StdCommand::new("git")
            .args(["checkout", "pit/test-task"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        write_file(repo.path(), "dirty.txt", "uncommitted work");

        let idx = create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();
        assert_eq!(idx, 1);

        let output = StdCommand::new("git")
            .args(["show", "HEAD:dirty.txt"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn multiple_checkpoints_increment() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "change 1");
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

        add_commit(repo.path(), "pit/test-task", "change 2");
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

        let checkpoints = list(repo.path(), "test-task").unwrap();
        assert_eq!(checkpoints.len(), 2);
        // Second checkpoint's Done section should only have "change 2"
        // (change 1 was before checkpoint #1)
        assert!(checkpoints[1].annotation.contains("change 2"));
    }

    #[test]
    fn rollback_creates_pre_rollback_tag() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "good");
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

        add_commit(repo.path(), "pit/test-task", "bad");

        let output = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let pre_rollback_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

        rollback(repo.path(), "test-task", repo.path(), None).unwrap();

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
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

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
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

        add_commit(repo.path(), "pit/test-task", "v2");
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

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
    }

    #[test]
    fn has_new_commits_detects_changes() {
        let repo = make_git_repo();
        add_commit(repo.path(), "pit/test-task", "work");
        create(repo.path(), "test-task", "pit/test-task", repo.path(), None).unwrap();

        assert!(!has_new_commits(repo.path(), "test-task", "pit/test-task"));

        add_commit(repo.path(), "pit/test-task", "more work");
        assert!(has_new_commits(repo.path(), "test-task", "pit/test-task"));
    }

    #[test]
    fn extract_agent_context_filters_prompts() {
        let output = "Implemented the feature\n\
                       Added tests\n\
                       ~/project (main) $\n\
                       \n\
                       ❯ \n";
        let ctx = extract_agent_context(output);
        assert!(!ctx.iter().any(|l| l.contains("~/project")));
        assert!(!ctx.iter().any(|l| l.contains("❯")));
        assert!(ctx.iter().any(|l| l.contains("Implemented")));
        assert!(ctx.iter().any(|l| l.contains("Added tests")));
    }

    #[test]
    fn list_empty_returns_empty_vec() {
        let repo = make_git_repo();
        let checkpoints = list(repo.path(), "nonexistent-task").unwrap();
        assert!(checkpoints.is_empty());
    }
}
