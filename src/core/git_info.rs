//! Collect git log and diff stat for a task branch vs main.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// A single commit entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub hash: String,    // short hash (7 chars)
    pub message: String, // first line
    pub age: String,     // relative time like "2 hours ago"
}

/// A file change summary.
#[derive(Debug, Clone, PartialEq)]
pub struct FileStat {
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
}

/// All git info for a task.
#[derive(Debug, Clone, Default)]
pub struct TaskGitInfo {
    pub commits: Vec<Commit>,
    pub files: Vec<FileStat>,
    pub total_insertions: u32,
    pub total_deletions: u32,
}

/// Get the diff for a single file between main and branch.
/// Returns the diff output as lines (without the diff header noise).
pub fn file_diff(repo_root: &Path, branch: &str, file_path: &str) -> Vec<String> {
    let main = match detect_main_branch(repo_root) {
        Ok(m) => m,
        Err(_) => return vec![],
    };

    let range = format!("{}...{}", main, branch);
    let output = match Command::new("git")
        .args(["diff", &range, "--", file_path])
        .current_dir(repo_root)
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    if !output.status.success() {
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Skip the header lines (diff --git, index, ---, +++) and just show hunks
    stdout
        .lines()
        .skip_while(|line| {
            line.starts_with("diff --git")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("new file")
                || line.starts_with("old mode")
                || line.starts_with("new mode")
                || line.starts_with("deleted file")
                || line.starts_with("similarity")
                || line.starts_with("rename")
                || line.starts_with("Binary")
        })
        .map(|s| s.to_string())
        .collect()
}

/// Detect the main branch name (main or master).
fn detect_main_branch(repo_root: &Path) -> Result<String> {
    for name in &["main", "master"] {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{}", name)])
            .current_dir(repo_root)
            .output()?;
        if output.status.success() {
            return Ok(name.to_string());
        }
    }
    anyhow::bail!("could not find main or master branch")
}

/// Gather git log and diff stat for a task branch.
pub fn gather(repo_root: &Path, branch: &str) -> TaskGitInfo {
    let main = match detect_main_branch(repo_root) {
        Ok(m) => m,
        Err(_) => return TaskGitInfo::default(),
    };

    let commits = gather_commits(repo_root, &main, branch).unwrap_or_default();
    let (files, total_ins, total_del) =
        gather_diff_stat(repo_root, &main, branch).unwrap_or_default();

    TaskGitInfo {
        commits,
        files,
        total_insertions: total_ins,
        total_deletions: total_del,
    }
}

/// Get recent commits on `branch` that are not on `main`.
fn gather_commits(repo_root: &Path, main: &str, branch: &str) -> Result<Vec<Commit>> {
    let range = format!("{}..{}", main, branch);
    let output = Command::new("git")
        .args(["log", &range, "--format=%h\t%s\t%cr", "-n", "20"])
        .current_dir(repo_root)
        .output()
        .context("failed to run git log")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let commits = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let hash = parts.next()?.to_string();
            let message = parts.next()?.to_string();
            let age = parts.next().unwrap_or("").to_string();
            Some(Commit { hash, message, age })
        })
        .collect();

    Ok(commits)
}

