use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::Duration;

use crate::core::project::Project;
use crate::core::task::{self, Task};

use super::ui;

/// Application state for the TUI.
pub struct App {
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub should_quit: bool,
    pub repo_root: std::path::PathBuf,
    db_path: std::path::PathBuf,
}

impl App {
    fn new(project: &Project) -> Result<Self> {
        let tasks = task::list(&project.db)?;
        Ok(App {
            tasks,
            selected: 0,
            should_quit: false,
            repo_root: project.repo_root.clone(),
            db_path: project.pit_dir.join("pit.db"),
        })
    }

    /// Refresh tasks from DB (re-opens connection each time for freshness).
    fn refresh(&mut self) -> Result<()> {
        let db = crate::db::open(&self.db_path)?;
        self.tasks = task::list(&db)?;
        // Keep selection in bounds
        if !self.tasks.is_empty() && self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        }
        Ok(())
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
                Ok(Action::None)
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                Ok(Action::None)
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                if !self.tasks.is_empty() && self.selected < self.tasks.len() - 1 {
                    self.selected += 1;
                }
                Ok(Action::None)
            }
            (KeyCode::Enter, _) => {
                if let Some(task) = self.tasks.get(self.selected) {
                    Ok(Action::Enter(task.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('b'), _) => {
                if let Some(task) = self.tasks.get(self.selected) {
                    Ok(Action::Background(task.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('d'), _) => {
                if let Some(task) = self.tasks.get(self.selected) {
                    Ok(Action::Delete(task.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('r'), _) => {
                self.refresh()?;
                Ok(Action::None)
            }
            _ => Ok(Action::None),
        }
    }
}

#[derive(Debug)]
enum Action {
    None,
    Enter(i64),
    Background(i64),
    Delete(i64),
}

/// Run the TUI dashboard. Returns when user quits.
pub fn run(project: &Project) -> Result<()> {
    let mut app = App::new(project)?;

    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Poll with timeout so we can refresh periodically
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }

                let action = app.handle_key(key.code, key.modifiers)?;

                match action {
                    Action::None => {}
                    Action::Enter(task_id) => {
                        // Suspend TUI, launch tmux, restore TUI
                        ratatui::restore();
                        handle_enter(app, task_id)?;
                        *terminal = ratatui::init();
                        app.refresh()?;
                    }
                    Action::Background(task_id) => {
                        handle_background(app, task_id)?;
                        app.refresh()?;
                    }
                    Action::Delete(task_id) => {
                        handle_delete(app, task_id)?;
                        app.refresh()?;
                    }
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }

        // Auto-refresh every loop iteration to catch status changes
        // (cheap — just reads from SQLite)
        app.refresh()?;
    }
}

/// Enter: create tmux session (if needed) and attach.
fn handle_enter(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;

    let tmux_name = format!("pit-{}", task.name);

    // Check if tmux session already exists
    let session_exists = std::process::Command::new("tmux")
        .args(["has-session", "-t", &tmux_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !session_exists {
        // Create new tmux session
        let session_id = task
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Build the claude command
        let claude_cmd = if task.session_id.is_some() {
            format!("claude -r {}", session_id)
        } else {
            format!("claude --session-id {}", session_id)
        };

        std::process::Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &tmux_name,
                "-c",
                &task.worktree,
            ])
            .output()?;

        std::process::Command::new("tmux")
            .args(["send-keys", "-t", &tmux_name, &claude_cmd, "Enter"])
            .output()?;

        // Store session info
        task::set_running(&db, task_id, &tmux_name, None, Some(&session_id))?;
    }

    // Attach to the tmux session
    std::process::Command::new("tmux")
        .args(["attach", "-t", &tmux_name])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    // After detach, check if tmux session is still alive
    let still_running = std::process::Command::new("tmux")
        .args(["has-session", "-t", &tmux_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !still_running {
        task::set_status(&db, task_id, &task::Status::Done)?;
    }

    Ok(())
}

/// Background: create tmux session without attaching.
fn handle_background(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;

    let tmux_name = format!("pit-{}", task.name);

    // Check if already running
    let session_exists = std::process::Command::new("tmux")
        .args(["has-session", "-t", &tmux_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if session_exists {
        return Ok(()); // Already running
    }

    let session_id = task
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let claude_cmd = if task.session_id.is_some() {
        format!("claude -r {}", session_id)
    } else {
        format!("claude --session-id {}", session_id)
    };

    std::process::Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &tmux_name,
            "-c",
            &task.worktree,
        ])
        .output()?;

    std::process::Command::new("tmux")
        .args(["send-keys", "-t", &tmux_name, &claude_cmd, "Enter"])
        .output()?;

    task::set_running(&db, task_id, &tmux_name, None, Some(&session_id))?;

    Ok(())
}

/// Delete: confirm and remove task.
fn handle_delete(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;

    // Kill tmux session if running
    if let Some(ref tmux_name) = task.tmux_session {
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", tmux_name])
            .output();
    }

    // If running, stop it first
    if task.status == task::Status::Running {
        task::set_status(&db, task_id, &task::Status::Idle)?;
    }

    task::delete(&db, &app.repo_root, task_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app(tasks: Vec<Task>) -> App {
        App {
            selected: 0,
            should_quit: false,
            tasks,
            repo_root: std::path::PathBuf::from("/tmp"),
            db_path: std::path::PathBuf::from("/tmp/test.db"),
        }
    }

    fn make_task(id: i64, name: &str, status: task::Status) -> Task {
        Task {
            id,
            name: name.to_string(),
            description: String::new(),
            branch: format!("pit/{}", name),
            worktree: format!("/tmp/wt/{}", name),
            status,
            session_id: None,
            tmux_session: None,
            pid: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn quit_on_q() {
        let mut app = make_app(vec![]);
        let action = app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE).unwrap();
        assert!(app.should_quit);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn quit_on_ctrl_c() {
        let mut app = make_app(vec![]);
        let action = app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap();
        assert!(app.should_quit);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn navigate_up_down() {
        let tasks = vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Idle),
            make_task(3, "c", task::Status::Idle),
        ];
        let mut app = make_app(tasks);

        // Start at 0, move down
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 2);

        // Can't go past end
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 2);

        // Move up
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);

        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0);

        // Can't go before 0
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn enter_returns_task_id() {
        let tasks = vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Idle),
        ];
        let mut app = make_app(tasks);
        app.selected = 1;

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Enter(2)));
    }

    #[test]
    fn enter_on_empty_is_noop() {
        let mut app = make_app(vec![]);
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn background_returns_task_id() {
        let tasks = vec![make_task(1, "a", task::Status::Idle)];
        let mut app = make_app(tasks);

        let action = app.handle_key(KeyCode::Char('b'), KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Background(1)));
    }

    #[test]
    fn delete_returns_task_id() {
        let tasks = vec![make_task(1, "a", task::Status::Idle)];
        let mut app = make_app(tasks);

        let action = app.handle_key(KeyCode::Char('d'), KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Delete(1)));
    }
}
