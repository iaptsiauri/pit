mod core;
mod db;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::core::project::Project;
use crate::core::reap;
use crate::core::task;
use crate::core::tmux;

#[derive(Parser)]
#[command(
    name = "pit",
    version,
    about = "Run multiple coding agents in parallel"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize pit in the current git repository
    Init,

    /// Create a new task (git branch + worktree)
    New {
        /// Task name (alphanumeric, hyphens, underscores)
        name: String,
        /// Description of what the agent should do
        #[arg(short, long, default_value = "")]
        description: String,
        /// Prompt to send to the agent on first launch
        #[arg(short, long, default_value = "")]
        prompt: String,
        /// Link to an issue (GitHub, Linear, etc.)
        #[arg(short, long, default_value = "")]
        issue: String,
        /// Agent to use (claude, codex, amp, aider, custom)
        #[arg(short, long, default_value = "claude")]
        agent: String,
    },

    /// List all tasks
    #[command(alias = "ls")]
    List,

    /// Show task status (with live tmux reaping)
    Status,

    /// Run a task in the background (no TUI)
    Run {
        /// Task name
        name: String,
    },

    /// Stop a running task (kills its tmux session)
    Stop {
        /// Task name
        name: String,
    },

    /// Show the diff for a task's worktree vs main branch
    Diff {
        /// Task name
        name: String,
    },

    /// Open a shell in a task's worktree
    #[command(alias = "sh")]
    Shell {
        /// Task name
        name: String,
    },

    /// Delete a task (removes worktree and branch)
    #[command(alias = "rm")]
    Delete {
        /// Task name
        name: String,
    },

    /// Save a checkpoint of a task's current state
    Checkpoint {
        /// Task name
        name: String,
    },

    /// Rollback a task to the last checkpoint (or a specific one)
    Rollback {
        /// Task name
        name: String,
        /// Checkpoint number to rollback to (default: latest)
        #[arg(short, long)]
        to: Option<usize>,
    },

    /// Watch live output from a running task
    Watch {
        /// Task name
        name: String,
        /// Number of lines to show (default 30)
        #[arg(short = 'n', long, default_value = "30")]
        lines: usize,
    },

    /// Manage configuration (API keys, preferences)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set a config value (e.g. pit config set linear.api_key <key>)
    Set {
        /// Config key (e.g. linear.api_key, github.token)
        key: String,
        /// Value to set
        value: String,
    },
    /// Get a config value
    Get {
        /// Config key
        key: String,
    },
    /// Remove a config value
    Unset {
        /// Config key
        key: String,
    },
    /// List all config values
    #[command(alias = "ls")]
    List,
    /// Show config file path
    Path,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => cmd_dashboard()?,
        Some(Commands::Init) => cmd_init()?,
        Some(Commands::New {
            name,
            description,
            prompt,
            issue,
            agent,
        }) => cmd_new(&name, &description, &prompt, &issue, &agent)?,
        Some(Commands::List) => cmd_list()?,
        Some(Commands::Status) => cmd_status()?,
        Some(Commands::Run { name }) => cmd_run(&name)?,
        Some(Commands::Stop { name }) => cmd_stop(&name)?,
        Some(Commands::Diff { name }) => cmd_diff(&name)?,
        Some(Commands::Shell { name }) => cmd_shell(&name)?,
        Some(Commands::Delete { name }) => cmd_delete(&name)?,
        Some(Commands::Checkpoint { name }) => cmd_checkpoint(&name)?,
        Some(Commands::Rollback { name, to }) => cmd_rollback(&name, to)?,
        Some(Commands::Watch { name, lines }) => cmd_watch(&name, lines)?,
        Some(Commands::Config { action }) => cmd_config(action)?,
    }

    Ok(())
}

fn cmd_dashboard() -> Result<()> {
    let project = open_or_init_project()?;
    tui::run(&project)
}

fn cmd_init() -> Result<()> {
    let cwd = PathBuf::from(".");
    let repo_root = Project::find_repo_root(&cwd.canonicalize()?)?;
    let project = Project::init(&repo_root)?;
    println!("Initialized pit in {}", project.repo_root.display());
    Ok(())
}