/// Get diff stat (files changed) between main and branch.
fn gather_diff_stat(
    repo_root: &Path,
    main: &str,
    branch: &str,
) -> Result<(Vec<FileStat>, u32, u32)> {
    let range = format!("{}...{}", main, branch);
    let output = Command::new("git")
        .args(["diff", &range, "--numstat"])
        .current_dir(repo_root)
        .output()
        .context("failed to run git diff --numstat")?;

    if !output.status.success() {
        return Ok((vec![], 0, 0));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    let mut total_ins = 0u32;
    let mut total_del = 0u32;

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let ins_str = parts.next().unwrap_or("0");
        let del_str = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("").to_string();

        // Binary files show "-" for insertions/deletions
        let insertions = ins_str.parse::<u32>().unwrap_or(0);
        let deletions = del_str.parse::<u32>().unwrap_or(0);

        total_ins += insertions;
        total_del += deletions;

        files.push(FileStat {
            path,
            insertions,
            deletions,
        });
    }

    Ok((files, total_ins, total_del))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    fn make_repo_with_branch() -> (TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();

        StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(p)
            .output()
            .unwrap();

        // Initial commit on main
        std::fs::write(p.join("README.md"), "# Hello").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(p)
            .output()
            .unwrap();

        // Rename default branch to main (in case it's master)
        StdCommand::new("git")
            .args(["branch", "-M", "main"])
            .current_dir(p)
            .output()
            .unwrap();

        // Create task branch
        let branch = "pit/test-task";
        StdCommand::new("git")
            .args(["checkout", "-b", branch])
            .current_dir(p)
            .output()
            .unwrap();

        // Add some commits
        std::fs::write(p.join("src.rs"), "fn main() {}\n").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "Add main function"])
            .current_dir(p)
            .output()
            .unwrap();

        std::fs::write(
            p.join("src.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(p.join("test.rs"), "fn test() {}\n").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "Add print and test"])
            .current_dir(p)
            .output()
            .unwrap();

        // Go back to main
        StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(p)
            .output()
            .unwrap();

        (dir, branch.to_string())
    }

    #[test]
    fn gather_returns_commits_and_files() {
        let (repo, branch) = make_repo_with_branch();
        let info = gather(repo.path(), &branch);

        assert_eq!(info.commits.len(), 2);
        assert_eq!(info.commits[0].message, "Add print and test");
        assert_eq!(info.commits[1].message, "Add main function");
        assert!(!info.commits[0].hash.is_empty());
        assert!(!info.commits[0].age.is_empty());

        assert_eq!(info.files.len(), 2);
        let paths: Vec<&str> = info.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src.rs"));
        assert!(paths.contains(&"test.rs"));

        assert!(info.total_insertions > 0);
    }

    #[test]
    fn gather_empty_branch_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();

        StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["branch", "-M", "main"])
            .current_dir(p)
            .output()
            .unwrap();

        // Branch with no extra commits
        StdCommand::new("git")
            .args(["branch", "pit/empty-task"])
            .current_dir(p)
            .output()
            .unwrap();

        let info = gather(p, "pit/empty-task");
        assert!(info.commits.is_empty());
        assert!(info.files.is_empty());
        assert_eq!(info.total_insertions, 0);
        assert_eq!(info.total_deletions, 0);
    }

    #[test]
    fn gather_nonexistent_branch_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();

        StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["branch", "-M", "main"])
            .current_dir(p)
            .output()
            .unwrap();

        let info = gather(p, "pit/does-not-exist");
        assert!(info.commits.is_empty());
        assert!(info.files.is_empty());
    }

    #[test]
    fn detect_main_branch_works() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();

        StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["branch", "-M", "main"])
            .current_dir(p)
            .output()
            .unwrap();

        let result = detect_main_branch(p).unwrap();
        assert_eq!(result, "main");
    }

    #[test]
    fn commit_fields_populated() {
        let (repo, branch) = make_repo_with_branch();
        let info = gather(repo.path(), &branch);

        for c in &info.commits {
            assert_eq!(c.hash.len(), 7, "short hash should be 7 chars: {}", c.hash);
            assert!(!c.message.is_empty());
            assert!(!c.age.is_empty());
        }
    }

    #[test]
    fn file_stat_has_insertions_deletions() {
        let (repo, branch) = make_repo_with_branch();
        let info = gather(repo.path(), &branch);

        let src = info.files.iter().find(|f| f.path == "src.rs").unwrap();
        assert!(src.insertions > 0, "src.rs should have insertions");

        let test = info.files.iter().find(|f| f.path == "test.rs").unwrap();
        assert!(test.insertions > 0, "test.rs should have insertions");
    }

    #[test]
    fn file_diff_returns_hunks() {
        let (repo, branch) = make_repo_with_branch();
        let lines = file_diff(repo.path(), &branch, "src.rs");

        assert!(!lines.is_empty(), "should have diff lines");
        // First line should be a hunk header
        assert!(
            lines[0].starts_with("@@"),
            "expected hunk header, got: {}",
            lines[0]
        );
        // Should contain added lines
        assert!(
            lines.iter().any(|l| l.starts_with('+')),
            "should have additions"
        );
    }

    #[test]
    fn file_diff_new_file() {
        let (repo, branch) = make_repo_with_branch();
        let lines = file_diff(repo.path(), &branch, "test.rs");

        assert!(!lines.is_empty());
        assert!(lines.iter().any(|l| l.starts_with('+')));
    }

    #[test]
    fn file_diff_nonexistent_file() {
        let (repo, branch) = make_repo_with_branch();
        let lines = file_diff(repo.path(), &branch, "nope.rs");
        assert!(lines.is_empty());
    }

    #[test]
    fn file_diff_nonexistent_branch() {
        let (repo, _) = make_repo_with_branch();
        let lines = file_diff(repo.path(), "pit/nope", "src.rs");
        assert!(lines.is_empty());
    }
}
