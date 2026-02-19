use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::Duration;

use crate::core::git_info::{self, TaskGitInfo};
use crate::core::names;
use crate::core::project::Project;
use crate::core::reap;
use crate::core::task::{self, CreateOpts, Task};
use crate::core::tmux;

use super::ui;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    NewTask,
}

/// Which pane has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    TaskList,
    Detail,
}

/// Which field is focused in the new-task modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalField {
    Name,
    Prompt,
    Agent,
    Issue,
    AutoApprove,
}

impl ModalField {
    pub const ALL: &[ModalField] = &[
        ModalField::Name,
        ModalField::Prompt,
        ModalField::Agent,
        ModalField::Issue,
        ModalField::AutoApprove,
    ];

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn is_text_input(self) -> bool {
        matches!(self, ModalField::Name | ModalField::Prompt | ModalField::Issue)
    }
}

/// State for the new-task modal.
#[derive(Debug, Clone)]
pub struct ModalState {
    pub field: ModalField,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub issue: String,
    pub auto_approve: bool,
}

const AGENTS: &[&str] = &["claude", "codex", "amp", "aider", "custom"];

impl ModalState {
    fn new(existing_names: &[String]) -> Self {
        ModalState {
            field: ModalField::Name,
            name: names::generate(existing_names),
            prompt: String::new(),
            agent: "claude".to_string(),
            issue: String::new(),
            auto_approve: false,
        }
    }

    fn active_text_mut(&mut self) -> Option<&mut String> {
        match self.field {
            ModalField::Name => Some(&mut self.name),
            ModalField::Prompt => Some(&mut self.prompt),
            ModalField::Issue => Some(&mut self.issue),
            _ => None,
        }
    }

    pub fn active_text(&self) -> &str {
        match self.field {
            ModalField::Name => &self.name,
            ModalField::Prompt => &self.prompt,
            ModalField::Issue => &self.issue,
            _ => "",
        }
    }

    fn cycle_agent(&mut self, forward: bool) {
        let idx = AGENTS.iter().position(|a| *a == self.agent).unwrap_or(0);
        let next = if forward {
            (idx + 1) % AGENTS.len()
        } else {
            (idx + AGENTS.len() - 1) % AGENTS.len()
        };
        self.agent = AGENTS[next].to_string();
    }
}

pub struct App {
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub should_quit: bool,
    pub mode: Mode,
    pub focus: Pane,
    pub modal: ModalState,
    pub error: Option<String>,
    pub repo_root: std::path::PathBuf,
    db_path: std::path::PathBuf,
    /// Cached git info for the currently selected task.
    pub detail: Option<TaskGitInfo>,
    /// Which task id the cached detail belongs to.
    detail_task_id: Option<i64>,
    /// Scroll offset in the detail pane.
    pub detail_scroll: u16,
    /// Which file index is selected in the detail pane (None = no file selected).
    pub file_cursor: Option<usize>,
    /// Which files have their diff expanded.
    pub expanded_files: std::collections::HashSet<usize>,
    /// Cached file diffs (file index → diff lines).
    pub file_diffs: std::collections::HashMap<usize, Vec<String>>,
}

impl App {
    fn new(project: &Project) -> Result<Self> {
        let tasks = task::list(&project.db)?;
        let existing: Vec<String> = tasks.iter().map(|t| t.name.clone()).collect();
        let mut app = App {
            tasks,
            selected: 0,
            should_quit: false,
            mode: Mode::Normal,
            focus: Pane::TaskList,
            modal: ModalState::new(&existing),
            error: None,
            repo_root: project.repo_root.clone(),
            db_path: project.pit_dir.join("pit.db"),
            detail: None,
            detail_task_id: None,
            detail_scroll: 0,
            file_cursor: None,
            expanded_files: std::collections::HashSet::new(),
            file_diffs: std::collections::HashMap::new(),
        };
        app.refresh_detail();
        Ok(app)
    }

