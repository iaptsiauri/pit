use assert_cmd::Command;
use predicates::prelude::*;
use std::process;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn make_git_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn init_creates_pit_directory() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized pit in"));

    assert!(repo.path().join(".pit").exists());
    assert!(repo.path().join(".pit/pit.db").exists());
}

#[test]
fn init_is_idempotent() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    // Second init should also succeed
    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();
}

#[test]
fn init_fails_outside_git_repo() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .failure();
}

#[test]
fn help_flag_shows_usage() {
    Command::cargo_bin("pit")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"));
}

#[test]
fn version_flag_works() {
    Command::cargo_bin("pit")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("pit 0.2.0"));
}

#[test]
fn new_creates_task() {
    let repo = make_git_repo();

    // Init first
    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    // Create task
    Command::cargo_bin("pit")
        .unwrap()
        .args(["new", "fix-bug", "-d", "Fix the login bug"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Created task 'fix-bug'"))
        .stdout(predicate::str::contains("pit/fix-bug"))
        .stdout(predicate::str::contains("agent: claude"));

    // Worktree should exist
    assert!(repo.path().join(".pit/worktrees/fix-bug").exists());
}

#[test]
fn new_with_agent_flag() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["new", "codex-task", "-a", "codex", "-p", "refactor API"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("agent: codex"))
        .stdout(predicate::str::contains("prompt: refactor API"));
}

#[test]
fn list_shows_tasks() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["new", "task-a"])
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["new", "task-b"])
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("list")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("task-a"))
        .stdout(predicate::str::contains("task-b"))
        .stdout(predicate::str::contains("2 task(s)"));
}

#[test]
fn list_empty_shows_hint() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("list")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No tasks"));
}

#[test]
fn delete_removes_task() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["new", "doomed"])
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["delete", "doomed"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Deleted task 'doomed'"));

    // Worktree should be gone
    assert!(!repo.path().join(".pit/worktrees/doomed").exists());

    // List should be empty
    Command::cargo_bin("pit")
        .unwrap()
        .arg("list")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No tasks"));
}

#[test]
fn delete_nonexistent_fails() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["delete", "ghost"])
        .current_dir(repo.path())
        .assert()
        .failure();
}

fn init_repo_with_task(repo: &TempDir, name: &str) {
    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["new", name])
        .current_dir(repo.path())
        .assert()
        .success();
}

#[test]
fn status_shows_tasks() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "status-task");

    Command::cargo_bin("pit")
        .unwrap()
        .arg("status")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("status-task"))
        .stdout(predicate::str::contains("idle"));
}

#[test]
fn status_empty() {
    let repo = make_git_repo();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("init")
        .current_dir(repo.path())
        .assert()
        .success();

    Command::cargo_bin("pit")
        .unwrap()
        .arg("status")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No tasks"));
}

#[test]
fn run_starts_task_in_background() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "run-task");

    Command::cargo_bin("pit")
        .unwrap()
        .args(["run", "run-task"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Started task 'run-task'"))
        .stdout(predicate::str::contains("pit-run-task"));

    // Verify the tmux session exists
    let tmux_check = process::Command::new("tmux")
        .args(["-L", "pit", "has-session", "-t", "pit-run-task"])
        .output()
        .unwrap();
    assert!(tmux_check.status.success(), "tmux session should exist");

    // Clean up
    let _ = process::Command::new("tmux")
        .args(["-L", "pit", "kill-session", "-t", "pit-run-task"])
        .output();
}

#[test]
fn run_already_running_says_so() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "dup-run");

    // First run
    Command::cargo_bin("pit")
        .unwrap()
        .args(["run", "dup-run"])
        .current_dir(repo.path())
        .assert()
        .success();

    // Second run — already running
    Command::cargo_bin("pit")
        .unwrap()
        .args(["run", "dup-run"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("already running"));

    // Clean up
    let _ = process::Command::new("tmux")
        .args(["-L", "pit", "kill-session", "-t", "pit-dup-run"])
        .output();
}

