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
/// Get the diff for a specific file, including uncommitted worktree changes.
pub fn file_diff_with_worktree(
    repo_root: &Path,
    branch: &str,
    file_path: &str,
    worktree: Option<&Path>,
) -> Vec<String> {
    let main = match detect_main_branch(repo_root) {
        Ok(m) => m,
        Err(_) => return vec![],
    };

    // Committed diff: main...branch
    let range = format!("{}...{}", main, branch);
    let committed = Command::new("git")
        .args(["diff", &range, "--", file_path])
        .current_dir(repo_root)
        .output()
        .ok();

    // Uncommitted diff: HEAD in the worktree
    let uncommitted = worktree.and_then(|wt| {
        Command::new("git")
            .args(["diff", "HEAD", "--", file_path])
            .current_dir(wt)
            .output()
            .ok()
    });

    // Combine both outputs
    let mut all_lines = String::new();
    if let Some(ref o) = committed {
        if o.status.success() {
            all_lines.push_str(&String::from_utf8_lossy(&o.stdout));
        }
    }
    if let Some(ref o) = uncommitted {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout);
            if !s.trim().is_empty() {
                if !all_lines.is_empty() {
                    all_lines.push('\n');
                }
                all_lines.push_str(&s);
            }
        }
    }

    // For untracked files, show the whole content as additions
    if all_lines.is_empty() {
        if let Some(wt) = worktree {
            let full_path = wt.join(file_path);
            if full_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&full_path) {
                    let mut lines = vec![format!(
                        "@@ -0,0 +1,{} @@ (new file)",
                        content.lines().count()
                    )];
                    for l in content.lines() {
                        lines.push(format!("+{}", l));
                    }
                    return lines;
                }
            }
        }
    }

    // Skip diff headers, just show hunks
    all_lines
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

/// Detect the default branch name.
/// Tries: origin HEAD → common names (main, master, develop) → first local branch.
pub fn detect_main_branch(repo_root: &Path) -> Result<String> {
    // 1. Check origin's HEAD (most reliable for cloned repos)
    let output = Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(repo_root)
        .output();
    if let Ok(o) = output {
        if o.status.success() {
            let refname = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // refs/remotes/origin/main → main
            if let Some(name) = refname.strip_prefix("refs/remotes/origin/") {
                return Ok(name.to_string());
            }
        }
    }

    // 2. Try common branch names
    for name in &["main", "master", "develop"] {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{}", name)])
            .current_dir(repo_root)
            .output()?;
        if output.status.success() {
            return Ok(name.to_string());
        }
    }

    // 3. Fall back to the first branch listed
    let output = Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(repo_root)
        .output()?;
    if output.status.success() {
        if let Some(first) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|l| !l.is_empty())
        {
            return Ok(first.to_string());
        }
    }

    // Default to "main" if nothing detected (new repo, no remote, etc.)
    Ok("main".to_string())
}

/// Gather git log and diff stat for a task branch.
/// Gather git info, including uncommitted changes from a worktree.
pub fn gather_with_worktree(
    repo_root: &Path,
    branch: &str,
    worktree: Option<&Path>,
) -> TaskGitInfo {
    let main = match detect_main_branch(repo_root) {
        Ok(m) => m,
        Err(_) => return TaskGitInfo::default(),
    };

    let commits = gather_commits(repo_root, &main, branch).unwrap_or_default();
    let (mut files, mut total_ins, mut total_del) =
        gather_diff_stat(repo_root, &main, branch).unwrap_or_default();

    // Also include uncommitted changes from the worktree (staged + unstaged)
    if let Some(wt) = worktree {
        if let Ok((wt_files, _wt_ins, _wt_del)) = gather_worktree_changes(wt) {
            for wf in wt_files {
                // Merge: if file already in committed diff, add the extra changes
                if let Some(existing) = files.iter_mut().find(|f| f.path == wf.path) {
                    existing.insertions += wf.insertions;
                    existing.deletions += wf.deletions;
                    total_ins += wf.insertions;
                    total_del += wf.deletions;
                } else {
                    total_ins += wf.insertions;
                    total_del += wf.deletions;
                    files.push(wf);
                }
            }
        }
    }

    TaskGitInfo {
        commits,
        files,
        total_insertions: total_ins,
        total_deletions: total_del,
    }
}

/// Get uncommitted changes (staged + unstaged) in a worktree.
fn gather_worktree_changes(worktree: &Path) -> Result<(Vec<FileStat>, u32, u32)> {
    // git diff HEAD --numstat shows all uncommitted changes (staged + unstaged)
    let output = Command::new("git")
        .args(["diff", "HEAD", "--numstat"])
        .current_dir(worktree)
        .output()
        .context("failed to run git diff HEAD")?;

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

    // Also include untracked files
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(worktree)
        .output();

    if let Ok(o) = output {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            for path in stdout.lines().filter(|l| !l.is_empty()) {
                // Count lines in new file
                let line_count = std::fs::read_to_string(worktree.join(path))
                    .map(|s| s.lines().count() as u32)
                    .unwrap_or(0);
                total_ins += line_count;
                files.push(FileStat {
                    path: path.to_string(),
                    insertions: line_count,
                    deletions: 0,
                });
            }
        }
    }

    Ok((files, total_ins, total_del))
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
        let info = gather_with_worktree(repo.path(), &branch, None);

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

        let info = gather_with_worktree(p, "pit/empty-task", None);
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

        let info = gather_with_worktree(p, "pit/does-not-exist", None);
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
        let info = gather_with_worktree(repo.path(), &branch, None);

        for c in &info.commits {
            assert_eq!(c.hash.len(), 7, "short hash should be 7 chars: {}", c.hash);
            assert!(!c.message.is_empty());
            assert!(!c.age.is_empty());
        }
    }

    #[test]
    fn file_stat_has_insertions_deletions() {
        let (repo, branch) = make_repo_with_branch();
        let info = gather_with_worktree(repo.path(), &branch, None);

        let src = info.files.iter().find(|f| f.path == "src.rs").unwrap();
        assert!(src.insertions > 0, "src.rs should have insertions");

        let test = info.files.iter().find(|f| f.path == "test.rs").unwrap();
        assert!(test.insertions > 0, "test.rs should have insertions");
    }

    #[test]
    fn file_diff_returns_hunks() {
        let (repo, branch) = make_repo_with_branch();
        let lines = file_diff_with_worktree(repo.path(), &branch, "src.rs", None);

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
        let lines = file_diff_with_worktree(repo.path(), &branch, "test.rs", None);

        assert!(!lines.is_empty());
        assert!(lines.iter().any(|l| l.starts_with('+')));
    }

    #[test]
    fn file_diff_nonexistent_file() {
        let (repo, branch) = make_repo_with_branch();
        let lines = file_diff_with_worktree(repo.path(), &branch, "nope.rs", None);
        assert!(lines.is_empty());
    }

    #[test]
    fn file_diff_nonexistent_branch() {
        let (repo, _) = make_repo_with_branch();
        let lines = file_diff_with_worktree(repo.path(), "pit/nope", "src.rs", None);
        assert!(lines.is_empty());
    }
}
