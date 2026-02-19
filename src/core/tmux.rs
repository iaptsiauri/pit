use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Default tmux socket name for pit. Using a dedicated socket avoids
/// polluting the user's normal tmux server and makes testing easy.
const SOCKET: &str = "pit";

/// Tmux config for pit sessions.
/// - Rebinds prefix to Ctrl-] so it doesn't conflict with Claude Code
///   (Claude captures Ctrl-b, making the default tmux prefix unusable)
/// - Shows a status bar reminder of the detach key
const TMUX_CONF: &str = "\
# pit tmux config — prefix is Ctrl-]
unbind C-b
set -g prefix C-]
bind C-] send-prefix
bind d detach-client

# Status bar with detach hint
set -g status on
set -g status-style 'bg=#1a1a2e,fg=#888888'
set -g status-left '#[fg=#e0af68,bold] pit #[fg=#555555]│ '
set -g status-left-length 20
set -g status-right '#[fg=#555555]│ #[fg=#e0af68]Ctrl-] d#[fg=#888888] to detach '
set -g status-right-length 40

# Terminal settings
set -g default-terminal 'xterm-256color'
set -ga terminal-overrides ',xterm-256color:Tc'
set -g mouse on
";

/// Ensure the pit tmux config file exists and return its path.
fn ensure_config() -> Result<PathBuf> {
    let config_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("pit");
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;

    let config_path = config_dir.join("tmux.conf");

    // Write config if missing or outdated
    let needs_write = match std::fs::read_to_string(&config_path) {
        Ok(existing) => existing != TMUX_CONF,
        Err(_) => true,
    };

    if needs_write {
        std::fs::write(&config_path, TMUX_CONF)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
    }

    Ok(config_path)
}

/// Build base tmux args: [-f config] -L socket
fn base_args() -> Vec<String> {
    let mut args = Vec::new();
    if let Ok(conf) = ensure_config() {
        args.push("-f".to_string());
        args.push(conf.to_string_lossy().to_string());
    }
    args.push("-L".to_string());
    args.push(SOCKET.to_string());
    args
}

/// Check if tmux is available on the system.
pub fn is_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a tmux session exists.
pub fn session_exists(name: &str) -> bool {
    let mut args = base_args();
    args.extend(["has-session".into(), "-t".into(), name.into()]);
    Command::new("tmux")
        .args(&args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create a new detached tmux session in the given working directory.
pub fn create_session(name: &str, cwd: &str) -> Result<()> {
    let mut args = base_args();
    args.extend([
        "new-session".into(),
        "-d".into(),
        "-s".into(),
        name.into(),
        "-c".into(),
        cwd.into(),
    ]);
    let output = Command::new("tmux").args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("tmux new-session failed: {}", stderr.trim());
    }
    Ok(())
}

/// Send keys to a tmux session (typically a command + Enter).
pub fn send_keys(name: &str, keys: &[&str]) -> Result<()> {
    let mut args = base_args();
    args.extend(["send-keys".into(), "-t".into(), name.into()]);
    for k in keys {
        args.push(k.to_string());
    }

    let output = Command::new("tmux").args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("tmux send-keys failed: {}", stderr.trim());
    }
    Ok(())
}

/// Attach to a tmux session (interactive — takes over the terminal).
pub fn attach(name: &str) -> Result<std::process::ExitStatus> {
    let mut args = base_args();
    args.extend(["attach".into(), "-t".into(), name.into()]);
    let status = Command::new("tmux")
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;
    Ok(status)
}

/// Kill a tmux session.
pub fn kill_session(name: &str) -> Result<()> {
    let mut args = base_args();
    args.extend(["kill-session".into(), "-t".into(), name.into()]);
    let _ = Command::new("tmux").args(&args).output();
    Ok(())
}

/// List all pit tmux sessions. Returns session names.
pub fn list_sessions() -> Result<Vec<String>> {
    let mut args = base_args();
    args.extend([
        "list-sessions".into(),
        "-F".into(),
        "#{session_name}".into(),
    ]);
    let output = Command::new("tmux").args(&args).output()?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let sessions: Vec<String> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(sessions)
}

