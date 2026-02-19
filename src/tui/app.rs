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
    IssuePicker,
}

/// Which view layout is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Split pane: task list (left) + detail (right)
    List,
    /// Kanban: three columns (Idle, Running, Done)
    Kanban,
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
        ModalField::AutoApprove,
        ModalField::Issue,
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
        matches!(self, ModalField::Name | ModalField::Prompt)
    }
}

/// State for the new-task modal.
#[derive(Debug, Clone)]
pub struct ModalState {
    pub field: ModalField,
    pub name: String,
    pub prompt: String,
    /// Cursor position within the prompt string (byte offset)
    pub prompt_cursor: usize,
    /// Scroll offset for the prompt textarea (first visible line)
    pub prompt_scroll: usize,
    pub agent: String,
    pub issue: String,
    pub auto_approve: bool,
    /// Status message after fetching an issue (e.g. "✓ ENG-42: Fix login" or "✗ not found")
    pub issue_status: Option<String>,
    /// Linear issue picker state
    pub picker_query: String,
    pub picker_results: Vec<crate::core::linear::LinearIssue>,
    pub picker_selected: usize,
    pub picker_status: Option<String>,
}

const AGENTS: &[&str] = &["claude", "pi", "codex", "amp", "aider", "goose", "custom"];

impl ModalState {
    fn new(existing_names: &[String]) -> Self {
        ModalState {
            field: ModalField::Name,
            name: names::generate(existing_names),
            prompt: String::new(),
            prompt_cursor: 0,
            prompt_scroll: 0,
            agent: "claude".to_string(),
            issue: String::new(),
            auto_approve: false,
            issue_status: None,
            picker_query: String::new(),
            picker_results: Vec::new(),
            picker_selected: 0,
            picker_status: None,
        }
    }

    fn active_text_mut(&mut self) -> Option<&mut String> {
        match self.field {
            ModalField::Name => Some(&mut self.name),
            ModalField::Prompt => Some(&mut self.prompt),
            _ => None,
        }
    }

    /// Update prompt_scroll so cursor stays visible in the textarea.
    pub fn update_prompt_scroll(&mut self) {
        let (cursor_row, _) = crate::tui::ui::cursor_pos_in_wrapped(
            &self.prompt,
            self.prompt_cursor,
            crate::tui::ui::PROMPT_TEXT_WIDTH,
        );
        let vl = crate::tui::ui::PROMPT_VISIBLE_LINES;
        if cursor_row < self.prompt_scroll {
            self.prompt_scroll = cursor_row;
        } else if cursor_row >= self.prompt_scroll + vl {
            self.prompt_scroll = cursor_row.saturating_sub(vl - 1);
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
    pub view: View,
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
    /// When inside an expanded diff, which line is highlighted (0-indexed into diff lines).
    /// None = cursor is on the file header, not inside the diff.
    pub diff_line: Option<usize>,
    /// Which files have their diff expanded.
    pub expanded_files: std::collections::HashSet<usize>,
    /// Cached file diffs (file index → diff lines).
    pub file_diffs: std::collections::HashMap<usize, Vec<String>>,
    /// Kanban: which column is focused (0=Idle, 1=Running, 2=Done)
    pub kanban_col: usize,
    /// Kanban: selected row within each column
    pub kanban_row: [usize; 3],
    /// Cached detail pane inner height (set during render).
    pub detail_pane_height: u16,
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
            view: View::List,
            focus: Pane::TaskList,
            modal: ModalState::new(&existing),
            error: None,
            repo_root: project.repo_root.clone(),
            db_path: project.pit_dir.join("pit.db"),
            detail: None,
            detail_task_id: None,
            detail_scroll: 0,
            file_cursor: None,
            diff_line: None,
            expanded_files: std::collections::HashSet::new(),
            file_diffs: std::collections::HashMap::new(),
            kanban_col: 0,
            kanban_row: [0, 0, 0],
            detail_pane_height: 30,
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
        self.diff_line = None;
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
            Mode::NewTask => {
                let result = self.handle_modal_key(code, modifiers);
                self.modal.update_prompt_scroll();
                result
            }
            Mode::IssuePicker => {
                let result = self.handle_picker_key(code, modifiers);
                // Update scroll in case picker filled the prompt
                if self.mode == Mode::NewTask {
                    self.modal.update_prompt_scroll();
                }
                result
            }
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
            (KeyCode::Char('v'), _) => {
                self.view = match self.view {
                    View::List => View::Kanban,
                    View::Kanban => View::List,
                };
                return Ok(Action::None);
            }
            _ => {}
        }

        match self.view {
            View::List => self.handle_list_view_key(code, modifiers),
            View::Kanban => self.handle_kanban_key(code, modifiers),
        }
    }

