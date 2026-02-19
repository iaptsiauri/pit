use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::Duration;

use crate::core::project::Project;
use crate::core::task::{self, Task};

use super::ui;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    /// Typing a new task name.
    InputName,
    /// Typing a description for the new task.
    InputDescription,
}

/// Application state for the TUI.
pub struct App {
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub should_quit: bool,
    pub mode: Mode,
    pub input: String,
    pub input_label: &'static str,
    pub error: Option<String>,
    /// Stashed task name while entering description.
    pending_name: Option<String>,
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
            mode: Mode::Normal,
            input: String::new(),
            input_label: "",
            error: None,
            pending_name: None,
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
        // Clear error on any keypress
        self.error = None;

        match self.mode {
            Mode::Normal => self.handle_normal_key(code, modifiers),
            Mode::InputName => self.handle_input_name_key(code, modifiers),
            Mode::InputDescription => self.handle_input_desc_key(code, modifiers),
        }
    }

    fn handle_normal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
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
            (KeyCode::Char('n'), _) => {
                self.mode = Mode::InputName;
                self.input.clear();
                self.input_label = "Task name";
                Ok(Action::None)
            }
            (KeyCode::Char('r'), _) => {
                self.refresh()?;
                Ok(Action::None)
            }
            _ => Ok(Action::None),
        }
    }

    fn handle_input_name_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = Mode::Normal;
                self.input.clear();
                Ok(Action::None)
            }
            (KeyCode::Enter, _) => {
                let name = self.input.trim().to_string();
                if name.is_empty() {
                    self.mode = Mode::Normal;
                    self.input.clear();
                    return Ok(Action::None);
                }
                // Validate name before moving to description
                if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    self.error = Some("Name: only alphanumeric, hyphens, underscores".into());
                    return Ok(Action::None);
                }
                self.pending_name = Some(name);
                self.input.clear();
                self.mode = Mode::InputDescription;
                self.input_label = "Description (optional, Enter to skip)";
                Ok(Action::None)
            }
            (KeyCode::Backspace, _) => {
                self.input.pop();
                Ok(Action::None)
            }
            (KeyCode::Char(c), _) if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                Ok(Action::None)
            }
            _ => Ok(Action::None),
        }
    }

    fn handle_input_desc_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.pending_name = None;
                Ok(Action::None)
            }
            (KeyCode::Enter, _) => {
                let description = self.input.trim().to_string();
                let name = self.pending_name.take().unwrap_or_default();
                self.input.clear();
                self.mode = Mode::Normal;
                Ok(Action::CreateTask(name, description))
            }
            (KeyCode::Backspace, _) => {
                self.input.pop();
                Ok(Action::None)
            }
            (KeyCode::Char(c), _) if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                Ok(Action::None)
            }
            _ => Ok(Action::None),
        }
    }
}

#[derive(Debug)]
pub enum Action {
    None,
    Enter(i64),
    Background(i64),
    Delete(i64),
    CreateTask(String, String),
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
                    Action::CreateTask(name, description) => {
                        handle_create(app, &name, &description)?;
                        app.refresh()?;
                        // Select the newly created task (last in list)
                        if !app.tasks.is_empty() {
                            app.selected = app.tasks.len() - 1;
                        }
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

/// Create a new task from the TUI.
fn handle_create(app: &mut App, name: &str, description: &str) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    match task::create(&db, &app.repo_root, name, description) {
        Ok(_) => Ok(()),
        Err(e) => {
            app.error = Some(e.to_string());
            Ok(())
        }
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
            mode: Mode::Normal,
            input: String::new(),
            input_label: "",
            error: None,
            pending_name: None,
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

        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 2);

        // Can't go past end
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 2);

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

    #[test]
    fn n_enters_input_mode() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::InputName);
        assert_eq!(app.input_label, "Task name");
    }

    #[test]
    fn input_mode_typing_and_backspace() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Type "fix-bug"
        for c in "fix-bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        assert_eq!(app.input, "fix-bug");

        // Backspace
        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
        assert_eq!(app.input, "fix-bu");
    }

    #[test]
    fn input_mode_esc_cancels() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "test".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.input.is_empty());
    }

    #[test]
    fn input_name_then_description_creates_task() {
        let mut app = make_app(vec![]);

        // Press n
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::InputName);

        // Type name
        for c in "my-task".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        // Enter → moves to description
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::InputDescription);
        assert!(app.input.is_empty());

        // Type description
        for c in "do stuff".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        // Enter → creates task
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(matches!(action, Action::CreateTask(name, desc) if name == "my-task" && desc == "do stuff"));
    }

    #[test]
    fn input_empty_name_cancels() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        // Enter with empty input → back to normal
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn input_invalid_name_shows_error() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "has spaces".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::InputName); // stays in input mode
        assert!(app.error.is_some());
    }

    #[test]
    fn esc_during_description_cancels() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "task".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::InputDescription);

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pending_name.is_none());
    }

    #[test]
    fn skip_description_with_enter() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "quick".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();

        // Empty description, just press Enter
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::CreateTask(name, desc) if name == "quick" && desc.is_empty()));
    }
}