    fn refresh(&mut self) -> Result<()> {
        let db = crate::db::open(&self.db_path)?;
        reap::reap_dead(&db)?;
        self.tasks = task::list(&db)?;
        if !self.tasks.is_empty() && self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        }
        self.refresh_detail();
        Ok(())
    }

    /// Update the cached git detail for the currently selected task.
    /// Only re-fetches if the selection changed.
    fn refresh_detail(&mut self) {
        let current_id = self.tasks.get(self.selected).map(|t| t.id);
        if current_id == self.detail_task_id && self.detail.is_some() {
            return;
        }
        self.detail_task_id = current_id;
        self.detail_scroll = 0;
        self.file_cursor = None;
        self.expanded_files.clear();
        self.file_diffs.clear();
        if let Some(task) = self.tasks.get(self.selected) {
            self.detail = Some(git_info::gather(&self.repo_root, &task.branch));
        } else {
            self.detail = None;
        }
    }

    /// Force re-fetch detail (e.g. after a refresh key).
    fn force_refresh_detail(&mut self) {
        self.detail_task_id = None;
        self.detail = None;
        self.refresh_detail();
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        self.error = None;
        match self.mode {
            Mode::Normal => self.handle_normal_key(code, modifiers),
            Mode::NewTask => self.handle_modal_key(code, modifiers),
        }
    }

    fn handle_normal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        // Global keys (work in both panes)
        match (code, modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
                return Ok(Action::None);
            }
            (KeyCode::Char('n'), _) => {
                let existing: Vec<String> = self.tasks.iter().map(|t| t.name.clone()).collect();
                self.modal = ModalState::new(&existing);
                self.mode = Mode::NewTask;
                return Ok(Action::None);
            }
            (KeyCode::Char('r'), _) => {
                self.refresh()?;
                self.force_refresh_detail();
                return Ok(Action::None);
            }
            _ => {}
        }

        // Pane switching
        match (code, modifiers) {
            (KeyCode::Right | KeyCode::Char('l'), _) if self.focus == Pane::TaskList => {
                self.focus = Pane::Detail;
                return Ok(Action::None);
            }
            (KeyCode::Left | KeyCode::Char('h'), _) if self.focus == Pane::Detail => {
                self.focus = Pane::TaskList;
                return Ok(Action::None);
            }
            (KeyCode::Esc, _) if self.focus == Pane::Detail => {
                self.focus = Pane::TaskList;
                return Ok(Action::None);
            }
            _ => {}
        }

        // Pane-specific keys
        match self.focus {
            Pane::TaskList => self.handle_tasklist_key(code, modifiers),
            Pane::Detail => self.handle_detail_key(code, modifiers),
        }
    }

    fn handle_tasklist_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                if self.selected > 0 {
                    self.selected -= 1;
                    self.refresh_detail();
                }
                Ok(Action::None)
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                if !self.tasks.is_empty() && self.selected < self.tasks.len() - 1 {
                    self.selected += 1;
                    self.refresh_detail();
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
            _ => Ok(Action::None),
        }
    }

    /// How many files are in the current detail.
    fn file_count(&self) -> usize {
        self.detail
            .as_ref()
            .map(|d| d.files.len())
            .unwrap_or(0)
    }

    fn handle_detail_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        let nfiles = self.file_count();

        match (code, modifiers) {
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                if nfiles == 0 {
                    return Ok(Action::None);
                }
                match self.file_cursor {
                    None => {
                        self.file_cursor = Some(0);
                    }
                    Some(i) if i + 1 < nfiles => {
                        self.file_cursor = Some(i + 1);
                    }
                    _ => {}
                }
                Ok(Action::None)
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                match self.file_cursor {
                    Some(0) => {
                        self.file_cursor = None;
                    }
                    Some(i) => {
                        self.file_cursor = Some(i - 1);
                    }
                    None => {}
                }
                Ok(Action::None)
            }
            // Enter: toggle file diff expansion
            (KeyCode::Enter, _) => {
                if let Some(idx) = self.file_cursor {
                    if self.expanded_files.contains(&idx) {
                        self.expanded_files.remove(&idx);
                    } else {
                        // Fetch diff if not cached
                        if !self.file_diffs.contains_key(&idx) {
                            if let (Some(task), Some(info)) =
                                (self.tasks.get(self.selected), self.detail.as_ref())
                            {
                                if let Some(file) = info.files.get(idx) {
                                    let diff = git_info::file_diff(
                                        &self.repo_root,
                                        &task.branch,
                                        &file.path,
                                    );
                                    self.file_diffs.insert(idx, diff);
                                }
                            }
                        }
                        self.expanded_files.insert(idx);
                    }
                } else {
                    // No file selected — launch agent
                    if let Some(t) = self.tasks.get(self.selected) {
                        return Ok(Action::Enter(t.id));
                    }
                }
                Ok(Action::None)
            }
            // Scroll the whole pane (for when content overflows)
            (KeyCode::PageDown, _) => {
                self.detail_scroll = self.detail_scroll.saturating_add(10);
                Ok(Action::None)
            }
            (KeyCode::PageUp, _) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(10);
                Ok(Action::None)
            }
            (KeyCode::Home, _) | (KeyCode::Char('g'), _) => {
                self.detail_scroll = 0;
                self.file_cursor = None;
                Ok(Action::None)
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
                self.detail_scroll = u16::MAX;
                if nfiles > 0 {
                    self.file_cursor = Some(nfiles - 1);
                }
                Ok(Action::None)
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
            _ => Ok(Action::None),
        }
    }

    fn handle_modal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = Mode::Normal;
                Ok(Action::None)
            }

            // Tab / Shift-Tab: cycle fields
            (KeyCode::Tab, _) => {
                self.modal.field = self.modal.field.next();
                Ok(Action::None)
            }
            (KeyCode::BackTab, _) => {
                self.modal.field = self.modal.field.prev();
                Ok(Action::None)
            }

            // Enter: submit (from any field)
            (KeyCode::Enter, _) if modifiers.contains(KeyModifiers::CONTROL) || !self.modal.field.is_text_input() || self.modal.field == ModalField::Name => {
                self.try_submit()
            }

            // Enter in prompt/issue: newline NOT supported (single line), so submit
            (KeyCode::Enter, _) => {
                self.try_submit()
            }

            // Agent field: left/right to cycle
            (KeyCode::Left, _) if self.modal.field == ModalField::Agent => {
                self.modal.cycle_agent(false);
                Ok(Action::None)
            }
            (KeyCode::Right, _) if self.modal.field == ModalField::Agent => {
                self.modal.cycle_agent(true);
                Ok(Action::None)
            }

            // Auto-approve: space toggles
            (KeyCode::Char(' '), _) if self.modal.field == ModalField::AutoApprove => {
                self.modal.auto_approve = !self.modal.auto_approve;
                Ok(Action::None)
            }

            // Text input
            (KeyCode::Backspace, _) if self.modal.field.is_text_input() => {
                if let Some(text) = self.modal.active_text_mut() {
                    text.pop();
                }
                Ok(Action::None)
            }
            (KeyCode::Char(c), m) if self.modal.field.is_text_input() && !m.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = self.modal.active_text_mut() {
                    text.push(c);
                }
                Ok(Action::None)
            }

            _ => Ok(Action::None),
        }
    }

    fn try_submit(&mut self) -> Result<Action> {
        let name = self.modal.name.trim().to_string();

        if name.is_empty() {
            self.error = Some("Task name is required".into());
            self.modal.field = ModalField::Name;
            return Ok(Action::None);
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            self.error = Some("Name: only alphanumeric, hyphens, underscores".into());
            self.modal.field = ModalField::Name;
            return Ok(Action::None);
        }

        let prompt = self.modal.prompt.trim().to_string();
        let issue_url = self.modal.issue.trim().to_string();
        let agent = self.modal.agent.clone();

        self.mode = Mode::Normal;
        Ok(Action::CreateTask {
            name,
            prompt,
            issue_url,
            agent,
        })
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
        agent: String,
    },
}

