mod core;
mod db;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::core::project::Project;
use crate::core::task;

#[derive(Parser)]
#[command(name = "pit", version, about = "Run multiple coding agents in parallel")]
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
    },

    /// List all tasks
    #[command(alias = "ls")]
    List,

    /// Delete a task (removes worktree and branch)
    #[command(alias = "rm")]
    Delete {
        /// Task name
        name: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => cmd_dashboard()?,
        Some(Commands::Init) => cmd_init()?,
        Some(Commands::New { name, description }) => cmd_new(&name, &description)?,
        Some(Commands::List) => cmd_list()?,
        Some(Commands::Delete { name }) => cmd_delete(&name)?,
    }

    Ok(())
}

fn cmd_dashboard() -> Result<()> {
    let project = open_project()?;
    tui::run(&project)
}

fn cmd_init() -> Result<()> {
    let cwd = PathBuf::from(".");
    let repo_root = Project::find_repo_root(&cwd.canonicalize()?)?;
    let project = Project::init(&repo_root)?;
    println!("Initialized pit in {}", project.repo_root.display());
    Ok(())
}

fn cmd_new(name: &str, description: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::create(&project.db, &project.repo_root, name, description)?;
    println!("Created task '{}' on branch '{}'", t.name, t.branch);
    println!("  worktree: {}", t.worktree);
    Ok(())
}

fn cmd_list() -> Result<()> {
    let project = open_project()?;
    let tasks = task::list(&project.db)?;

    if tasks.is_empty() {
        println!("No tasks. Create one with: pit new <name>");
        return Ok(());
    }

    println!("{:<4} {:<20} {:<10} {}", "ID", "NAME", "STATUS", "BRANCH");
    println!("{}", "-".repeat(60));
    for t in &tasks {
        println!("{:<4} {:<20} {:<10} {}", t.id, t.name, t.status, t.branch);
    }
    println!("\n{} task(s)", tasks.len());
    Ok(())
}

fn cmd_delete(name: &str) -> Result<()> {
    let project = open_project()?;
    let t = task::get_by_name(&project.db, name)?
        .ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;
    task::delete(&project.db, &project.repo_root, t.id)?;
    println!("Deleted task '{}'", name);
    Ok(())
}

fn open_project() -> Result<Project> {
    let cwd = PathBuf::from(".").canonicalize()?;
    Project::find_and_open(&cwd)
}