/// Capture the last N lines from a tmux pane (for status preview).
pub fn capture_pane(name: &str, lines: usize) -> Result<String> {
    let start = format!("-{}", lines);
    let mut args = base_args();
    args.extend([
        "capture-pane".into(),
        "-t".into(),
        name.into(),
        "-p".into(),
        "-S".into(),
        start,
    ]);
    let output = Command::new("tmux").args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("tmux capture-pane failed: {}", stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the tmux session name for a task.
pub fn session_name(task_name: &str) -> String {
    format!("pit-{}", task_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Use a unique socket per test to avoid interference.
    /// We override SOCKET by calling tmux directly with -L in tests.

    fn test_socket() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "pit-test-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn tmux_cmd(socket: &str) -> Command {
        let mut cmd = Command::new("tmux");
        cmd.args(["-L", socket]);
        cmd
    }

    fn create(socket: &str, name: &str) -> bool {
        tmux_cmd(socket)
            .args(["new-session", "-d", "-s", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn exists(socket: &str, name: &str) -> bool {
        tmux_cmd(socket)
            .args(["has-session", "-t", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn kill(socket: &str, name: &str) {
        let _ = tmux_cmd(socket)
            .args(["kill-session", "-t", name])
            .output();
    }

    fn kill_server(socket: &str) {
        let _ = tmux_cmd(socket)
            .args(["kill-server"])
            .output();
    }

    #[test]
    fn tmux_is_available() {
        assert!(is_available(), "tmux must be installed for these tests");
    }

    #[test]
    fn create_and_check_session() {
        let sock = test_socket();
        let name = "test-create";

        assert!(!exists(&sock, name));
        assert!(create(&sock, name));
        assert!(exists(&sock, name));

        kill(&sock, name);
        kill_server(&sock);
    }

    #[test]
    fn kill_session_removes_it() {
        let sock = test_socket();
        let name = "test-kill";

        create(&sock, name);
        assert!(exists(&sock, name));

        kill(&sock, name);
        assert!(!exists(&sock, name));

        kill_server(&sock);
    }

    #[test]
    fn list_sessions_works() {
        let sock = test_socket();

        create(&sock, "list-a");
        create(&sock, "list-b");

        let output = tmux_cmd(&sock)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let sessions: Vec<&str> = stdout.lines().collect();

        assert!(sessions.contains(&"list-a"));
        assert!(sessions.contains(&"list-b"));

        kill(&sock, "list-a");
        kill(&sock, "list-b");
        kill_server(&sock);
    }

    #[test]
    fn send_keys_and_capture() {
        let sock = test_socket();
        let name = "test-capture";

        create(&sock, name);

        // Send an echo command
        tmux_cmd(&sock)
            .args(["send-keys", "-t", name, "echo HELLO_PIT", "Enter"])
            .output()
            .unwrap();

        // Wait a moment for the command to execute
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Capture pane output
        let output = tmux_cmd(&sock)
            .args(["capture-pane", "-t", name, "-p", "-S", "-10"])
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("HELLO_PIT"),
            "Expected HELLO_PIT in capture output: {}",
            stdout
        );

        kill(&sock, name);
        kill_server(&sock);
    }

    #[test]
    fn session_exists_returns_false_for_nonexistent() {
        // Use the pit socket (module-level function)
        assert!(!session_exists("definitely-does-not-exist-xyz"));
    }

    #[test]
    fn session_name_format() {
        assert_eq!(session_name("fix-bug"), "pit-fix-bug");
        assert_eq!(session_name("task_1"), "pit-task_1");
    }

    #[test]
    fn create_session_with_cwd() {
        let sock = test_socket();
        let name = "test-cwd";
        let dir = tempfile::tempdir().unwrap();

        let output = tmux_cmd(&sock)
            .args([
                "new-session",
                "-d",
                "-s",
                name,
                "-c",
                dir.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        // Send pwd and check it matches
        tmux_cmd(&sock)
            .args(["send-keys", "-t", name, "pwd", "Enter"])
            .output()
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(500));

        let output = tmux_cmd(&sock)
            .args(["capture-pane", "-t", name, "-p", "-S", "-5"])
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        // macOS resolves /tmp → /private/tmp, so check the dir name
        let dir_name = dir.path().file_name().unwrap().to_str().unwrap();
        assert!(
            stdout.contains(dir_name),
            "Expected dir name '{}' in pwd output: {}",
            dir_name,
            stdout
        );

        kill(&sock, name);
        kill_server(&sock);
    }
}