// --- TUI loop ---

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
                        agent,
                    } => {
                        handle_create(app, &name, &prompt, &issue_url, &agent)?;
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

// --- Handlers ---

fn handle_create(app: &mut App, name: &str, prompt: &str, issue_url: &str, agent: &str) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    match task::create(
        &db,
        &app.repo_root,
        &CreateOpts {
            name,
            description: "",
            prompt,
            issue_url,
            agent,
        },
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            app.error = Some(e.to_string());
            Ok(())
        }
    }
}

/// Build the shell command to launch an agent for a task.
/// Returns (command, session_id).
pub fn build_agent_cmd(task: &Task) -> (String, String) {
    let session_id = task
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let is_resume = task.session_id.is_some();
    let escaped_prompt = task.prompt.replace('\'', "'\\''");

    let cmd = match task.agent.as_str() {
        "codex" => {
            // OpenAI Codex CLI: no session resume, prompt is positional
            if !task.prompt.is_empty() {
                format!("codex '{}'", escaped_prompt)
            } else {
                "codex".to_string()
            }
        }
        "aider" => {
            // Aider: message via --message flag
            if !task.prompt.is_empty() {
                format!("aider --message '{}'", escaped_prompt)
            } else {
                "aider".to_string()
            }
        }
        "amp" => {
            // Amp: prompt via --prompt flag
            if !task.prompt.is_empty() {
                format!("amp --prompt '{}'", escaped_prompt)
            } else {
                "amp".to_string()
            }
        }
        "custom" => {
            // Custom: just send the prompt as a bare command, user configures their shell
            if !task.prompt.is_empty() {
                format!("'{}'", escaped_prompt)
            } else {
                "echo 'No agent configured. Type your command.'".to_string()
            }
        }
        // Default: claude (with session resume support)
        _ => {
            let mut c = if is_resume {
                format!("claude -r {}", session_id)
            } else {
                format!("claude --session-id {}", session_id)
            };
            if !is_resume && !task.prompt.is_empty() {
                c.push_str(&format!(" -p '{}'", escaped_prompt));
            }
            c
        }
    };

    (cmd, session_id)
}

