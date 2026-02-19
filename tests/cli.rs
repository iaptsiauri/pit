use assert_cmd::Command;
use predicates::prelude::*;
use std::process;
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
        .stdout(predicate::str::contains("pit/fix-bug"));

    // Worktree should exist
    assert!(repo.path().join(".pit/worktrees/fix-bug").exists());
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