fn cmd_new(name: &str, description: &str, prompt: &str, issue: &str, agent: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::create(
        &project.db,
        &project.repo_root,
        &task::CreateOpts {
            name,
            description,
            prompt,
            issue_url: issue,
            agent,
        },
    )?;
    println!(
        "Created task '{}' on branch '{}' (agent: {})",
        t.name, t.branch, t.agent
    );
    println!("  worktree: {}", t.worktree);
    if !t.prompt.is_empty() {
        println!("  prompt: {}", t.prompt);
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let project = open_project()?;
    reap::reap_dead(&project.db, &project.repo_root)?;
    let tasks = task::list(&project.db)?;

    if tasks.is_empty() {
        println!("No tasks. Create one with: pit new <name>");
        return Ok(());
    }

    println!("{:<4} {:<20} {:<10} BRANCH", "ID", "NAME", "STATUS");
    println!("{}", "-".repeat(60));
    for t in &tasks {
        println!("{:<4} {:<20} {:<10} {}", t.id, t.name, t.status, t.branch);
    }
    println!("\n{} task(s)", tasks.len());
    Ok(())
}

fn cmd_status() -> Result<()> {
    let project = open_project()?;
    let reaped = reap::reap_dead(&project.db, &project.repo_root)?;
    let tasks = task::list(&project.db)?;

    if tasks.is_empty() {
        println!("No tasks.");
        return Ok(());
    }

    for t in &tasks {
        let icon = match t.status {
            task::Status::Idle => "○",
            task::Status::Running => "▶",
            task::Status::Done => "✓",
            task::Status::Error => "✗",
        };
        let extra = match &t.tmux_session {
            Some(s) if t.status == task::Status::Running => format!("  (tmux: {})", s),
            _ => String::new(),
        };
        println!("{} {:<20} {}{}", icon, t.name, t.status, extra);
    }

    if reaped > 0 {
        println!("\n({} task(s) finished since last check)", reaped);
    }
    Ok(())
}

fn cmd_run(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    let tmux_name = tmux::session_name(&t.name);

    if tmux::session_exists(&tmux_name) {
        println!("Task '{}' is already running (tmux: {})", name, tmux_name);
        return Ok(());
    }

    let (agent_cmd, session_id) = tui::build_agent_cmd(&t);

    tmux::create_session_with_cmd(&tmux_name, &t.worktree, &agent_cmd)?;
    task::set_running(&project.db, t.id, &tmux_name, None, Some(&session_id))?;

    println!(
        "Started task '{}' ({}) in background (tmux: {})",
        name, t.agent, tmux_name
    );
    println!("  Attach with: tmux -L pit attach -t {}", tmux_name);
    println!("  Detach with: F1");
    Ok(())
}

fn cmd_shell(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    let tmux_name = format!("pit-shell-{}", t.name);

    if !tmux::session_exists(&tmux_name) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        tmux::create_session_with_cmd(&tmux_name, &t.worktree, &shell)?;
    }

    tmux::attach(&tmux_name)?;
    Ok(())
}

fn cmd_stop(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    if t.status != task::Status::Running {
        println!("Task '{}' is not running (status: {})", name, t.status);
        return Ok(());
    }

    if let Some(ref tmux_name) = t.tmux_session {
        tmux::kill_session(tmux_name)?;
    }

    task::set_status(&project.db, t.id, &task::Status::Idle)?;
    println!("Stopped task '{}'", name);
    Ok(())
}

fn cmd_diff(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    // Get the main branch name
    let main_branch = get_main_branch(&project.repo_root)?;

    // Show diff between main and the task's branch
    let output = std::process::Command::new("git")
        .args(["diff", &format!("{}...{}", main_branch, t.branch), "--stat"])
        .current_dir(&project.repo_root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        println!("No changes on branch '{}' vs '{}'", t.branch, main_branch);
    } else {
        print!("{}", stdout);
    }

    // Also show full diff if there are changes
    if !stdout.trim().is_empty() {
        println!(); // blank line between stat and diff
        let output = std::process::Command::new("git")
            .args(["diff", &format!("{}...{}", main_branch, t.branch)])
            .current_dir(&project.repo_root)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()?;

        if !output.success() {
            anyhow::bail!("git diff failed");
        }
    }

    Ok(())
}

fn cmd_checkpoint(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    let worktree = std::path::Path::new(&t.worktree);
    // Capture agent output if running
    let agent_output = t
        .tmux_session
        .as_deref()
        .and_then(|name| tmux::capture_pane(name, 50).ok());
    let idx = core::checkpoint::create(
        &project.repo_root,
        &t.name,
        &t.branch,
        worktree,
        agent_output.as_deref(),
    )?;

    // Show the new checkpoint with its annotation
    let checkpoints = core::checkpoint::list(&project.repo_root, &t.name)?;
    println!("✓ Checkpoint #{} saved for '{}'\n", idx, name);

    if let Some(cp) = checkpoints.iter().find(|c| c.index == idx) {
        if !cp.annotation.is_empty() {
            println!("{}", cp.annotation);
        }
    }

    // Summary of all checkpoints
    println!("All checkpoints:");
    for cp in &checkpoints {
        let marker = if cp.index == idx { " ← new" } else { "" };
        println!(
            "  #{}: {} {}  {}{}",
            cp.index, cp.commit_hash, cp.message, cp.timestamp, marker
        );
    }

    Ok(())
}