    fn handle_list_view_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
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
                if self.diff_line.is_some() {
                    self.diff_line = None;
                } else if let Some(idx) = self.file_cursor {
                    if self.expanded_files.contains(&idx) {
                        self.expanded_files.remove(&idx);
                    } else {
                        self.file_cursor = None;
                    }
                } else {
                    self.focus = Pane::TaskList;
                }
                return Ok(Action::None);
            }
            _ => {}
        }

        match self.focus {
            Pane::TaskList => self.handle_tasklist_key(code, modifiers),
            Pane::Detail => {
                let result = self.handle_detail_key(code, modifiers);
                // Auto-scroll only when cursor is on a file/diff line
                // (PageDown/PageUp do manual scroll without cursor)
                if self.file_cursor.is_some() {
                    self.scroll_detail_to_cursor(self.detail_pane_height);
                }
                result
            }
        }
    }

    /// Get tasks for a kanban column (0=Idle, 1=Running, 2=Done).
    pub fn kanban_column_tasks(&self, col: usize) -> Vec<&Task> {
        let status = match col {
            0 => task::Status::Idle,
            1 => task::Status::Running,
            _ => task::Status::Done,
        };
        self.tasks.iter().filter(|t| t.status == status).collect()
    }

    /// Get the selected task in the current kanban column.
    fn kanban_selected_task(&self) -> Option<&Task> {
        let tasks = self.kanban_column_tasks(self.kanban_col);
        tasks.get(self.kanban_row[self.kanban_col]).copied()
    }

    fn handle_kanban_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            // Navigate within column
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                if self.kanban_row[self.kanban_col] > 0 {
                    self.kanban_row[self.kanban_col] -= 1;
                }
                Ok(Action::None)
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                let count = self.kanban_column_tasks(self.kanban_col).len();
                if count > 0 && self.kanban_row[self.kanban_col] < count - 1 {
                    self.kanban_row[self.kanban_col] += 1;
                }
                Ok(Action::None)
            }
            // Navigate between columns
            (KeyCode::Left | KeyCode::Char('h'), _) => {
                if self.kanban_col > 0 {
                    self.kanban_col -= 1;
                }
                Ok(Action::None)
            }
            (KeyCode::Right | KeyCode::Char('l'), _) => {
                if self.kanban_col < 2 {
                    self.kanban_col += 1;
                }
                Ok(Action::None)
            }
            // Actions on selected task
            (KeyCode::Enter, _) => {
                if let Some(t) = self.kanban_selected_task() {
                    Ok(Action::Enter(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('t'), _) => {
                if let Some(t) = self.kanban_selected_task() {
                    Ok(Action::Shell(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('b'), _) => {
                if let Some(t) = self.kanban_selected_task() {
                    Ok(Action::Background(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            (KeyCode::Char('d'), _) => {
                if let Some(t) = self.kanban_selected_task() {
                    Ok(Action::Delete(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            _ => Ok(Action::None),
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
            (KeyCode::Char('t'), _) => {
                if let Some(t) = self.tasks.get(self.selected) {
                    Ok(Action::Shell(t.id))
                } else {
                    Ok(Action::None)
                }
            }
            _ => Ok(Action::None),
        }
    }

    /// How many files are in the current detail.
    fn file_count(&self) -> usize {
        self.detail.as_ref().map(|d| d.files.len()).unwrap_or(0)
    }

    /// How many diff lines the currently selected file has (if expanded).
    fn current_diff_len(&self) -> usize {
        if let Some(idx) = self.file_cursor {
            if self.expanded_files.contains(&idx) {
                return self.file_diffs.get(&idx).map(|d| d.len()).unwrap_or(0);
            }
        }
        0
    }

    /// Ensure a file's diff is fetched and cached.
    fn ensure_diff_cached(&mut self, idx: usize) {
        if self.file_diffs.contains_key(&idx) {
            return;
        }
        if let (Some(task), Some(info)) = (self.tasks.get(self.selected), self.detail.as_ref()) {
            if let Some(file) = info.files.get(idx) {
                let diff = git_info::file_diff(&self.repo_root, &task.branch, &file.path);
                self.file_diffs.insert(idx, diff);
            }
        }
    }

    /// Compute the visual line (within the detail pane content) where the
    /// cursor currently sits. Used to auto-scroll the detail pane.
    fn cursor_visual_line(&self) -> usize {
        let info = match self.detail.as_ref() {
            Some(i) => i,
            None => return 0,
        };

        // Count fixed header lines:
        // name, status+agent, branch = 3
        // + prompt (if set) + issue (if set)
        // + blank
        // + commits header + commits + blank
        // + files header
        let task = match self.tasks.get(self.selected) {
            Some(t) => t,
            None => return 0,
        };

        let mut line: usize = 3; // name, status, branch
        if !task.prompt.is_empty() {
            line += 1;
        }
        if !task.issue_url.is_empty() {
            line += 1;
        }
        line += 1; // blank

        // Commits header + commits + blank
        line += 1; // "── Commits (N) ──"
        if info.commits.is_empty() {
            line += 1; // "No commits yet"
        } else {
            line += info.commits.len();
        }
        line += 1; // blank

        // Files header
        line += 1; // "── Changes (N file(s)) ──"

        if info.files.is_empty() {
            line += 1; // "No changes"
            return line;
        }

        // Now count through files up to the cursor
        let cursor_file = match self.file_cursor {
            Some(f) => f,
            None => return line, // no file selected, cursor is at top
        };

        for idx in 0..=cursor_file.min(info.files.len().saturating_sub(1)) {
            if idx == cursor_file {
                // We're at the file header line for the cursor file
                if let Some(dl) = self.diff_line {
                    // Cursor is inside the expanded diff
                    line += 1; // the file header line itself
                    line += dl; // the diff line offset
                    return line;
                }
                return line; // cursor is on the file header
            }

            // Count this file's line(s)
            line += 1; // file header
            if self.expanded_files.contains(&idx) {
                let diff_count = self.file_diffs.get(&idx).map(|d| d.len()).unwrap_or(0);
                if diff_count == 0 {
                    line += 1; // "(no diff content)"
                } else {
                    line += diff_count;
                }
                line += 1; // blank line after diff
            }
        }

        line
    }

    /// Adjust `detail_scroll` so the cursor is visible within the detail pane.
    /// `pane_height` is the inner height of the detail area.
    pub fn scroll_detail_to_cursor(&mut self, pane_height: u16) {
        let cursor_line = self.cursor_visual_line() as u16;
        let h = pane_height.saturating_sub(1); // leave 1 line margin

        if cursor_line < self.detail_scroll {
            self.detail_scroll = cursor_line;
        } else if cursor_line >= self.detail_scroll + h {
            self.detail_scroll = cursor_line.saturating_sub(h) + 1;
        }
    }

    fn handle_detail_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        let nfiles = self.file_count();

        match (code, modifiers) {
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                if nfiles == 0 {
                    return Ok(Action::None);
                }

                // If inside an expanded diff, scroll through diff lines
                if let Some(idx) = self.file_cursor {
                    if self.expanded_files.contains(&idx) {
                        let diff_len = self.current_diff_len();
                        match self.diff_line {
                            None => {
                                // Enter the diff: select first line
                                if diff_len > 0 {
                                    self.diff_line = Some(0);
                                    return Ok(Action::None);
                                }
                                // Empty diff: fall through to next file
                            }
                            Some(line) if line + 1 < diff_len => {
                                // Move down within diff
                                self.diff_line = Some(line + 1);
                                return Ok(Action::None);
                            }
                            Some(_) => {
                                // Past last diff line: move to next file
                                self.diff_line = None;
                                if idx + 1 < nfiles {
                                    self.file_cursor = Some(idx + 1);
                                }
                                return Ok(Action::None);
                            }
                        }
                    }
                }

                // File-level navigation
                self.diff_line = None;
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
                // If inside an expanded diff, scroll up through diff lines
                if let Some(idx) = self.file_cursor {
                    if self.expanded_files.contains(&idx) {
                        match self.diff_line {
                            Some(0) => {
                                // Back to file header
                                self.diff_line = None;
                                return Ok(Action::None);
                            }
                            Some(line) => {
                                self.diff_line = Some(line - 1);
                                return Ok(Action::None);
                            }
                            None => {
                                // On file header of expanded file.
                                // Move to previous file. If that file is expanded,
                                // land on its last diff line.
                            }
                        }
                    }
                }

                // File-level navigation
                self.diff_line = None;
                match self.file_cursor {
                    Some(0) => {
                        self.file_cursor = None;
                    }
                    Some(i) => {
                        let prev = i - 1;
                        self.file_cursor = Some(prev);
                        // If previous file is expanded, land on its last diff line
                        if self.expanded_files.contains(&prev) {
                            let prev_len = self.file_diffs.get(&prev).map(|d| d.len()).unwrap_or(0);
                            if prev_len > 0 {
                                self.diff_line = Some(prev_len - 1);
                            }
                        }
                    }
                    None => {}
                }
                Ok(Action::None)
            }
            // Enter: toggle file diff expansion (or launch agent if no file selected)
            (KeyCode::Enter, _) => {
                if let Some(idx) = self.file_cursor {
                    if self.expanded_files.contains(&idx) {
                        // Collapse
                        self.expanded_files.remove(&idx);
                        self.diff_line = None;
                    } else {
                        // Expand
                        self.ensure_diff_cached(idx);
                        self.expanded_files.insert(idx);
                        self.diff_line = None; // cursor on file header
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
                self.diff_line = None;
                Ok(Action::None)
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
                self.detail_scroll = u16::MAX;
                self.diff_line = None;
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
                let _leaving = self.modal.field;
                self.modal.field = self.modal.field.next();
                Ok(Action::None)
            }
            (KeyCode::BackTab, _) => {
                self.modal.field = self.modal.field.prev();
                Ok(Action::None)
            }

            // Issue field: Enter or Space opens the picker
            (KeyCode::Enter | KeyCode::Char(' '), _) if self.modal.field == ModalField::Issue => {
                self.open_issue_picker();
                Ok(Action::None)
            }

            // Enter: submit (from any other field)
            (KeyCode::Enter, _) => self.try_submit(),

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

            // Ctrl+L: open Linear issue picker (from any field)
            (KeyCode::Char('l'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.open_issue_picker();
                Ok(Action::None)
            }

            // Prompt-specific: cursor movement
            (KeyCode::Left, _) if self.modal.field == ModalField::Prompt => {
                if self.modal.prompt_cursor > 0 {
                    // Move back one char boundary
                    let prev = self.modal.prompt[..self.modal.prompt_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.modal.prompt_cursor = prev;
                }
                Ok(Action::None)
            }
            (KeyCode::Right, _) if self.modal.field == ModalField::Prompt => {
                if self.modal.prompt_cursor < self.modal.prompt.len() {
                    // Move forward one char boundary
                    let next = self.modal.prompt[self.modal.prompt_cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.modal.prompt_cursor + i)
                        .unwrap_or(self.modal.prompt.len());
                    self.modal.prompt_cursor = next;
                }
                Ok(Action::None)
            }
            (KeyCode::Home, _) if self.modal.field == ModalField::Prompt => {
                // Move to start of current line
                let before = &self.modal.prompt[..self.modal.prompt_cursor];
                self.modal.prompt_cursor = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                Ok(Action::None)
            }
            (KeyCode::End, _) if self.modal.field == ModalField::Prompt => {
                // Move to end of current line
                let after = &self.modal.prompt[self.modal.prompt_cursor..];
                self.modal.prompt_cursor = after
                    .find('\n')
                    .map(|i| self.modal.prompt_cursor + i)
                    .unwrap_or(self.modal.prompt.len());
                Ok(Action::None)
            }
            (KeyCode::Up, _) if self.modal.field == ModalField::Prompt => {
                // Move cursor to previous line
                let before = &self.modal.prompt[..self.modal.prompt_cursor];
                if let Some(newline_pos) = before.rfind('\n') {
                    // Find column offset in current line
                    let line_start = before[..newline_pos]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let col = self.modal.prompt_cursor - (newline_pos + 1);
                    let prev_line_len = newline_pos - line_start;
                    self.modal.prompt_cursor = line_start + col.min(prev_line_len);
                }
                Ok(Action::None)
            }
            (KeyCode::Down, _) if self.modal.field == ModalField::Prompt => {
                // Move cursor to next line
                let after = &self.modal.prompt[self.modal.prompt_cursor..];
                if let Some(newline_pos) = after.find('\n') {
                    let abs_newline = self.modal.prompt_cursor + newline_pos;
                    // Column offset in current line
                    let before = &self.modal.prompt[..self.modal.prompt_cursor];
                    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                    let col = self.modal.prompt_cursor - line_start;
                    // Next line bounds
                    let next_line_start = abs_newline + 1;
                    let next_line_end = self.modal.prompt[next_line_start..]
                        .find('\n')
                        .map(|i| next_line_start + i)
                        .unwrap_or(self.modal.prompt.len());
                    let next_line_len = next_line_end - next_line_start;
                    self.modal.prompt_cursor = next_line_start + col.min(next_line_len);
                }
                Ok(Action::None)
            }

            // Text input (name field: append at end; prompt field: insert at cursor)
            (KeyCode::Backspace, _) if self.modal.field.is_text_input() => {
                if self.modal.field == ModalField::Prompt {
                    if self.modal.prompt_cursor > 0 {
                        let prev = self.modal.prompt[..self.modal.prompt_cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        self.modal.prompt.drain(prev..self.modal.prompt_cursor);
                        self.modal.prompt_cursor = prev;
                    }
                } else if let Some(text) = self.modal.active_text_mut() {
                    text.pop();
                }
                Ok(Action::None)
            }
            (KeyCode::Char(c), m)
                if self.modal.field.is_text_input() && !m.contains(KeyModifiers::CONTROL) =>
            {
                if self.modal.field == ModalField::Prompt {
                    self.modal.prompt.insert(self.modal.prompt_cursor, c);
                    self.modal.prompt_cursor += c.len_utf8();
                } else if let Some(text) = self.modal.active_text_mut() {
                    text.push(c);
                }
                Ok(Action::None)
            }

            _ => Ok(Action::None),
        }
    }

    fn open_issue_picker(&mut self) {
        if crate::core::config::get("linear.api_key").is_some() {
            self.mode = Mode::IssuePicker;
            self.modal.picker_query.clear();
            self.modal.picker_selected = 0;
            self.modal.picker_status = Some("Loading your issues…".to_string());
            match crate::core::linear::my_issues(20) {
                Ok(issues) => {
                    self.modal.picker_status = Some(format!("{} issue(s)", issues.len()));
                    self.modal.picker_results = issues;
                }
                Err(e) => {
                    let msg: String = e.to_string().chars().take(60).collect();
                    self.modal.picker_status = Some(format!("✗ {}", msg));
                    self.modal.picker_results.clear();
                }
            }
        } else {
            self.error = Some("Set Linear key first: pit config set linear.api_key <key>".into());
        }
    }

    fn handle_picker_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<Action> {
        match (code, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = Mode::NewTask;
                Ok(Action::None)
            }

            // Navigate results
            (KeyCode::Up | KeyCode::Char('k'), _) if !modifiers.contains(KeyModifiers::CONTROL) => {
                if self.modal.picker_selected > 0 {
                    self.modal.picker_selected -= 1;
                }
                Ok(Action::None)
            }
            (KeyCode::Down | KeyCode::Char('j'), _)
                if !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if !self.modal.picker_results.is_empty()
                    && self.modal.picker_selected < self.modal.picker_results.len() - 1
                {
                    self.modal.picker_selected += 1;
                }
                Ok(Action::None)
            }

            // Select issue
            (KeyCode::Enter, _) => {
                if let Some(issue) = self.modal.picker_results.get(self.modal.picker_selected) {
                    let issue = issue.clone();
                    // Fill in the modal fields
                    self.modal.issue = issue.url.clone();
                    self.modal.issue_status = Some(format!(
                        "✓ {} · {} [{}]",
                        issue.identifier, issue.title, issue.state
                    ));

                    // Fill prompt from issue title + description
                    self.modal.prompt = crate::core::linear::issue_to_prompt(&issue);
                    self.modal.prompt_cursor = self.modal.prompt.len();
                    self.modal.prompt_scroll = 0;

                    // Auto-fill name if it's still the generated default
                    let slug: String = issue.identifier.to_lowercase().replace(' ', "-");
                    if !slug.is_empty() {
                        self.modal.name = slug;
                    }
                }
                // Focus prompt field so user can review/edit the issue text
                self.modal.field = ModalField::Prompt;
                self.mode = Mode::NewTask;
                Ok(Action::None)
            }

            // Search: type to filter
            (KeyCode::Backspace, _) => {
                self.modal.picker_query.pop();
                self.picker_search();
                Ok(Action::None)
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.modal.picker_query.push(c);
                self.picker_search();
                Ok(Action::None)
            }

            _ => Ok(Action::None),
        }
    }

    fn picker_search(&mut self) {
        let query = self.modal.picker_query.trim().to_string();
        if query.is_empty() {
            // Reload assigned issues
            self.modal.picker_status = Some("Loading…".to_string());
            match crate::core::linear::my_issues(20) {
                Ok(issues) => {
                    self.modal.picker_status = Some(format!("{} issue(s)", issues.len()));
                    self.modal.picker_results = issues;
                }
                Err(e) => {
                    let msg: String = e.to_string().chars().take(60).collect();
                    self.modal.picker_status = Some(format!("✗ {}", msg));
                    self.modal.picker_results.clear();
                }
            }
        } else {
            match crate::core::linear::search_issues(&query, 15) {
                Ok(issues) => {
                    self.modal.picker_status =
                        Some(format!("{} result(s) for '{}'", issues.len(), query));
                    self.modal.picker_results = issues;
                }
                Err(e) => {
                    let msg: String = e.to_string().chars().take(60).collect();
                    self.modal.picker_status = Some(format!("✗ {}", msg));
                    self.modal.picker_results.clear();
                }
            }
        }
        self.modal.picker_selected = 0;
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
    Shell(i64),
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
        // Auto-scroll detail pane to keep cursor visible (before draw)
        if app.focus == Pane::Detail && app.file_cursor.is_some() {
            app.scroll_detail_to_cursor(app.detail_pane_height);
        }

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
                    Action::Shell(task_id) => {
                        ratatui::restore();
                        handle_shell(app, task_id)?;
                        *terminal = ratatui::init();
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

fn handle_create(
    app: &mut App,
    name: &str,
    prompt: &str,
    issue_url: &str,
    agent: &str,
) -> Result<()> {
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
/// Build the shell command to launch an agent for a task.
/// Returns (command, session_id).
///
/// Prompts are written to a `.pit-prompt` file in the worktree and read back
/// via `cat` to avoid shell escaping issues with backticks, quotes, newlines,
/// and other special characters that appear in issue descriptions.
pub fn build_agent_cmd(task: &Task) -> (String, String) {
    let session_id = task
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let is_resume = task.session_id.is_some();

    // Write prompt to file to avoid shell escaping issues.
    // The file lives in the task's worktree and is gitignored.
    let prompt_file = std::path::Path::new(&task.worktree).join(".pit-prompt");
    if !task.prompt.is_empty() {
        let _ = std::fs::write(&prompt_file, &task.prompt);
    }
    let prompt_path = prompt_file.to_string_lossy();

    let cmd = match task.agent.as_str() {
        "pi" => {
            // Pi coding agent: --continue resumes last session, prompt is positional
            // Pi uses -p for non-interactive (like claude), so prompt is positional
            if is_resume {
                "pi --continue".to_string()
            } else if !task.prompt.is_empty() {
                format!("pi \"$(cat '{}')\"", prompt_path)
            } else {
                "pi".to_string()
            }
        }
        "codex" => {
            if !task.prompt.is_empty() {
                format!("codex \"$(cat '{}')\"", prompt_path)
            } else {
                "codex".to_string()
            }
        }
        "aider" => {
            if !task.prompt.is_empty() {
                format!("aider --message \"$(cat '{}')\"", prompt_path)
            } else {
                "aider".to_string()
            }
        }
        "amp" => {
            if !task.prompt.is_empty() {
                format!("amp --prompt \"$(cat '{}')\"", prompt_path)
            } else {
                "amp".to_string()
            }
        }
        "goose" => {
            // Block's Goose agent: prompt via positional arg
            if !task.prompt.is_empty() {
                format!("goose \"$(cat '{}')\"", prompt_path)
            } else {
                "goose".to_string()
            }
        }
        "custom" => {
            if !task.prompt.is_empty() {
                task.prompt.clone() // custom: prompt IS the command
            } else {
                "echo 'No agent configured. Type your command.'".to_string()
            }
        }
        // Default: claude (with session resume support)
        _ => {
            if is_resume {
                format!("claude -r {}", session_id)
            } else if !task.prompt.is_empty() {
                format!(
                    "claude --session-id {} \"$(cat '{}')\"",
                    session_id, prompt_path
                )
            } else {
                format!("claude --session-id {}", session_id)
            }
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

    tmux::create_session_with_cmd(&tmux_name, &task.worktree, &agent_cmd)?;
    task::set_running(db, task.id, &tmux_name, None, Some(&session_id))?;

    Ok(tmux_name)
}

fn handle_enter(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;
    let tmux_name = launch_task(&db, &task)?;
    tmux::attach(&tmux_name)?;

    if !tmux::session_exists(&tmux_name) {
        task::set_status(&db, task_id, &task::Status::Idle)?;
    }

    Ok(())
}

fn handle_background(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;
    launch_task(&db, &task)?;
    Ok(())
}

fn handle_shell(app: &mut App, task_id: i64) -> Result<()> {
    let db = crate::db::open(&app.db_path)?;
    let task = task::get(&db, task_id)?.ok_or_else(|| anyhow::anyhow!("task not found"))?;

    let tmux_name = format!("pit-shell-{}", task.name);

    if !tmux::session_exists(&tmux_name) {
        // Launch a plain shell in the task's worktree
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        tmux::create_session_with_cmd(&tmux_name, &task.worktree, &shell)?;
    }

    tmux::attach(&tmux_name)?;
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
            view: View::List,
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
            diff_line: None,
            expanded_files: std::collections::HashSet::new(),
            file_diffs: std::collections::HashMap::new(),
            kanban_col: 0,
            kanban_row: [0, 0, 0],
            detail_pane_height: 30,
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
        app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE)
            .unwrap();
        assert!(app.should_quit);
    }

    #[test]
    fn quit_on_ctrl_c() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
            .unwrap();
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
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.selected, 2);
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 2);
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.selected, 0);
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn enter_returns_task_id() {
        let mut app = make_app(vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Idle),
        ]);
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
        let action = app
            .handle_key(KeyCode::Char('b'), KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(action, Action::Background(1)));
    }

    #[test]
    fn delete_returns_task_id() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        let action = app
            .handle_key(KeyCode::Char('d'), KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(action, Action::Delete(1)));
    }

    #[test]
    fn shell_returns_task_id() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        let action = app
            .handle_key(KeyCode::Char('t'), KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(action, Action::Shell(1)));
    }

    #[test]
    fn shell_on_empty_list_is_noop() {
        let mut app = make_app(vec![]);
        let action = app
            .handle_key(KeyCode::Char('t'), KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(action, Action::None));
    }

    // --- Modal ---

    #[test]
    fn n_opens_modal_with_generated_name() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert_eq!(app.modal.field, ModalField::Name);
        // Name should be auto-generated (adjective-noun)
        assert!(
            app.modal.name.contains('-'),
            "expected generated name, got: {}",
            app.modal.name
        );
        assert!(!app.modal.name.is_empty());
    }

    #[test]
    fn modal_typing_replaces_generated_name() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        // Clear the generated name
        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
        }

        for c in "fix-bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE)
                .unwrap();
        }
        assert_eq!(app.modal.name, "fix-bug");
    }

    #[test]
    fn modal_tab_cycles_all_fields() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.field, ModalField::Name);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Prompt);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Agent);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::AutoApprove);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Issue);

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Name); // wraps
    }

    #[test]
    fn modal_backtab_cycles_backwards() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.field, ModalField::Issue);

        app.handle_key(KeyCode::BackTab, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.field, ModalField::AutoApprove);
    }

    #[test]
    fn modal_agent_cycle() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        // Go to agent field
        app.modal.field = ModalField::Agent;
        assert_eq!(app.modal.agent, "claude");

        // claude → pi → codex → amp → aider → goose → custom → claude
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.agent, "pi");

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
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        app.modal.field = ModalField::AutoApprove;
        assert!(!app.modal.auto_approve);

        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE)
            .unwrap();
        assert!(app.modal.auto_approve);

        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE)
            .unwrap();
        assert!(!app.modal.auto_approve);
    }

    #[test]
    fn modal_esc_cancels() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn modal_submit_full_flow() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        // Clear generated name, type custom
        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
        }
        for c in "my-task".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE)
                .unwrap();
        }

        // Tab to prompt
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        for c in "fix the bug".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE)
                .unwrap();
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
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

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
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        // Clear the generated name
        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert!(app.error.is_some());
    }

    #[test]
    fn modal_invalid_name_shows_error() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        while !app.modal.name.is_empty() {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                .unwrap();
        }
        for c in "has spaces".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE)
                .unwrap();
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert!(app.error.is_some());
    }

    #[test]
    fn modal_typing_in_prompt_field() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.field, ModalField::Prompt);

        for c in "refactor the API layer".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::NONE)
                .unwrap();
        }
        assert_eq!(app.modal.prompt, "refactor the API layer");
    }

    #[test]
    fn issue_field_rejects_typing() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        app.modal.field = ModalField::Issue;
        app.handle_key(KeyCode::Char('x'), KeyModifiers::NONE)
            .unwrap();
        // Issue field is not a text input — typing is ignored
        assert!(app.modal.issue.is_empty());
    }

    #[test]
    fn modal_submit_includes_agent() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        // Cycle to codex (claude → pi → codex)
        app.modal.field = ModalField::Agent;
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
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
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            action,
            Action::CreateTask { ref agent, .. } if agent == "claude"
        ));
    }

    // --- build_agent_cmd ---

    fn make_task_with_agent(agent: &str, prompt: &str, session_id: Option<&str>) -> Task {
        // Use a unique worktree per agent+prompt to avoid parallel test races
        let hash = format!("{:x}", {
            let mut h: u64 = 0;
            for b in format!("{}-{}", agent, prompt).bytes() {
                h = h.wrapping_mul(31).wrapping_add(b as u64);
            }
            h
        });
        let worktree = format!("/tmp/pit-test-wt-{}", hash);
        let _ = std::fs::create_dir_all(&worktree);
        Task {
            id: 1,
            name: "test".to_string(),
            description: String::new(),
            prompt: prompt.to_string(),
            issue_url: String::new(),
            agent: agent.to_string(),
            branch: "pit/test".to_string(),
            worktree,
            status: task::Status::Idle,
            session_id: session_id.map(|s| s.to_string()),
            tmux_session: None,
            pid: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn prompt_file_for(task: &Task) -> String {
        format!("{}/.pit-prompt", task.worktree)
    }

    #[test]
    fn agent_cmd_claude_new_session() {
        let task = make_task_with_agent("claude", "fix bug", None);
        let (cmd, session_id) = build_agent_cmd(&task);
        assert!(cmd.starts_with("claude --session-id "), "got: {}", cmd);
        assert!(cmd.contains(&session_id));
        // Prompt read from file via $(cat ...)
        assert!(cmd.contains("$(cat '"), "got: {}", cmd);
        assert!(!cmd.contains("-p "), "should not use -p: {}", cmd);
        // Prompt file written
        let content = std::fs::read_to_string(prompt_file_for(&task)).unwrap();
        assert_eq!(content, "fix bug");
    }

    #[test]
    fn agent_cmd_claude_resume_session() {
        let task = make_task_with_agent("claude", "fix bug", Some("sess-123"));
        let (cmd, session_id) = build_agent_cmd(&task);
        assert_eq!(session_id, "sess-123");
        assert_eq!(cmd, "claude -r sess-123");
        // Resume should NOT include the prompt
        assert!(
            !cmd.contains("cat"),
            "resume should not read prompt file: {}",
            cmd
        );
    }

    #[test]
    fn agent_cmd_claude_no_prompt() {
        let task = make_task_with_agent("claude", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.starts_with("claude --session-id "));
        assert!(!cmd.contains("cat"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_codex_with_prompt() {
        let task = make_task_with_agent("codex", "refactor API", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.starts_with("codex \"$(cat '"), "got: {}", cmd);
        let content = std::fs::read_to_string(prompt_file_for(&task)).unwrap();
        assert_eq!(content, "refactor API");
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
        assert!(cmd.starts_with("aider --message \"$(cat '"), "got: {}", cmd);
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
        assert!(cmd.starts_with("amp --prompt \"$(cat '"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_amp_no_prompt() {
        let task = make_task_with_agent("amp", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "amp");
    }

    #[test]
    fn agent_cmd_pi_with_prompt() {
        let task = make_task_with_agent("pi", "fix the login bug", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.starts_with("pi \"$(cat '"), "got: {}", cmd);
        let content = std::fs::read_to_string(prompt_file_for(&task)).unwrap();
        assert_eq!(content, "fix the login bug");
    }

    #[test]
    fn agent_cmd_pi_no_prompt() {
        let task = make_task_with_agent("pi", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "pi");
    }

    #[test]
    fn agent_cmd_pi_resume() {
        let task = make_task_with_agent("pi", "fix bug", Some("sess-456"));
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "pi --continue");
    }

    #[test]
    fn agent_cmd_goose_with_prompt() {
        let task = make_task_with_agent("goose", "add tests", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert!(cmd.starts_with("goose \"$(cat '"), "got: {}", cmd);
    }

    #[test]
    fn agent_cmd_goose_no_prompt() {
        let task = make_task_with_agent("goose", "", None);
        let (cmd, _) = build_agent_cmd(&task);
        assert_eq!(cmd, "goose");
    }

    #[test]
    fn agent_cmd_custom_with_prompt() {
        let task = make_task_with_agent("custom", "my-script --flag", None);
        let (cmd, _) = build_agent_cmd(&task);
        // Custom: prompt IS the command (no wrapping)
        assert_eq!(cmd, "my-script --flag");
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
        assert!(cmd.starts_with("claude --session-id "), "got: {}", cmd);
        assert!(cmd.contains("$(cat '"), "got: {}", cmd);
        assert!(!cmd.contains("-p "), "should not use -p: {}", cmd);
    }

    #[test]
    fn agent_cmd_prompt_file_handles_special_chars() {
        let prompt = "Fix `bug` in user's\nlogin with $PATH and \"quotes\"";
        let task = make_task_with_agent("claude", prompt, None);
        let (cmd, _) = build_agent_cmd(&task);
        // Command uses file-based prompt, not inline
        assert!(cmd.contains("$(cat '"), "got: {}", cmd);
        // File contains the exact prompt, unescaped
        let content = std::fs::read_to_string(prompt_file_for(&task)).unwrap();
        assert_eq!(content, prompt);
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
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();
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
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE)
            .unwrap();
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
        // No file selected → switches pane
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::TaskList);
    }

    #[test]
    fn esc_layered_in_detail() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Inside a diff line
        app.file_cursor = Some(0);
        app.expanded_files.insert(0);
        app.file_diffs
            .insert(0, vec!["@@ @@".into(), "+line".into()]);
        app.diff_line = Some(1);

        // Esc 1: exits diff line → on file header
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.diff_line, None);
        assert_eq!(app.file_cursor, Some(0));
        assert!(app.expanded_files.contains(&0));
        assert_eq!(app.focus, Pane::Detail);

        // Esc 2: collapses expanded file
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert!(!app.expanded_files.contains(&0));
        assert_eq!(app.file_cursor, Some(0));
        assert_eq!(app.focus, Pane::Detail);

        // Esc 3: deselects file
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.file_cursor, None);
        assert_eq!(app.focus, Pane::Detail);

        // Esc 4: switches pane
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.focus, Pane::TaskList);
    }

    // --- File navigation in detail pane ---

    fn sample_files() -> Vec<git_info::FileStat> {
        vec![
            git_info::FileStat {
                path: "src/main.rs".into(),
                insertions: 10,
                deletions: 2,
            },
            git_info::FileStat {
                path: "src/lib.rs".into(),
                insertions: 5,
                deletions: 0,
            },
            git_info::FileStat {
                path: "Cargo.toml".into(),
                insertions: 1,
                deletions: 0,
            },
        ]
    }

    #[test]
    fn jk_in_detail_moves_file_cursor() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;
        assert_eq!(app.file_cursor, None);

        // First j: selects first file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(0));

        // Second j: moves to second file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(1));

        // Third j: moves to third file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(2));

        // Fourth j: stays at last file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(2));

        // k: moves back
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(1));

        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(0));

        // k from first file: deselects (back to None)
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, None);

        // k from None: stays None
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, None);
    }

    #[test]
    fn jk_in_detail_no_files_is_noop() {
        let mut app = make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], vec![]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, None);
    }

    #[test]
    fn enter_on_file_toggles_expansion() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Navigate to first file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
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
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;
        assert_eq!(app.file_cursor, None);

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Enter(1)));
    }

    #[test]
    fn multiple_files_can_be_expanded() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Expand file 0
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(app.expanded_files.contains(&0));

        // Move to file 1 and expand
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(app.expanded_files.contains(&0));
        assert!(app.expanded_files.contains(&1));
    }

    #[test]
    fn g_resets_file_cursor_and_diff_line() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;
        app.file_cursor = Some(2);
        app.diff_line = Some(5);

        app.handle_key(KeyCode::Char('g'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, None);
        assert_eq!(app.diff_line, None);
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn G_selects_last_file() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('G'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(2)); // last of 3 files
        assert_eq!(app.diff_line, None);
    }

    #[test]
    fn jk_in_tasklist_moves_selection_not_file_cursor() {
        let mut app = make_app(vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Idle),
        ]);
        assert_eq!(app.focus, Pane::TaskList);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.selected, 1);
        assert_eq!(app.file_cursor, None);
    }

    #[test]
    fn page_down_up_in_detail() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::PageDown, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.detail_scroll, 10);

        app.handle_key(KeyCode::PageDown, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.detail_scroll, 20);

        app.handle_key(KeyCode::PageUp, KeyModifiers::NONE).unwrap();
        assert_eq!(app.detail_scroll, 10);
    }

    #[test]
    fn cursor_visual_line_on_file_header() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;
        app.file_cursor = Some(0);

        let line = app.cursor_visual_line();
        // Should be after: name, status, branch, blank, commits header,
        // "No commits yet", blank, files header = 8 lines
        assert!(line >= 7, "cursor_visual_line={}", line);
    }

    #[test]
    fn cursor_visual_line_inside_expanded_diff() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;
        app.file_cursor = Some(0);
        app.expanded_files.insert(0);
        app.file_diffs.insert(
            0,
            vec!["+ line1".into(), "- line2".into(), "  line3".into()],
        );
        app.diff_line = Some(2);

        let line = app.cursor_visual_line();
        // Should be file_header_line + 1 (header) + 2 (diff_line index)
        let header_line = {
            let mut a2 =
                make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
            a2.file_cursor = Some(0);
            a2.cursor_visual_line()
        };
        assert_eq!(line, header_line + 1 + 2);
    }

    #[test]
    fn scroll_detail_to_cursor_scrolls_down() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;
        app.detail_pane_height = 10;
        app.detail_scroll = 0;

        // Put cursor on a line that's beyond the visible area
        app.file_cursor = Some(1);
        app.expanded_files.insert(0);
        app.file_diffs
            .insert(0, (0..20).map(|i| format!("+ line {}", i)).collect());

        app.scroll_detail_to_cursor(app.detail_pane_height);
        // Cursor should be visible now — scroll > 0
        let cursor = app.cursor_visual_line() as u16;
        assert!(
            cursor >= app.detail_scroll && cursor < app.detail_scroll + app.detail_pane_height,
            "cursor={} scroll={} height={}",
            cursor,
            app.detail_scroll,
            app.detail_pane_height
        );
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

        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.mode, Mode::NewTask);
    }

    #[test]
    fn q_quits_from_detail_pane() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE)
            .unwrap();
        assert!(app.should_quit);
    }

    #[test]
    fn h_in_tasklist_is_noop() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        assert_eq!(app.focus, Pane::TaskList);

        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.focus, Pane::TaskList); // stays in task list
    }

    #[test]
    fn l_in_detail_is_noop() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.focus = Pane::Detail;

        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.focus, Pane::Detail); // stays in detail
    }

    #[test]
    fn j_enters_expanded_diff_then_exits_to_next_file() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Select and expand file 0
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(0));
        // Simulate expanding with cached diff
        app.expanded_files.insert(0);
        app.file_diffs.insert(
            0,
            vec![
                "@@ -0,0 +1,3 @@".into(),
                "+fn main() {}".into(),
                "+// end".into(),
            ],
        );

        // j enters the diff: line 0
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(0));
        assert_eq!(app.diff_line, Some(0));

        // j moves to line 1
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.diff_line, Some(1));

        // j moves to line 2 (last)
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.diff_line, Some(2));

        // j past last line: exits to next file
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(1));
        assert_eq!(app.diff_line, None);
        // file 0 still expanded
        assert!(app.expanded_files.contains(&0));
    }

    #[test]
    fn k_exits_diff_to_file_header() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Select file 0, expand, enter diff
        app.file_cursor = Some(0);
        app.expanded_files.insert(0);
        app.file_diffs
            .insert(0, vec!["@@ line @@".into(), "+added".into()]);
        app.diff_line = Some(1);

        // k: move to line 0
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.diff_line, Some(0));

        // k: back to file header
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.diff_line, None);
        assert_eq!(app.file_cursor, Some(0));
    }

    #[test]
    fn k_from_file_header_to_prev_file_last_diff_line() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Expand file 0 with 3 lines
        app.expanded_files.insert(0);
        app.file_diffs.insert(
            0,
            vec!["@@ hunk @@".into(), "+line1".into(), "+line2".into()],
        );

        // Cursor on file 1 header
        app.file_cursor = Some(1);
        app.diff_line = None;

        // k: should go to file 0, last diff line
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(0));
        assert_eq!(app.diff_line, Some(2)); // last line of file 0's diff
    }

    #[test]
    fn switching_pane_preserves_file_cursor() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Select file 1
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.file_cursor, Some(1));

        // Switch to task list and back
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.focus, Pane::TaskList);
        assert_eq!(app.file_cursor, Some(1)); // preserved

        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.focus, Pane::Detail);
        assert_eq!(app.file_cursor, Some(1)); // still there
    }

    #[test]
    fn expanded_files_preserved_across_pane_switch() {
        let mut app =
            make_app_with_files(vec![make_task(1, "a", task::Status::Idle)], sample_files());
        app.focus = Pane::Detail;

        // Expand file 0
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE)
            .unwrap();
        app.expanded_files.insert(0);

        // Switch panes and back
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();

        // Still expanded
        assert!(app.expanded_files.contains(&0));
    }

    // --- Issue picker tests ---

    #[test]
    fn issue_field_enter_tries_to_open_picker() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Issue;

        // Enter on Issue field should NOT submit — it should try to open picker
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        // Either opens picker (if key configured) or shows error — but NOT submitted
        assert!(app.mode == Mode::IssuePicker || app.error.is_some());
    }

    #[test]
    fn issue_field_space_tries_to_open_picker() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Issue;

        app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE)
            .unwrap();
        assert!(app.mode == Mode::IssuePicker || app.error.is_some());
    }

    #[test]
    fn issue_field_is_not_text_input() {
        // Typing in the Issue field should NOT insert characters
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Issue;

        app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE)
            .unwrap();
        assert!(app.modal.issue.is_empty());
    }

    #[test]
    fn ctrl_l_tries_to_open_picker() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.mode, Mode::NewTask);

        app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
            .unwrap();
        // Either opens picker (if key configured) or shows error
        assert!(app.mode == Mode::IssuePicker || app.error.is_some());
    }

    #[test]
    fn picker_esc_returns_to_modal() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
    }

    #[test]
    fn picker_navigation() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;
        app.modal.picker_results = vec![
            crate::core::linear::LinearIssue {
                identifier: "A-1".into(),
                title: "First".into(),
                description: String::new(),
                state: "Todo".into(),
                priority_label: String::new(),
                url: "https://linear.app/t/issue/A-1".into(),
            },
            crate::core::linear::LinearIssue {
                identifier: "A-2".into(),
                title: "Second".into(),
                description: String::new(),
                state: "Todo".into(),
                priority_label: String::new(),
                url: "https://linear.app/t/issue/A-2".into(),
            },
        ];
        app.modal.picker_selected = 0;

        // Down
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.picker_selected, 1);

        // Down at end — stays
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.picker_selected, 1);

        // Up
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.picker_selected, 0);

        // Up at top — stays
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.picker_selected, 0);
    }

    #[test]
    fn picker_enter_fills_modal() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;
        app.modal.picker_results = vec![crate::core::linear::LinearIssue {
            identifier: "ENG-42".into(),
            title: "Fix login".into(),
            description: "SSO timeout issue".into(),
            state: "In Progress".into(),
            priority_label: "High".into(),
            url: "https://linear.app/t/issue/ENG-42/fix-login".into(),
        }];
        app.modal.picker_selected = 0;
        app.modal.prompt = String::new();

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();

        // Should return to modal mode
        assert_eq!(app.mode, Mode::NewTask);
        // Issue URL filled
        assert_eq!(
            app.modal.issue,
            "https://linear.app/t/issue/ENG-42/fix-login"
        );
        // Prompt auto-filled
        assert!(app.modal.prompt.contains("ENG-42"));
        assert!(app.modal.prompt.contains("Fix login"));
        assert!(app.modal.prompt.contains("SSO timeout"));
        // Name set to identifier slug
        assert_eq!(app.modal.name, "eng-42");
        // Issue status set
        assert!(app.modal.issue_status.as_ref().unwrap().contains("✓"));
    }

    #[test]
    fn picker_enter_overwrites_prompt_with_issue() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;
        app.modal.prompt = "my existing prompt".to_string();
        app.modal.picker_results = vec![crate::core::linear::LinearIssue {
            identifier: "X-1".into(),
            title: "Thing".into(),
            description: "Desc".into(),
            state: "Todo".into(),
            priority_label: String::new(),
            url: String::new(),
        }];
        app.modal.picker_selected = 0;

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(app.modal.prompt.contains("X-1"));
        assert!(app.modal.prompt.contains("Thing"));
        assert!(app.modal.prompt.contains("Desc"));
    }

    #[test]
    fn picker_typing_updates_query() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;

        app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.picker_query, "ab");

        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.picker_query, "a");
    }

    // --- Prompt textarea ---

    #[test]
    fn picker_focuses_prompt_after_selection() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;
        app.modal.picker_results = vec![crate::core::linear::LinearIssue {
            identifier: "ENG-1".into(),
            title: "Task".into(),
            description: "Desc".into(),
            state: "Todo".into(),
            priority_label: String::new(),
            url: String::new(),
        }];
        app.modal.picker_selected = 0;

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        assert_eq!(app.modal.field, ModalField::Prompt);
    }

    #[test]
    fn prompt_cursor_at_end_after_issue_fill() {
        let mut app = make_app(vec![]);
        app.mode = Mode::IssuePicker;
        app.modal.picker_results = vec![crate::core::linear::LinearIssue {
            identifier: "ENG-1".into(),
            title: "Task".into(),
            description: "Some description".into(),
            state: "Todo".into(),
            priority_label: String::new(),
            url: String::new(),
        }];
        app.modal.picker_selected = 0;

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.prompt_cursor, app.modal.prompt.len());
    }

    #[test]
    fn prompt_cursor_insert_at_position() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.mode, Mode::NewTask);
        app.modal.field = ModalField::Prompt;
        app.modal.prompt = "helo".to_string();
        app.modal.prompt_cursor = 3; // between 'l' and 'o'

        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.prompt, "hello");
        assert_eq!(app.modal.prompt_cursor, 4);
    }

    #[test]
    fn prompt_cursor_backspace_at_position() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Prompt;
        app.modal.prompt = "hello".to_string();
        app.modal.prompt_cursor = 3; // after 'l'

        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.modal.prompt, "helo");
        assert_eq!(app.modal.prompt_cursor, 2);
    }

    #[test]
    fn prompt_cursor_left_right() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Prompt;
        app.modal.prompt = "abc".to_string();
        app.modal.prompt_cursor = 2;

        app.handle_key(KeyCode::Left, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.prompt_cursor, 1);

        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.prompt_cursor, 2);
    }

    #[test]
    fn prompt_cursor_up_down_lines() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Prompt;
        app.modal.prompt = "line1\nline2\nline3".to_string();
        app.modal.prompt_cursor = 8; // "line2" position 2 = 'n'

        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        // Should move to line1, col 2
        assert_eq!(app.modal.prompt_cursor, 2);

        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        // Should move back to line2, col 2
        assert_eq!(app.modal.prompt_cursor, 8);

        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        // Should move to line3, col 2
        assert_eq!(app.modal.prompt_cursor, 14);
    }

    #[test]
    fn prompt_home_end() {
        let mut app = make_app(vec![]);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE)
            .unwrap();
        app.modal.field = ModalField::Prompt;
        app.modal.prompt = "hello\nworld".to_string();
        app.modal.prompt_cursor = 8; // 'r' in "world"

        app.handle_key(KeyCode::Home, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.prompt_cursor, 6); // start of "world"

        app.handle_key(KeyCode::End, KeyModifiers::NONE).unwrap();
        assert_eq!(app.modal.prompt_cursor, 11); // end of "world"
    }

    // --- Kanban ---

    #[test]
    fn v_toggles_kanban() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        assert_eq!(app.view, View::List);

        app.handle_key(KeyCode::Char('v'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.view, View::Kanban);

        app.handle_key(KeyCode::Char('v'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(app.view, View::List);
    }

    #[test]
    fn kanban_column_navigation() {
        let mut app = make_app(vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Running),
            make_task(3, "c", task::Status::Done),
        ]);
        app.view = View::Kanban;
        assert_eq!(app.kanban_col, 0);

        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_col, 1);

        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_col, 2);

        // Can't go past last column
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_col, 2);

        app.handle_key(KeyCode::Left, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_col, 1);
    }

    #[test]
    fn kanban_row_navigation() {
        let mut app = make_app(vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Idle),
            make_task(3, "c", task::Status::Idle),
        ]);
        app.view = View::Kanban;
        assert_eq!(app.kanban_row[0], 0);

        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_row[0], 1);

        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_row[0], 2);

        // Can't go past last row
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_row[0], 2);

        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_row[0], 1);
    }

    #[test]
    fn kanban_enter_on_selected() {
        let mut app = make_app(vec![
            make_task(1, "idle-one", task::Status::Idle),
            make_task(2, "running-one", task::Status::Running),
        ]);
        app.view = View::Kanban;

        // Select running column
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(app.kanban_col, 1);

        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::Enter(2)));
    }

    #[test]
    fn kanban_shell_on_selected() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.view = View::Kanban;

        let action = app
            .handle_key(KeyCode::Char('t'), KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(action, Action::Shell(1)));
    }

    #[test]
    fn kanban_empty_column_enter_is_noop() {
        let mut app = make_app(vec![make_task(1, "a", task::Status::Idle)]);
        app.view = View::Kanban;

        // Move to Running column (empty)
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn kanban_column_tasks_filters() {
        let app = make_app(vec![
            make_task(1, "a", task::Status::Idle),
            make_task(2, "b", task::Status::Running),
            make_task(3, "c", task::Status::Done),
            make_task(4, "d", task::Status::Idle),
        ]);
        assert_eq!(app.kanban_column_tasks(0).len(), 2); // idle
        assert_eq!(app.kanban_column_tasks(1).len(), 1); // running
        assert_eq!(app.kanban_column_tasks(2).len(), 1); // done
    }
}