#[test]
fn stop_kills_running_task() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "stop-task");

    // Start it
    Command::cargo_bin("pit")
        .unwrap()
        .args(["run", "stop-task"])
        .current_dir(repo.path())
        .assert()
        .success();

    // Stop it
    Command::cargo_bin("pit")
        .unwrap()
        .args(["stop", "stop-task"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Stopped task 'stop-task'"));

    // tmux session should be gone
    let tmux_check = process::Command::new("tmux")
        .args(["-L", "pit", "has-session", "-t", "pit-stop-task"])
        .output()
        .unwrap();
    assert!(!tmux_check.status.success(), "tmux session should be gone");
}

#[test]
fn stop_not_running_says_so() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "not-running");

    Command::cargo_bin("pit")
        .unwrap()
        .args(["stop", "not-running"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("not running"));
}

#[test]
fn diff_no_changes() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "diff-task");

    Command::cargo_bin("pit")
        .unwrap()
        .args(["diff", "diff-task"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No changes"));
}

#[test]
fn diff_shows_changes() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "diff-changes");

    // Make a change in the worktree and commit it
    let worktree = repo.path().join(".pit/worktrees/diff-changes");
    std::fs::write(worktree.join("test.txt"), "hello").unwrap();

    process::Command::new("git")
        .args(["add", "test.txt"])
        .current_dir(&worktree)
        .output()
        .unwrap();

    process::Command::new("git")
        .args(["commit", "-m", "add test file"])
        .current_dir(&worktree)
        .output()
        .unwrap();

    Command::cargo_bin("pit")
        .unwrap()
        .args(["diff", "diff-changes"])
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("test.txt"));
}

#[test]
fn status_reaps_dead_sessions() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "reap-test");

    // Start the task
    Command::cargo_bin("pit")
        .unwrap()
        .args(["run", "reap-test"])
        .current_dir(repo.path())
        .assert()
        .success();

    // Verify it shows as running
    Command::cargo_bin("pit")
        .unwrap()
        .arg("status")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("running"));

    // Kill the tmux session behind pit's back
    let _ = process::Command::new("tmux")
        .args(["-L", "pit", "kill-session", "-t", "pit-reap-test"])
        .output();

    // Wait a moment
    thread::sleep(Duration::from_millis(100));

    // Status should now show "done" (reaped)
    Command::cargo_bin("pit")
        .unwrap()
        .arg("status")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("done"));
}

#[test]
fn config_path_shows_path() {
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config.toml"));
}

#[test]
fn config_list_empty() {
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Example"));
}

#[test]
fn config_set_get_unset() {
    // Set
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "set", "test.value", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Set test.value"));

    // Get
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "get", "test.value"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));

    // Unset
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "unset", "test.value"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed"));

    // Get again — should be gone
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "get", "test.value"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not set"));
}

#[test]
fn config_masks_secrets() {
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "set", "linear.api_key", "lin_api_very_secret_key_12345"])
        .assert()
        .success()
        .stdout(predicate::str::contains("lin_...2345"));

    // Clean up
    Command::cargo_bin("pit")
        .unwrap()
        .args(["config", "unset", "linear.api_key"])
        .assert()
        .success();
}

#[test]
fn list_also_reaps() {
    let repo = make_git_repo();
    init_repo_with_task(&repo, "reap-list");

    // Start it
    Command::cargo_bin("pit")
        .unwrap()
        .args(["run", "reap-list"])
        .current_dir(repo.path())
        .assert()
        .success();

    // Kill tmux behind pit's back
    let _ = process::Command::new("tmux")
        .args(["-L", "pit", "kill-session", "-t", "pit-reap-list"])
        .output();

    thread::sleep(Duration::from_millis(100));

    // List should show done (reaped)
    Command::cargo_bin("pit")
        .unwrap()
        .arg("list")
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("done"));
}
