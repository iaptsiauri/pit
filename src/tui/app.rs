use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::Duration;

use crate::core::project::Project;
use crate::core::reap;
use crate::core::task::{self, CreateOpts, Task};
use crate::core::tmux;

use super::ui;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    /// New task modal is open.
    NewTask,
}

/// Which field is focused in the new-task modal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModalField {
    Name,
    Prompt,
    Issue,
}

impl ModalField {
    pub fn next(self) -> Self {
        match self {
            ModalField::Name => ModalField::Prompt,
            ModalField::Prompt => ModalField::Issue,
            ModalField::Issue => ModalField::Name,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            ModalField::Name => ModalField::Issue,
            ModalField::Prompt => ModalField::Name,
            ModalField::Issue => ModalField::Prompt,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ModalField::Name => "Task name",
            ModalField::Prompt => "Agent prompt",
            ModalField::Issue => "Issue URL (optional)",
        }
    }
}

/// State for the new-task modal.
#[derive(Debug, Clone)]
pub struct ModalState {
    pub field: ModalField,
    pub name: String,
    pub prompt: String,
    pub issue: String,
}

impl ModalState {
    fn new() -> Self {
        ModalState {
            field: ModalField::Name,
            name: String::new(),
            prompt: String::new(),
            issue: String::new(),
        }
    }

    /// Get a mutable reference to the currently focused field's text.
    fn active_text_mut(&mut self) -> &mut String {
        match self.field {
            ModalField::Name => &mut self.name,
            ModalField::Prompt => &mut self.prompt,
            ModalField::Issue => &mut self.issue,
        }
    }

    /// Get the currently focused field's text.
    pub fn active_text(&self) -> &str {
        match self.field {
            ModalField::Name => &self.name,
            ModalField::Prompt => &self.prompt,
            ModalField::Issue => &self.issue,
        }
    }
}