fn cmd_rollback(name: &str, target: Option<usize>) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    if t.status == task::Status::Running {
        anyhow::bail!(
            "task '{}' is running — stop it first before rolling back",
            name
        );
    }

    let worktree = std::path::Path::new(&t.worktree);
    let idx = core::checkpoint::rollback(&project.repo_root, &t.name, worktree, target)?;
    println!("✓ Rolled back '{}' to checkpoint #{}", name, idx);

    Ok(())
}

fn cmd_watch(name: &str, num_lines: usize) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    if t.status != task::Status::Running {
        anyhow::bail!("task '{}' is not running (status: {})", name, t.status);
    }

    let tmux_name = t
        .tmux_session
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("task '{}' has no tmux session", name))?;

    // Stream output in a loop until Ctrl-C
    println!("Watching '{}' (Ctrl-C to stop)\n", name);

    loop {
        // Clear screen and move cursor to top
        print!("\x1b[2J\x1b[H");
        println!(
            "\x1b[1;33m{}\x1b[0m  \x1b[32m▶ running\x1b[0m  (Ctrl-C to stop)\n",
            name
        );

        match tmux::capture_pane(tmux_name, num_lines) {
            Ok(output) => {
                print!("{}", output);
            }
            Err(_) => {
                println!("\x1b[31m(session ended)\x1b[0m");
                break;
            }
        }

        // Check if session is still alive
        if !tmux::session_exists(tmux_name) {
            println!("\n\x1b[33m(agent exited)\x1b[0m");
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    Ok(())
}

fn cmd_delete(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;

    // Kill tmux session if running
    if let Some(ref tmux_name) = t.tmux_session {
        tmux::kill_session(tmux_name)?;
    }
    if t.status == task::Status::Running {
        task::set_status(&project.db, t.id, &task::Status::Idle)?;
    }

    task::delete(&project.db, &project.repo_root, t.id)?;
    println!("Deleted task '{}'", name);
    Ok(())
}

fn cmd_config(action: ConfigAction) -> Result<()> {
    use crate::core::config;

    match action {
        ConfigAction::Set { key, value } => {
            config::set(&key, &value)?;
            println!("Set {} = {}", key, mask_secret(&key, &value));
        }
        ConfigAction::Get { key } => match config::get(&key) {
            Some(value) => println!("{} = {}", key, mask_secret(&key, &value)),
            None => println!("{} is not set", key),
        },
        ConfigAction::Unset { key } => {
            config::unset(&key)?;
            println!("Removed {}", key);
        }
        ConfigAction::List => {
            let all = config::list();
            if all.is_empty() {
                println!("No config values set.");
                println!("  Config file: {}", config::config_path().display());
                println!();
                println!("  Example:");
                println!("    pit config set linear.api_key lin_api_...");
                println!("    pit config set github.token ghp_...");
            } else {
                let mut keys: Vec<&String> = all.keys().collect();
                keys.sort();
                for key in keys {
                    println!("{} = {}", key, mask_secret(key, &all[key]));
                }
            }
        }
        ConfigAction::Path => {
            println!("{}", config::config_path().display());
        }
    }
    Ok(())
}

/// Mask sensitive values in output (show first 4 + last 4 chars).
fn mask_secret(key: &str, value: &str) -> String {
    let is_secret = key.contains("key") || key.contains("token") || key.contains("secret");
    if !is_secret || value.len() < 12 {
        return value.to_string();
    }
    let prefix: String = value.chars().take(4).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}...{}", prefix, suffix)
}

fn open_project() -> Result<Project> {
    let cwd = PathBuf::from(".").canonicalize()?;
    Project::find_and_open(&cwd)
}

/// Open existing project, or auto-init if we're in a git repo.
fn open_or_init_project() -> Result<Project> {
    let cwd = PathBuf::from(".").canonicalize()?;
    match Project::find_and_open(&cwd) {
        Ok(p) => Ok(p),
        Err(_) => {
            let repo_root = Project::find_repo_root(&cwd)?;
            eprintln!("Initializing pit in {} ...", repo_root.display());
            Project::init(&repo_root)
        }
    }
}

/// Detect the main branch name.
fn get_main_branch(repo_root: &std::path::Path) -> Result<String> {
    core::git_info::detect_main_branch(repo_root)
}
