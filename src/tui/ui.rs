use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::core::task::Status;

use super::app::{App, ModalField, Mode, Pane, View};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(frame.area());

    match app.view {
        View::List => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(chunks[0]);

            let list_focused = app.focus == Pane::TaskList && app.mode == Mode::Normal;
            let detail_focused = app.focus == Pane::Detail && app.mode == Mode::Normal;
            draw_task_list(frame, app, panes[0], list_focused);
            draw_detail_pane(frame, app, panes[1], detail_focused);
        }
        View::Kanban => {
            draw_kanban(frame, app, chunks[0]);
        }
    }

    draw_help_bar(frame, app, chunks[1]);

    if app.mode == Mode::NewTask {
        draw_modal(frame, app);
    }

    if app.mode == Mode::IssuePicker {
        draw_issue_picker(frame, app);
    }

    if let Some(ref err) = app.error {
        draw_error_toast(frame, err);
    }
}

// ── Task list (left pane) ───────────────────────────────────────────────────

fn draw_task_list(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let border_color = if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    if app.tasks.is_empty() {
        let msg = Paragraph::new("  No tasks yet.\n  Press n to create.")
            .block(
                Block::default()
                    .title(" Tasks ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color)),
            )
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|t| {
            let status_style = status_color(t.status.clone());
            let icon = status_icon(&t.status);

            // Compact: icon + name + short status
            let name_width = area.width.saturating_sub(10) as usize;
            let name: String = t.name.chars().take(name_width).collect();

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", icon), status_style),
                Span::styled(name, Style::default().fg(Color::White)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Tasks ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸");

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

// ── Detail pane (right side) ────────────────────────────────────────────────

fn draw_detail_pane(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let border_color = if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let task = match app.tasks.get(app.selected) {
        Some(t) => t,
        None => {
            let empty = Paragraph::new("  Select a task to view details.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block);
            frame.render_widget(empty, area);
            return;
        }
    };

    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };

    // Build all lines for the detail view
    let mut lines: Vec<Line> = Vec::new();
    let w = inner.width as usize;

    // ── Header ──
    lines.push(Line::from(vec![Span::styled(
        &task.name,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )]));

    // Status + agent
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} {}", status_icon(&task.status), task.status),
            status_color(task.status.clone()),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("agent: {}", task.agent),
            Style::default().fg(Color::Gray),
        ),
    ]));

    // Branch
    lines.push(Line::from(vec![
        Span::styled("branch: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&task.branch, Style::default().fg(Color::Cyan)),
    ]));

    // Prompt (if set)
    if !task.prompt.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("prompt: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&task.prompt, Style::default().fg(Color::Gray)),
        ]));
    }

    // Issue URL (if set)
    if !task.issue_url.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("issue:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &task.issue_url,
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]));
    }

    lines.push(Line::from(""));

    // ── Git info ──
    if let Some(ref info) = app.detail {
        // Commits section
        let commit_count = info.commits.len();
        lines.push(Line::from(vec![
            Span::styled(
                format!("── Commits ({}) ", commit_count),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "─".repeat(w.saturating_sub(16 + digit_count(commit_count))),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        if info.commits.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No commits yet",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for c in &info.commits {
                let hash_span =
                    Span::styled(format!("  {}", c.hash), Style::default().fg(Color::Yellow));
                let msg_span =
                    Span::styled(format!(" {}", c.message), Style::default().fg(Color::White));
                let age_span =
                    Span::styled(format!("  {}", c.age), Style::default().fg(Color::DarkGray));
                lines.push(Line::from(vec![hash_span, msg_span, age_span]));
            }
        }

        lines.push(Line::from(""));

        // Files changed section
        let file_count = info.files.len();
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "── Changes ({} file{}) ",
                    file_count,
                    if file_count == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "─".repeat(w.saturating_sub(20 + digit_count(file_count))),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        if info.files.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No changes",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            // Find max path length for alignment
            let max_path = info
                .files
                .iter()
                .map(|f| f.path.len())
                .max()
                .unwrap_or(0)
                .min(w.saturating_sub(20));

            for (idx, f) in info.files.iter().enumerate() {
                let is_selected = focused && app.file_cursor == Some(idx);
                let is_expanded = app.expanded_files.contains(&idx);

                let path: String = f.path.chars().take(max_path).collect();
                let padding = max_path.saturating_sub(path.len()) + 2;

                // Cursor marker and expand indicator
                let marker = if is_selected { "▸ " } else { "  " };
                let expand = if is_expanded { "▾ " } else { "▸ " };

                let path_style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let mut spans = vec![
                    Span::styled(
                        marker,
                        if is_selected {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::styled(expand, Style::default().fg(Color::DarkGray)),
                    Span::styled(path, path_style),
                    Span::raw(" ".repeat(padding)),
                ];

                if f.insertions > 0 {
                    spans.push(Span::styled(
                        format!("+{}", f.insertions),
                        Style::default().fg(Color::Green),
                    ));
                }
                if f.insertions > 0 && f.deletions > 0 {
                    spans.push(Span::raw("  "));
                }
                if f.deletions > 0 {
                    spans.push(Span::styled(
                        format!("-{}", f.deletions),
                        Style::default().fg(Color::Red),
                    ));
                }

                lines.push(Line::from(spans));

                // Render expanded diff lines
                if is_expanded {
                    let is_cursor_file = focused && app.file_cursor == Some(idx);
                    if let Some(diff_lines) = app.file_diffs.get(&idx) {
                        if diff_lines.is_empty() {
                            lines.push(Line::from(Span::styled(
                                "      (no diff content)",
                                Style::default().fg(Color::DarkGray),
                            )));
                        } else {
                            for (di, dl) in diff_lines.iter().enumerate() {
                                let is_active_line = is_cursor_file && app.diff_line == Some(di);
                                let base_style = if dl.starts_with('+') {
                                    Style::default().fg(Color::Green)
                                } else if dl.starts_with('-') {
                                    Style::default().fg(Color::Red)
                                } else if dl.starts_with("@@") {
                                    Style::default().fg(Color::Cyan)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                };
                                let style = if is_active_line {
                                    base_style.bg(Color::DarkGray).add_modifier(Modifier::BOLD)
                                } else {
                                    base_style
                                };
                                let line_marker = if is_active_line { "  ▸ " } else { "    " };
                                let display: String =
                                    dl.chars().take(w.saturating_sub(6)).collect();
                                lines.push(Line::from(Span::styled(
                                    format!("{}{}", line_marker, display),
                                    style,
                                )));
                            }
                        }
                        lines.push(Line::from("")); // blank after diff
                    }
                }
            }

            // Total line
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  total: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("+{}", info.total_insertions),
                    Style::default().fg(Color::Green),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("-{}", info.total_deletions),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  Loading git info…",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    frame.render_widget(paragraph, inner);
}

fn status_icon(status: &Status) -> &'static str {
    match status {
        Status::Idle => "○",
        Status::Running => "▶",
        Status::Done => "✓",
        Status::Error => "✗",
    }
}

fn status_color(status: Status) -> Style {
    match status {
        Status::Idle => Style::default().fg(Color::DarkGray),
        Status::Running => Style::default().fg(Color::Green),
        Status::Done => Style::default().fg(Color::Blue),
        Status::Error => Style::default().fg(Color::Red),
    }
}

fn digit_count(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}

/// Number of visible lines in the prompt textarea.
pub const PROMPT_VISIBLE_LINES: usize = 6;
/// Approximate inner text width for the prompt textarea (modal_width - borders/padding).
pub const PROMPT_TEXT_WIDTH: usize = 62;

// ── Text wrapping helpers ────────────────────────────────────────────────────

/// Wrap text into lines that fit within `width` columns.
/// Handles explicit newlines and word-wrapping.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in raw_line.split(' ') {
            if current.is_empty() {
                if word.len() > width {
                    // Break long word across lines
                    let mut chars = word.chars();
                    while current.len() < word.len() {
                        if let Some(c) = chars.next() {
                            if current.len() + c.len_utf8() > width {
                                lines.push(current);
                                current = String::new();
                            }
                            current.push(c);
                        } else {
                            break;
                        }
                    }
                } else {
                    current = word.to_string();
                }
            } else if current.len() + 1 + word.len() > width {
                lines.push(current);
                if word.len() > width {
                    current = String::new();
                    for c in word.chars() {
                        if current.len() + c.len_utf8() > width {
                            lines.push(current);
                            current = String::new();
                        }
                        current.push(c);
                    }
                } else {
                    current = word.to_string();
                }
            } else {
                current.push(' ');
                current.push_str(word);
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Given a string, a byte cursor position, and wrap width, return the
/// (visual_row, visual_col) of the cursor within the wrapped output.
pub fn cursor_pos_in_wrapped(text: &str, cursor: usize, width: usize) -> (usize, usize) {
    if width == 0 {
        return (0, 0);
    }
    let cursor = cursor.min(text.len());

    // We need to reconstruct the wrapping character by character to track
    // where the cursor byte offset lands visually.
    let mut row: usize = 0;
    let mut col: usize = 0;
    let mut byte_pos: usize = 0;

    // Split by newlines first
    let lines: Vec<&str> = text.split('\n').collect();
    for (line_idx, raw_line) in lines.iter().enumerate() {
        let line_start_byte = byte_pos;
        let line_end_byte = byte_pos + raw_line.len();

        if cursor >= line_start_byte && cursor <= line_end_byte {
            // Cursor is within this raw line
            let offset_in_line = cursor - line_start_byte;
            // Wrap this line and find position
            let wrapped = wrap_text(raw_line, width);
            let mut consumed = 0;
            for (wrap_idx, wline) in wrapped.iter().enumerate() {
                let wlen = wline.len();
                // Account for spaces consumed between words during wrapping
                if offset_in_line <= consumed + wlen {
                    col = offset_in_line - consumed;
                    row += wrap_idx;
                    return (row, col);
                }
                consumed += wlen;
                // Skip the space or nothing that was between this wrapped line and next
                if consumed < raw_line.len() {
                    // The character at `consumed` in the raw line was either a space
                    // that caused the wrap break, or continuation
                    if raw_line.as_bytes().get(consumed) == Some(&b' ') {
                        consumed += 1; // skip the space that was the break point
                    }
                }
            }
            // Fallback: end of last wrapped line
            col = wrapped.last().map(|l| l.len()).unwrap_or(0);
            row += wrapped.len().saturating_sub(1);
            return (row, col);
        }

        let wrapped_count = wrap_text(raw_line, width).len();
        row += wrapped_count;
        byte_pos = line_end_byte;

        // Account for the newline character
        if line_idx < lines.len() - 1 {
            byte_pos += 1; // '\n'
            if cursor == byte_pos - 1 {
                // Cursor is right on the '\n' — show at end of last wrapped line
                col = wrap_text(raw_line, width)
                    .last()
                    .map(|l| l.len())
                    .unwrap_or(0);
                return (row - 1, col);
            }
        }
    }

    (row.saturating_sub(1), col)
}

// ── Modal ───────────────────────────────────────────────────────────────────

// ── Kanban view ─────────────────────────────────────────────────────────

fn draw_kanban(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);

    let headers = [
        ("○ Idle", Color::White),
        ("▶ Running", Color::Green),
        ("✓ Done", Color::Blue),
    ];

    for (col_idx, (header, color)) in headers.iter().enumerate() {
        let is_focused = app.kanban_col == col_idx && app.mode == Mode::Normal;
        let border_color = if is_focused {
            Color::Yellow
        } else {
            Color::DarkGray
        };

        let tasks = app.kanban_column_tasks(col_idx);
        let count = tasks.len();

        let title = format!(" {} ({}) ", header, count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(*color).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(columns[col_idx]);
        frame.render_widget(block, columns[col_idx]);

        let selected_row = app.kanban_row[col_idx];

        for (i, task) in tasks.iter().enumerate() {
            let y = inner.y + (i as u16) * 2;
            if y + 1 >= inner.y + inner.height {
                break;
            }

            let is_selected = is_focused && i == selected_row;
            let marker = if is_selected { "▸ " } else { "  " };

            let name_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            let bg = if is_selected {
                Style::default().bg(Color::Rgb(40, 40, 50))
            } else {
                Style::default()
            };

            // Line 1: marker + name
            let name_line = Line::from(vec![
                Span::styled(marker, bg),
                Span::styled(&task.name, name_style.patch(bg)),
            ]);
            frame.render_widget(
                Paragraph::new(name_line),
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
            );

            // Line 2: agent + extra info
            let agent_span = Span::styled(
                format!("  {}", task.agent),
                Style::default().fg(Color::DarkGray).patch(bg),
            );
            frame.render_widget(
                Paragraph::new(Line::from(agent_span)),
                Rect {
                    x: inner.x,
                    y: y + 1,
                    width: inner.width,
                    height: 1,
                },
            );
        }

        if tasks.is_empty() {
            let empty = Paragraph::new(Span::styled(
                "  (empty)",
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(
                empty,
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: 1,
                },
            );
        }
    }
}

fn draw_modal(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let modal_width = 70u16.min(area.width.saturating_sub(4));
    let modal_height = 26u16.min(area.height.saturating_sub(2));

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(modal_height)])
        .flex(Flex::Center)
        .split(area);

    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(modal_width)])
        .flex(Flex::Center)
        .split(vert[0]);

    let modal_area = horiz[0];
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" New Task ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(block, modal_area);

    let inner = Rect {
        x: modal_area.x + 2,
        y: modal_area.y + 1,
        width: modal_area.width.saturating_sub(4),
        height: modal_area.height.saturating_sub(2),
    };

    let m = &app.modal;
    let fw = inner.width;
    let mut y = inner.y;

    // --- Task name ---
    draw_field_label(
        frame,
        inner.x,
        y,
        fw,
        "Task name",
        m.field == ModalField::Name,
    );
    y += 1;
    draw_field_input(frame, inner.x, y, fw, &m.name, m.field == ModalField::Name);
    if m.field == ModalField::Name {
        frame.set_cursor_position((inner.x + 2 + m.name.len() as u16, y));
    }
    y += 2;

    // --- Agent prompt (multi-line textarea) ---
    let prompt_focused = m.field == ModalField::Prompt;
    draw_field_label(frame, inner.x, y, fw, "Agent prompt", prompt_focused);
    y += 1;

    let prompt_visible_lines: u16 = PROMPT_VISIBLE_LINES as u16;
    let text_width = (fw.saturating_sub(4)) as usize; // 2 padding + 1 border each side

    let (prompt_display, is_placeholder) = if m.prompt.is_empty() && !prompt_focused {
        (
            "e.g. Fix the login timeout bug and add tests".to_string(),
            true,
        )
    } else {
        (m.prompt.clone(), false)
    };

    // Wrap text into visual lines
    let wrapped = wrap_text(&prompt_display, text_width);

    // Find cursor visual position (row, col) within wrapped lines
    let (cursor_row, cursor_col) = if prompt_focused && !is_placeholder {
        cursor_pos_in_wrapped(&m.prompt, m.prompt_cursor, text_width)
    } else {
        (0, 0)
    };

    // Scroll is maintained by App via update_prompt_scroll()
    let scroll = m.prompt_scroll;

    let prompt_style = if is_placeholder {
        Style::default().fg(Color::DarkGray)
    } else if prompt_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };

    let border_color = if prompt_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    // Draw border around textarea
    let textarea_rect = Rect {
        x: inner.x,
        y,
        width: fw,
        height: prompt_visible_lines + 2, // +2 for top/bottom border
    };
    let textarea_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    frame.render_widget(textarea_block, textarea_rect);

    // Render visible lines inside the border
    let text_area_inner = Rect {
        x: inner.x + 2,
        y: y + 1,
        width: fw.saturating_sub(4),
        height: prompt_visible_lines,
    };

    let visible: Vec<Line> = wrapped
        .iter()
        .skip(scroll)
        .take(prompt_visible_lines as usize)
        .map(|l| Line::from(Span::styled(l.as_str(), prompt_style)))
        .collect();

    let text_widget = Paragraph::new(visible);
    frame.render_widget(text_widget, text_area_inner);

    // Scroll indicator on the right border
    if wrapped.len() > prompt_visible_lines as usize {
        let total = wrapped.len();
        let vl = prompt_visible_lines as usize;
        // Thumb position
        let thumb_pos = if total <= vl {
            0
        } else {
            (scroll * (vl - 1)) / (total - vl)
        };
        for row_i in 0..prompt_visible_lines {
            let ch = if row_i as usize == thumb_pos {
                "█"
            } else {
                "│"
            };
            let style = if row_i as usize == thumb_pos {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(ch, style)),
                Rect {
                    x: inner.x + fw - 1,
                    y: y + 1 + row_i,
                    width: 1,
                    height: 1,
                },
            );
        }
    }

    // Place cursor
    if prompt_focused && !is_placeholder {
        let vis_row = cursor_row.saturating_sub(scroll);
        if vis_row < prompt_visible_lines as usize {
            frame.set_cursor_position((inner.x + 2 + cursor_col as u16, y + 1 + vis_row as u16));
        }
    }

    y += prompt_visible_lines + 2 + 1; // textarea height + 1 gap

    // --- Agent ---
    draw_field_label(frame, inner.x, y, fw, "Agent", m.field == ModalField::Agent);
    y += 1;
    let agent_style = if m.field == ModalField::Agent {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let arrows = if m.field == ModalField::Agent {
        "  ◂ "
    } else {
        "  "
    };
    let arrows_r = if m.field == ModalField::Agent {
        " ▸"
    } else {
        ""
    };
    let agent_widget = Paragraph::new(Line::from(vec![
        Span::styled(arrows, Style::default().fg(Color::DarkGray)),
        Span::styled(&m.agent, agent_style),
        Span::styled(arrows_r, Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(
        agent_widget,
        Rect {
            x: inner.x,
            y,
            width: fw,
            height: 1,
        },
    );
    y += 1;

    // --- Auto-approve ---
    let aa_active = m.field == ModalField::AutoApprove;
    let aa_label_style = if aa_active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let check = if m.auto_approve { "✓" } else { " " };
    let check_style = if m.auto_approve {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if aa_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let marker = if aa_active { "▸" } else { " " };
    let aa_widget = Paragraph::new(Line::from(vec![
        Span::styled(format!("{} ", marker), aa_label_style),
        Span::styled(format!("[{}]", check), check_style),
        Span::styled(" Auto-approve ", aa_label_style),
        Span::styled(
            "— skip permission prompts",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(
        aa_widget,
        Rect {
            x: inner.x,
            y,
            width: fw,
            height: 1,
        },
    );
    y += 1;

    // --- Separator ---
    let sep = "─".repeat(fw as usize);
    let sep_widget = Paragraph::new(Span::styled(&sep, Style::default().fg(Color::DarkGray)));
    frame.render_widget(
        sep_widget,
        Rect {
            x: inner.x,
            y,
            width: fw,
            height: 1,
        },
    );
    y += 1;

    // --- Issue ---
    let issue_active = m.field == ModalField::Issue;
    let issue_label_style = if issue_active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let issue_label = Paragraph::new(Span::styled(" Issue", issue_label_style));
    frame.render_widget(
        issue_label,
        Rect {
            x: inner.x,
            y,
            width: fw,
            height: 1,
        },
    );
    y += 1;

    if let Some(ref status) = m.issue_status {
        let status_style = if status.starts_with('✓') {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let status_widget = Paragraph::new(Span::styled(format!("  {}", status), status_style));
        frame.render_widget(
            status_widget,
            Rect {
                x: inner.x,
                y,
                width: fw,
                height: 1,
            },
        );
    } else {
        let hint = if issue_active {
            Line::from(vec![
                Span::styled("  Press ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" or ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Ctrl+L",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to link an issue", Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(Span::styled(
                "  — no issue linked",
                Style::default().fg(Color::DarkGray),
            ))
        };
        frame.render_widget(
            Paragraph::new(hint),
            Rect {
                x: inner.x,
                y,
                width: fw,
                height: 1,
            },
        );
    }
}

fn draw_field_label(frame: &mut Frame, x: u16, y: u16, w: u16, label: &str, active: bool) {
    let style = if active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let marker = if active { "▸" } else { " " };
    let widget = Paragraph::new(Span::styled(format!("{} {}", marker, label), style));
    frame.render_widget(
        widget,
        Rect {
            x,
            y,
            width: w,
            height: 1,
        },
    );
}

fn draw_field_input(frame: &mut Frame, x: u16, y: u16, w: u16, value: &str, active: bool) {
    let style = if active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };
    let widget = Paragraph::new(Span::styled(format!("  {}", value), style));
    frame.render_widget(
        widget,
        Rect {
            x,
            y,
            width: w,
            height: 1,
        },
    );
}

// ── Toast / Help ────────────────────────────────────────────────────────────

fn draw_issue_picker(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let picker_width = 72u16.min(area.width.saturating_sub(4));
    let picker_height = 22u16.min(area.height.saturating_sub(2));

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(picker_height)])
        .flex(Flex::Center)
        .split(area);
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(picker_width)])
        .flex(Flex::Center)
        .split(vert[0]);
    let picker_area = horiz[0];
    frame.render_widget(Clear, picker_area);

    let block = Block::default()
        .title(" Linear Issues (Ctrl+L) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, picker_area);

    let inner = Rect {
        x: picker_area.x + 2,
        y: picker_area.y + 1,
        width: picker_area.width.saturating_sub(4),
        height: picker_area.height.saturating_sub(2),
    };

    let m = &app.modal;
    let fw = inner.width;
    let mut y = inner.y;

    // Search field
    let search_label = Span::styled(
        " Search: ",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    let search_value = Span::styled(
        if m.picker_query.is_empty() {
            "(type to search, empty = my issues)"
        } else {
            &m.picker_query
        },
        if m.picker_query.is_empty() {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        },
    );
    let search_line = Paragraph::new(Line::from(vec![search_label, search_value]));
    frame.render_widget(
        search_line,
        Rect {
            x: inner.x,
            y,
            width: fw,
            height: 1,
        },
    );
    // Cursor
    let cursor_x = inner.x + 9 + m.picker_query.len() as u16;
    frame.set_cursor_position((cursor_x.min(inner.x + fw - 1), y));
    y += 1;

    // Status line
    if let Some(ref status) = m.picker_status {
        let style = if status.starts_with('✗') {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let status_line = Paragraph::new(Span::styled(format!(" {}", status), style));
        frame.render_widget(
            status_line,
            Rect {
                x: inner.x,
                y,
                width: fw,
                height: 1,
            },
        );
    }
    y += 1;

    // Separator
    let sep = "─".repeat(fw as usize);
    let sep_widget = Paragraph::new(Span::styled(&sep, Style::default().fg(Color::DarkGray)));
    frame.render_widget(
        sep_widget,
        Rect {
            x: inner.x,
            y,
            width: fw,
            height: 1,
        },
    );
    y += 1;

    // Issue list
    let max_items = (inner.height.saturating_sub(4)) as usize;
    let scroll = if m.picker_selected >= max_items {
        m.picker_selected - max_items + 1
    } else {
        0
    };

    for (i, issue) in m
        .picker_results
        .iter()
        .enumerate()
        .skip(scroll)
        .take(max_items)
    {
        if y >= inner.y + inner.height {
            break;
        }
        let is_selected = i == m.picker_selected;

        let marker = if is_selected { "▸ " } else { "  " };
        let id_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let title_style = if is_selected {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };
        let state_style = Style::default().fg(Color::DarkGray);

        let bg = if is_selected {
            Style::default().bg(Color::Rgb(40, 40, 50))
        } else {
            Style::default()
        };

        // Truncate title to fit
        let id_len = issue.identifier.len();
        let state_len = issue.state.len() + 3; // " [state]"
        let available = (fw as usize).saturating_sub(id_len + state_len + 4); // marker + spaces
        let title: String = issue.title.chars().take(available).collect();

        let line = Line::from(vec![
            Span::styled(marker, bg),
            Span::styled(&issue.identifier, id_style.patch(bg)),
            Span::styled(" ", bg),
            Span::styled(title, title_style.patch(bg)),
            Span::styled(format!(" [{}]", issue.state), state_style.patch(bg)),
        ]);
        let item = Paragraph::new(line);
        frame.render_widget(
            item,
            Rect {
                x: inner.x,
                y,
                width: fw,
                height: 1,
            },
        );
        y += 1;
    }

    if m.picker_results.is_empty() && m.picker_status.is_none() {
        let empty = Paragraph::new(Span::styled(
            "  No issues found",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(
            empty,
            Rect {
                x: inner.x,
                y,
                width: fw,
                height: 1,
            },
        );
    }

    // Help bar at bottom of picker
    let help_y = picker_area.y + picker_area.height - 2;
    if help_y > y {
        let help = Line::from(vec![
            Span::styled(" ↑/↓", Style::default().fg(Color::Yellow)),
            Span::raw(":navigate  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(":select  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(":cancel  "),
            Span::styled("type", Style::default().fg(Color::Yellow)),
            Span::raw(":search"),
        ]);
        let help_widget = Paragraph::new(help).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(
            help_widget,
            Rect {
                x: inner.x,
                y: help_y,
                width: fw,
                height: 1,
            },
        );
    }
}

fn draw_error_toast(frame: &mut Frame, msg: &str) {
    let area = frame.area();
    let toast_width = (msg.len() as u16 + 6).min(area.width.saturating_sub(4));
    let toast_area = Rect {
        x: area.x + (area.width.saturating_sub(toast_width)) / 2,
        y: area.y + area.height.saturating_sub(5),
        width: toast_width,
        height: 1,
    };
    frame.render_widget(Clear, toast_area);
    let widget = Paragraph::new(Span::styled(
        format!(" ✗ {} ", msg),
        Style::default().fg(Color::White).bg(Color::Red),
    ));
    frame.render_widget(widget, toast_area);
}

fn config_indicators() -> Vec<Span<'static>> {
    use crate::core::config;

    let mut spans = vec![Span::raw("  ")];

    let linear = config::get("linear.api_key").is_some();
    let github = config::get("github.token").is_some();

    if linear {
        spans.push(Span::styled("Linear✓", Style::default().fg(Color::Green)));
    } else {
        spans.push(Span::styled(
            "Linear✗",
            Style::default().fg(Color::DarkGray),
        ));
    }

    spans.push(Span::raw(" "));

    if github {
        spans.push(Span::styled("GitHub✓", Style::default().fg(Color::Green)));
    } else {
        spans.push(Span::styled(
            "GitHub✗",
            Style::default().fg(Color::DarkGray),
        ));
    }

    spans
}

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = if app.mode == Mode::IssuePicker {
        Line::from(vec![
            Span::styled(
                " ↑/↓",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":navigate  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":select  "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":back  "),
            Span::styled(
                "type",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":search"),
        ])
    } else if app.mode == Mode::NewTask {
        Line::from(vec![
            Span::styled(
                " Tab",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":next  "),
            Span::styled(
                "Ctrl+L",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":issue  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":create  "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":cancel"),
        ])
    } else if app.view == View::Kanban {
        Line::from(vec![
            Span::styled(
                " ←/→",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":column  "),
            Span::styled(
                "↑/↓",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":select  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":open  "),
            Span::styled(
                "t",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":shell  "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":new  "),
            Span::styled(
                "d",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":del  "),
            Span::styled(
                "v",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":list  "),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":quit"),
        ])
    } else if app.focus == Pane::Detail {
        Line::from(vec![
            Span::styled(
                " j/k",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":navigate  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":expand diff  "),
            Span::styled(
                "h/←",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":back  "),
            Span::styled(
                "v",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":kanban  "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":new  "),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":quit"),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " Enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":open  "),
            Span::styled(
                "l/→",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":detail  "),
            Span::styled(
                "t",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":shell  "),
            Span::styled(
                "b",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":bg  "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":new  "),
            Span::styled(
                "d",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":del  "),
            Span::styled(
                "v",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":kanban  "),
            Span::styled(
                "r",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":refresh  "),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":quit"),
        ])
    };

    // Append config indicators to the right
    let mut spans = help.spans;
    spans.extend(config_indicators());
    let help = Line::from(spans);

    let bar = Paragraph::new(help).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(bar, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_short_text() {
        let lines = wrap_text("hello world", 20);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn wrap_at_boundary() {
        let lines = wrap_text("hello world", 5);
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[test]
    fn wrap_long_word() {
        let lines = wrap_text("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_preserves_newlines() {
        let lines = wrap_text("line1\nline2\nline3", 20);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn wrap_empty_lines() {
        let lines = wrap_text("a\n\nb", 20);
        assert_eq!(lines, vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_multiline_with_wrapping() {
        let lines = wrap_text("hello world\nfoo bar baz", 8);
        assert_eq!(lines, vec!["hello", "world", "foo bar", "baz"]);
    }

    #[test]
    fn cursor_at_start() {
        let (row, col) = cursor_pos_in_wrapped("hello world", 0, 20);
        assert_eq!((row, col), (0, 0));
    }

    #[test]
    fn cursor_at_end() {
        let (row, col) = cursor_pos_in_wrapped("hello", 5, 20);
        assert_eq!((row, col), (0, 5));
    }

    #[test]
    fn cursor_on_second_wrapped_line() {
        // "hello world" wraps to ["hello", "world"] at width 5
        // Cursor at byte 6 = 'w' in "world" = row 1, col 0
        let (row, col) = cursor_pos_in_wrapped("hello world", 6, 5);
        assert_eq!((row, col), (1, 0));
    }

    #[test]
    fn cursor_after_newline() {
        // "a\nb" -> row 0: "a", row 1: "b"
        // cursor at byte 2 = 'b' = row 1, col 0
        let (row, col) = cursor_pos_in_wrapped("a\nb", 2, 20);
        assert_eq!((row, col), (1, 0));
    }

    #[test]
    fn cursor_on_newline_char() {
        // "ab\ncd" cursor at byte 2 = the '\n' itself
        let (row, _col) = cursor_pos_in_wrapped("ab\ncd", 2, 20);
        assert_eq!(row, 0); // shows at end of first line
    }

    #[test]
    fn wrap_empty_string() {
        let lines = wrap_text("", 20);
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn cursor_in_empty_string() {
        let (row, col) = cursor_pos_in_wrapped("", 0, 20);
        assert_eq!((row, col), (0, 0));
    }
}