/// Application state for the TUI.
pub struct App {
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub should_quit: bool,
    pub mode: Mode,
    pub modal: ModalState,
    pub error: Option<String>,
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
            modal: ModalState::new(),
            error: None,
            repo_root: project.repo_root.clone(),
            db_path: project.pit_dir.join("pit.db"),
        })
    }

    /// Refresh tasks from DB. Reaps dead tmux sessions first.
    fn refresh(&mut self) -> Result<()> {
        let db = crate::db::open(&self.db_path)?;
        reap::reap_dead(&db)?;
        self.tasks = task::list(&db)?;
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
            Mode::NewTask => self.handle_modal_key(code, modifiers),
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
                if let Some(t) = self.tasks.get(self.selected) {
                    Ok(Action::Enter(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('b'), _) => {
                if let Some(t) = self.tasks.get(self.selected) {
                    Ok(Action::Background(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('d'), _) => {
                if let Some(t) = self.tasks.get(self.selected) {
                    Ok(Action::Delete(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('n'), _) => {
                self.mode = Mode::NewTask;
                self.modal = ModalState::new();
                Ok(Action::None)
            }
            (KeyCode::Char('r'), _) => {
                self.refresh()?;
                Ok(Action::None)
            }
            _ => Ok(Action::None),
        }
    }

    fn handle_modal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            // Esc → cancel
            (KeyCode::Esc, _) => {
                self.mode = Mode::Normal;
                Ok(Action::None)
            }
            // Tab → next field
            (KeyCode::Tab, _) | (KeyCode::BackTab, KeyModifiers::SHIFT) => {
                // BackTab is Shift+Tab
                self.modal.field = self.modal.field.next();
                Ok(Action::None)
            }
            // Shift+Tab → prev field (BackTab is the actual event)
            (KeyCode::BackTab, _) => {
                self.modal.field = self.modal.field.prev();
                Ok(Action::None)
            }
            // Enter → submit
            (KeyCode::Enter, _) => {
                let name = self.modal.name.trim().to_string();

                // Validate
                if name.is_empty() {
                    self.error = Some("Task name is required".into());
                    self.modal.field = ModalField::Name;
                    return Ok(Action::None);
                }
                if !name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    self.error =
                        Some("Name: only alphanumeric, hyphens, underscores".into());
                    self.modal.field = ModalField::Name;
                    return Ok(Action::None);
                }

                let prompt = self.modal.prompt.trim().to_string();
                let issue = self.modal.issue.trim().to_string();

                self.mode = Mode::Normal;
                Ok(Action::CreateTask {
                    name,
                    prompt,
                    issue_url: issue,
                })
            }
            // Backspace
            (KeyCode::Backspace, _) => {
                self.modal.active_text_mut().pop();
                Ok(Action::None)
            }
            // Regular character
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.modal.active_text_mut().push(c);
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
    CreateTask {
        name: String,
        prompt: String,
        issue_url: String,
    },
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

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }

                let action = app.handle_key(key.code, key.modifiers)?;

                match action {
                    Action::None => {}
                    Action::Enter(task_id) => {
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
                    Action::CreateTask {
                        name,
                        prompt,
                        issue_url,
                    } => {
                        handle_create(app, &name, &prompt, &issue_url)?;
                        app.refresh()?;
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

        app.refresh()?;
    }
}

/// Create a new task from the TUI.
fn handle_create(app: &mut App, name: &str, prompt: &str, issue_url: &str) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    match task::create(
        &db,
        &app.repo_root,
        &CreateOpts {
            name,
            description: "", // description is derived from prompt
            prompt,
            issue_url,
        },
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            app.error = Some(e.to_string());
            Ok(())
        }
    }
}

/// Build the claude command for a task, handling new vs resume sessions.
fn build_claude_cmd(task: &Task) -> (String, String) {
    let session_id = task
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut cmd = if task.session_id.is_some() {
        format!("claude -r {}", session_id)
    } else {
        format!("claude --session-id {}", session_id)
    };

    // If there's a prompt and this is the first launch, pass it
    if task.session_id.is_none() && !task.prompt.is_empty() {
        // Shell-escape the prompt using single quotes
        let escaped = task.prompt.replace('\'', "'\\''");
        cmd.push_str(&format!(" -p '{}'", escaped));
    }

    (cmd, session_id)
}

/// Launch a task in a tmux session (shared by enter + background).
fn launch_task(db: &rusqlite::Connection, task: &Task) -> Result<String> {
    let tmux_name = tmux::session_name(&task.name);

    if tmux::session_exists(&tmux_name) {
        return Ok(tmux_name);
    }

    let (claude_cmd, session_id) = build_claude_cmd(task);

    tmux::create_session(&tmux_name, &task.worktree)?;
    tmux::send_keys(&tmux_name, &[&claude_cmd, "Enter"])?;
    task::set_running(db, task.id, &tmux_name, None, Some(&session_id))?;

    Ok(tmux_name)
}

/// Enter: create tmux session (if needed) and attach.
fn handle_enter(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;

    let tmux_name = launch_task(&db, &task)?;
    tmux::attach(&tmux_name)?;

    if !tmux::session_exists(&tmux_name) {
        task::set_status(&db, task_id, &task::Status::Done)?;
    }

    Ok(())
}

/// Background: create tmux session without attaching.
fn handle_background(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;
    launch_task(&db, &task)?;
    Ok(())
}

/// Delete: kill tmux session and remove task.
fn handle_delete(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;

    if let Some(ref tmux_name) = task.tmux_session {
        tmux::kill_session(tmux_name)?;
    }

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
            modal: ModalState::new(),
            error: None,
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
            prompt: String::new(),
            issue_url: String::new(),
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

    // --- Normal mode tests ---

    #[test]
    fn quit_on_q() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE).unwrap();
        assert!(app.should_quit);
    }

    #[test]
    fn quit_on_ctrl_c() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap();
        assert!(app.should_quit);
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
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 2); // clamped
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0);
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0); // clamped
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
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        let action = app.handle_key(KeyCode::Char('b'), KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Background(1)));
    }

    #[test]
    fn delete_returns_task_id() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        let action = app.handle_key(KeyCode::Char('d'), KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Delete(1)));
    }

    // --- Modal tests ---

    #[test]
    fn n_opens_modal() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert_eq!(app.modal.field, ModalField::Name);
    }

    #[test]
    fn modal_typing() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "fix-bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        assert_eq!(app.modal.name, "fix-bug");

        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.name, "fix-bu");
    }

    #[test]
    fn modal_tab_cycles_fields() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Prompt);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Issue);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name); // wraps
    }

    #[test]
    fn modal_backtab_cycles_backwards() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name);

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Issue);

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Prompt);

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name);
    }

    #[test]
    fn modal_esc_cancels() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "test".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn modal_typing_in_different_fields() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Type name
        for c in "my-task".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        // Tab to prompt
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        for c in "fix the bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        // Tab to issue
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        for c in "https://linear.app/issue/123".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        assert_eq!(app.modal.name, "my-task");
        assert_eq!(app.modal.prompt, "fix the bug");
        assert_eq!(app.modal.issue, "https://linear.app/issue/123");
    }

    #[test]
    fn modal_submit_creates_task() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "my-task".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        for c in "do stuff".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(matches!(
            action,
            Action::CreateTask { ref name, ref prompt, .. }
                if name == "my-task" && prompt == "do stuff"
        ));
    }

    #[test]
    fn modal_empty_name_shows_error() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Submit with empty name
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask); // stays in modal
        assert!(app.error.is_some());
        assert_eq!(app.modal.field, ModalField::Name); // focuses name
    }

    #[test]
    fn modal_invalid_name_shows_error() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "has spaces".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert!(app.error.is_some());
    }

    #[test]
    fn modal_submit_without_prompt_works() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "quick".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            action,
            Action::CreateTask { ref name, ref prompt, .. }
                if name == "quick" && prompt.is_empty()
        ));
    }

    #[test]
    fn modal_submit_with_issue_url() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        for c in "feat".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        // Skip to issue
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        for c in "https://github.com/org/repo/issues/42".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            action,
            Action::CreateTask { ref issue_url, .. }
                if issue_url == "https://github.com/org/repo/issues/42"
        ));
    }
}