fn launch_task(db: &rusqlite::Connection, task: &Task) -> Result<String> {
    let tmux_name = tmux::session_name(&task.name);

    if tmux::session_exists(&tmux_name) {
        return Ok(tmux_name);
    }

    let (agent_cmd, session_id) = build_agent_cmd(task);

    tmux::create_session(&tmux_name, &task.worktree)?;
    tmux::send_keys(&tmux_name, &[&agent_cmd, "Enter"])?;
    task::set_running(db, task.id, &tmux_name, None, Some(&session_id))?;

    Ok(tmux_name)
}

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

fn handle_background(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;
    launch_task(&db, &task)?;
    Ok(())
}

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

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app(tasks: Vec<Task>) -> App {
        let existing: Vec<String> = tasks.iter().map(|t| t.name.clone()).collect();
        App {
            selected: 0,
            should_quit: false,
            mode: Mode::Normal,
            focus: Pane::TaskList,
            modal: ModalState::new(&existing),
            error: None,
            tasks,
            repo_root: std::path::PathBuf::from("/tmp"),
            db_path: std::path::PathBuf::from("/tmp/test.db"),
            detail: None,
            detail_task_id: None,
            detail_scroll: 0,
            file_cursor: None,
            expanded_files: std::collections::HashSet::new(),
            file_diffs: std::collections::HashMap::new(),
        }
    }

    fn make_app_with_files(tasks: Vec<Task>, files: Vec<git_info::FileStat>) -> App {
        let mut app = make_app(tasks);
        app.detail = Some(TaskGitInfo {
            commits: vec![],
            files,
            total_insertions: 0,
            total_deletions: 0,
        });
        app
    }

    fn make_task(id: i64, name: &str, status: task::Status) -> Task {
        Task {
            id,
            name: name.to_string(),
            description: String::new(),
            prompt: String::new(),
            issue_url: String::new(),
            agent: "claude".to_string(),
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

    // --- Normal mode ---

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
        assert_eq!(app.selected, 2);
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0);
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn enter_returns_task_id() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle), make_task(2, "b", task::Status::Idle)]);
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

    // --- Modal ---

    #[test]
    fn n_opens_modal_with_generated_name() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert_eq!(app.modal.field, ModalField::Name);
        // Name should be auto-generated (adjective-noun)
        assert!(app.modal.name.contains('-'), "expected generated name, got: {}", app.modal.name);
        assert!(!app.modal.name.is_empty());
    }

    #[test]
    fn modal_typing_replaces_generated_name() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Clear the generated name
        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
        }

        for c in "fix-bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        assert_eq!(app.modal.name, "fix-bug");
    }

    #[test]
    fn modal_tab_cycles_all_fields() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Prompt);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Agent);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Issue);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::AutoApprove);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name); // wraps
    }

    #[test]
    fn modal_backtab_cycles_backwards() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::AutoApprove);

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Issue);
    }

    #[test]
    fn modal_agent_cycle() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Go to agent field
        app.modal.field = ModalField::Agent;
        assert_eq!(app.modal.agent, "claude");

        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.agent, "codex");

        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.agent, "amp");

        app.handle_key(KeyCode::Left, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.agent, "codex");
    }

    #[test]
    fn modal_auto_approve_toggle() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        app.modal.field = ModalField::AutoApprove;
        assert!(!app.modal.auto_approve);

        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE).unwrap();
        assert!(app.modal.auto_approve);

        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE).unwrap();
        assert!(!app.modal.auto_approve);
    }

    #[test]
    fn modal_esc_cancels() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn modal_submit_full_flow() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Clear generated name, type custom
        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
        }
        for c in "my-task".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        // Tab to prompt
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        for c in "fix the bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        // Submit
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(matches!(
            action,
            Action::CreateTask { ref name, ref prompt, .. }
                if name == "my-task" && prompt == "fix the bug"
        ));
    }

    #[test]
    fn modal_submit_with_generated_name() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        let generated = app.modal.name.clone();
        assert!(!generated.is_empty());

        // Submit without changing name
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            action,
            Action::CreateTask { ref name, .. } if *name == generated
        ));
    }

    #[test]
    fn modal_empty_name_shows_error() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Clear the generated name
        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert!(app.error.is_some());
    }

    #[test]
    fn modal_invalid_name_shows_error() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
        }
        for c in "has spaces".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert!(app.error.is_some());
    }

    #[test]
    fn modal_typing_in_prompt_field() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Prompt);

        for c in "refactor the API layer".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        assert_eq!(app.modal.prompt, "refactor the API layer");
    }

    #[test]
    fn modal_typing_in_issue_field() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        app.modal.field = ModalField::Issue;
        for c in "https://github.com/org/repo/issues/42".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE).unwrap();
        }
        assert_eq!(app.modal.issue, "https://github.com/org/repo/issues/42");
    }

    #[test]
    fn modal_submit_includes_agent() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        // Cycle to codex
        app.modal.field = ModalField::Agent;
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.agent, "codex");

        // Submit
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            action,
            Action::CreateTask { ref agent, .. } if agent == "codex"
        ));
    }

    #[test]
    fn modal_submit_default_agent_is_claude() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            action,
            Action::CreateTask { ref agent, .. } if agent == "claude"
        ));
    }

    // --- build_agent_cmd ---

    fn make_task_with_agent(agent: &str, prompt: &str, session_id: Option<&str>) -> Task {
        Task {
            id: 1,
            name: "test".to_string(),
            description: String::new(),
            prompt: prompt.to_string(),
            issue_url: String::new(),
            agent: agent.to_string(),
            branch: "pit/test".to_string(),
            worktree: "/tmp/wt/test".to_string(),
            status: task::Status::Idle,
            session_id: session_id.map(|s| s.to_string()),
            tmux_session: None,
            pid: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn agent_cmd_claude_new_session() {
        let task = make_task_with_agent("claude", "fix bug", None);
        let (cmd, session_id) = build_agent_cmd(&task);
        assert!(cmd.starts_with("claude --session-id "), "got: {}", cmd);
        assert!(cmd.contains(&session_id));
        assert!(cmd.contains("-p 'fix bug'"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_claude_resume_session() {
        let task = make_task_with_agent("claude", "fix bug", Some("sess-123"));
        let (cmd, session_id) = build_agent_cmd(&task);
        assert_eq!(session_id, "sess-123");
        assert_eq!(cmd, "claude -r sess-123");
        // Resume should NOT include the prompt
        assert!(!cmd.contains("-p"), "resume should not include prompt, got: {}", cmd);
    }

    #[test]
    fn agent_cmd_claude_no_prompt() {
        let task = make_task_with_agent("claude", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.starts_with("claude --session-id "));
        assert!(!cmd.contains("-p"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_codex_with_prompt() {
        let task = make_task_with_agent("codex", "refactor API", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "codex 'refactor API'");
    }

    #[test]
    fn agent_cmd_codex_no_prompt() {
        let task = make_task_with_agent("codex", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "codex");
    }

    #[test]
    fn agent_cmd_aider_with_prompt() {
        let task = make_task_with_agent("aider", "add tests", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "aider --message 'add tests'");
    }

    #[test]
    fn agent_cmd_aider_no_prompt() {
        let task = make_task_with_agent("aider", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "aider");
    }

    #[test]
    fn agent_cmd_amp_with_prompt() {
        let task = make_task_with_agent("amp", "fix login", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "amp --prompt 'fix login'");
    }

    #[test]
    fn agent_cmd_amp_no_prompt() {
        let task = make_task_with_agent("amp", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "amp");
    }

    #[test]
    fn agent_cmd_custom_with_prompt() {
        let task = make_task_with_agent("custom", "my-script --flag", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "'my-script --flag'");
    }

    #[test]
    fn agent_cmd_custom_no_prompt() {
        let task = make_task_with_agent("custom", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.contains("No agent configured"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_unknown_falls_back_to_claude() {
        let task = make_task_with_agent("mystery-agent", "do stuff", None);
        let (cmd, _) = build_agent_cmd(&task);
        // Unknown agents fall through to the claude default
        assert!(cmd.starts_with("claude --session-id "), "got: {}", cmd);
        assert!(cmd.contains("-p 'do stuff'"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_escapes_single_quotes() {
        let task = make_task_with_agent("codex", "fix the user's login", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.contains("'fix the user'\\''s login'"), "got: {}", cmd);
    }

    // --- Pane focus ---

    #[test]
    fn starts_with_tasklist_focus() {
        let app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        assert_eq!(app.focus, Pane::TaskList);
    }

    #[test]
    fn l_moves_to_detail_pane() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::Detail);
    }

    #[test]
    fn right_arrow_moves_to_detail_pane() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::Detail);
    }

    #[test]
    fn h_moves_back_to_tasklist() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::TaskList);
    }

    #[test]
    fn left_arrow_moves_back_to_tasklist() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;
        app.handle_key(KeyCode::Left, KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::TaskList);
    }

    #[test]
    fn esc_from_detail_returns_to_tasklist() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::TaskList);
    }

    // --- File navigation in detail pane ---

    fn sample_files() -> Vec<git_info::FileStat> {
        vec![
            git_info::FileStat { path: "src/main.rs".into(), insertions: 10, deletions: 2 },
            git_info::FileStat { path: "src/lib.rs".into(), insertions: 5, deletions: 0 },
            git_info::FileStat { path: "Cargo.toml".into(), insertions: 1, deletions: 0 },
        ]
    }

    #[test]
    fn jk_in_detail_moves_file_cursor() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            sample_files(),
        );
        app.focus = Pane::Detail;
        assert_eq!(app.file_cursor, None);

        // First j: selects first file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(0));

        // Second j: moves to second file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(1));

        // Third j: moves to third file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(2));

        // Fourth j: stays at last file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(2));

        // k: moves back
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(1));

        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(0));

        // k from first file: deselects (back to None)
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, None);

        // k from None: stays None
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, None);
    }

    #[test]
    fn jk_in_detail_no_files_is_noop() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            vec![],
        );
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, None);
    }

    #[test]
    fn enter_on_file_toggles_expansion() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            sample_files(),
        );
        app.focus = Pane::Detail;

        // Navigate to first file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(0));
        assert!(!app.expanded_files.contains(&0));

        // Enter: expand
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(app.expanded_files.contains(&0));

        // Enter again: collapse
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(!app.expanded_files.contains(&0));
    }

    #[test]
    fn enter_with_no_file_selected_launches_agent() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            sample_files(),
        );
        app.focus = Pane::Detail;
        assert_eq!(app.file_cursor, None);

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Enter(1)));
    }

    #[test]
    fn multiple_files_can_be_expanded() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            sample_files(),
        );
        app.focus = Pane::Detail;

        // Expand file 0
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(app.expanded_files.contains(&0));

        // Move to file 1 and expand
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(app.expanded_files.contains(&0));
        assert!(app.expanded_files.contains(&1));
    }

    #[test]
    fn g_resets_file_cursor() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            sample_files(),
        );
        app.focus = Pane::Detail;
        app.file_cursor = Some(2);

        app.handle_key(KeyCode::Char('g'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, None);
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn G_selects_last_file() {
        let mut app = make_app_with_files(
            vec![make_task(1, "a", task::Status::Idle)],
            sample_files(),
        );
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('G'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, Some(2)); // last of 3 files
    }

    #[test]
    fn jk_in_tasklist_moves_selection_not_file_cursor() {
        let mut app = make_app(vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Idle),
        ]);
        assert_eq!(app.focus, Pane::TaskList);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);
        assert_eq!(app.file_cursor, None);
    }

    #[test]
    fn page_down_up_in_detail() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::PageDown, KeyModifiers::NONE).unwrap();
        assert_eq!(app.detail_scroll, 10);

        app.handle_key(KeyCode::PageDown, KeyModifiers::NONE).unwrap();
        assert_eq!(app.detail_scroll, 20);

        app.handle_key(KeyCode::PageUp, KeyModifiers::NONE).unwrap();
        assert_eq!(app.detail_scroll, 10);
    }

    #[test]
    fn enter_works_from_detail_pane_no_files() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Enter(1)));
    }

    #[test]
    fn n_opens_modal_from_detail_pane() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
    }

    #[test]
    fn q_quits_from_detail_pane() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE).unwrap();
        assert!(app.should_quit);
    }

    #[test]
    fn h_in_tasklist_is_noop() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        assert_eq!(app.focus, Pane::TaskList);

        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::TaskList); // stays in task list
    }

    #[test]
    fn l_in_detail_is_noop() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::Detail); // stays in detail
    }
}
