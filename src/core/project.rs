use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The hidden directory inside the repo root that holds pit's state.
const PIT_DIR: &str = ".pit";
const DB_FILE: &str = "pit.db";

/// Resolved project context — everything needed to operate on an initialized pit project.
#[derive(Debug)]
pub struct Project {
    /// The git repository root.
    pub repo_root: PathBuf,
    /// Path to the .pit directory.
    pub pit_dir: PathBuf,
    /// Open database connection.
    pub db: Connection,
}

impl Project {
    /// Initialize a new pit project in the current (or given) git repo.
    /// Creates `.pit/` and the SQLite database. Idempotent — safe to call twice.
    pub fn init(repo_root: &Path) -> Result<Self> {
        // Verify it's a git repo
        let git_dir = repo_root.join(".git");
        if !git_dir.exists() {
            bail!(
                "{} is not a git repository (no .git found)",
                repo_root.display()
            );
        }

        let pit_dir = repo_root.join(PIT_DIR);
        std::fs::create_dir_all(&pit_dir)
            .with_context(|| format!("failed to create {}", pit_dir.display()))?;

        // Add .pit to .gitignore if not already there
        ensure_gitignored(repo_root)?;

        let db_path = pit_dir.join(DB_FILE);
        let db = crate::db::open(&db_path)?;

        Ok(Project {
            repo_root: repo_root.to_path_buf(),
            pit_dir,
            db,
        })
    }

    /// Open an existing pit project. Fails if not initialized.
    pub fn open(repo_root: &Path) -> Result<Self> {
        let pit_dir = repo_root.join(PIT_DIR);
        let db_path = pit_dir.join(DB_FILE);

        if !db_path.exists() {
            bail!(
                "not a pit project (no {}). Run `pit init` first.",
                db_path.display()
            );
        }

        let db = crate::db::open(&db_path)?;

        Ok(Project {
            repo_root: repo_root.to_path_buf(),
            pit_dir,
            db,
        })
    }

    /// Find the repo root by walking up from `start` looking for `.git`.
    pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
        let mut dir = start.to_path_buf();
        loop {
            if dir.join(".git").exists() {
                return Ok(dir);
            }
            if !dir.pop() {
                bail!("not inside a git repository");
            }
        }
    }

    /// Find and open a pit project by walking up from `start`.
    pub fn find_and_open(start: &Path) -> Result<Self> {
        let root = Self::find_repo_root(start)?;
        Self::open(&root)
    }
}

/// Ensure `.pit` and `.pit-prompt` are listed in `.gitignore`.
fn ensure_gitignored(repo_root: &Path) -> Result<()> {
    let gitignore = repo_root.join(".gitignore");
    let mut content = if gitignore.exists() {
        std::fs::read_to_string(&gitignore)?
    } else {
        String::new()
    };

    let entries = [".pit", ".pit-prompt"];
    let mut needs_write = false;

    for entry in &entries {
        let already_present = content
            .lines()
            .any(|l| l.trim() == *entry || l.trim() == format!("{}/", entry));

        if already_present {
            continue;
        }

        // Check via git — maybe it's ignored by a parent .gitignore or global ignore
        let output = Command::new("git")
            .args(["check-ignore", "-q", entry])
            .current_dir(repo_root)
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                continue; // Already ignored by some rule
            }
        }

        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(entry);
        content.push('\n');
        needs_write = true;
    }

    if needs_write {
        std::fs::write(&gitignore, content)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn make_git_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Set user for commits
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Need at least one commit for worktrees to work later
        Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn init_creates_pit_dir_and_db() {
        let repo = make_git_repo();
        let project = Project::init(repo.path()).unwrap();

        assert!(project.pit_dir.exists());
        assert!(project.pit_dir.join("pit.db").exists());
    }

    #[test]
    fn init_is_idempotent() {
        let repo = make_git_repo();
        let _p1 = Project::init(repo.path()).unwrap();
        drop(_p1);
        let _p2 = Project::init(repo.path()).unwrap();
        // No error, no duplicate .gitignore entries
        let content = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
        // Should have exactly one ".pit" line and one ".pit-prompt" line
        assert_eq!(
            content.lines().filter(|l| l.trim() == ".pit").count(),
            1,
            "gitignore: {}",
            content
        );
        assert_eq!(
            content
                .lines()
                .filter(|l| l.trim() == ".pit-prompt")
                .count(),
            1,
            "gitignore: {}",
            content
        );
    }

    #[test]
    fn init_fails_outside_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let result = Project::init(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not a git repository"));
    }

    #[test]
    fn open_fails_if_not_initialized() {
        let repo = make_git_repo();
        let result = Project::open(repo.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not a pit project"));
    }

    #[test]
    fn open_succeeds_after_init() {
        let repo = make_git_repo();
        let _p1 = Project::init(repo.path()).unwrap();
        drop(_p1);
        let p2 = Project::open(repo.path()).unwrap();
        assert_eq!(p2.repo_root, repo.path());
    }

    #[test]
    fn find_repo_root_walks_up() {
        let repo = make_git_repo();
        let sub = repo.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&sub).unwrap();
        let root = Project::find_repo_root(&sub).unwrap();
        assert_eq!(root, repo.path());
    }

    #[test]
    fn find_repo_root_fails_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let result = Project::find_repo_root(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn gitignore_preserves_existing_content() {
        let repo = make_git_repo();
        std::fs::write(repo.path().join(".gitignore"), "node_modules\n").unwrap();
        let _p = Project::init(repo.path()).unwrap();
        let content = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
        assert!(content.contains("node_modules"));
        assert!(content.contains(".pit"));
    }

    #[test]
    fn find_and_open_works_from_subdirectory() {
        let repo = make_git_repo();
        let _p = Project::init(repo.path()).unwrap();
        drop(_p);

        let sub = repo.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let project = Project::find_and_open(&sub).unwrap();
        assert_eq!(project.repo_root, repo.path());
    }
}
