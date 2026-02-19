use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::core::task::Status;

use super::app::{App, ModalField, Mode, Pane};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(frame.area());

    // Split the main area into task list (left) and detail (right)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(chunks[0]);

    let list_focused = app.focus == Pane::TaskList && app.mode == Mode::Normal;
    let detail_focused = app.focus == Pane::Detail && app.mode == Mode::Normal;
    draw_task_list(frame, app, panes[0], list_focused);
    draw_detail_pane(frame, app, panes[1], detail_focused);
    draw_help_bar(frame, app, chunks[1]);

    if app.mode == Mode::NewTask {
        draw_modal(frame, app);
    }

    if let Some(ref err) = app.error {
        draw_error_toast(frame, err);
    }
}

// ── Task list (left pane) ───────────────────────────────────────────────────

fn draw_task_list(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

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
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };
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
    lines.push(Line::from(vec![
        Span::styled(&task.name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]));

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
                let hash_span = Span::styled(
                    format!("  {}", c.hash),
                    Style::default().fg(Color::Yellow),
                );
                let msg_span = Span::styled(
                    format!(" {}", c.message),
                    Style::default().fg(Color::White),
                );
                let age_span = Span::styled(
                    format!("  {}", c.age),
                    Style::default().fg(Color::DarkGray),
                );
                lines.push(Line::from(vec![hash_span, msg_span, age_span]));
            }
        }

        lines.push(Line::from(""));

        // Files changed section
        let file_count = info.files.len();
        lines.push(Line::from(vec![
            Span::styled(
                format!("── Changes ({} file{}) ", file_count, if file_count == 1 { "" } else { "s" }),
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
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let mut spans = vec![
                    Span::styled(marker, if is_selected {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }),
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
                    if let Some(diff_lines) = app.file_diffs.get(&idx) {
                        if diff_lines.is_empty() {
                            lines.push(Line::from(Span::styled(
                                "      (no diff content)",
                                Style::default().fg(Color::DarkGray),
                            )));
                        } else {
                            for dl in diff_lines {
                                let (style, prefix) = if dl.starts_with('+') {
                                    (Style::default().fg(Color::Green), "")
                                } else if dl.starts_with('-') {
                                    (Style::default().fg(Color::Red), "")
                                } else if dl.starts_with("@@") {
                                    (Style::default().fg(Color::Cyan), "")
                                } else {
                                    (Style::default().fg(Color::DarkGray), "")
                                };
                                let display: String = dl.chars().take(w.saturating_sub(6) as usize).collect();
                                lines.push(Line::from(Span::styled(
                                    format!("    {}{}", prefix, display),
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
    if n == 0 { 1 } else { (n as f64).log10().floor() as usize + 1 }
}

// ── Modal ───────────────────────────────────────────────────────────────────

fn draw_modal(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let modal_width = 64u16.min(area.width.saturating_sub(4));
    let modal_height = 19u16.min(area.height.saturating_sub(2));

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
    draw_field_label(frame, inner.x, y, fw, "Task name", m.field == ModalField::Name);
    y += 1;
    draw_field_input(frame, inner.x, y, fw, &m.name, m.field == ModalField::Name);
    if m.field == ModalField::Name {
        frame.set_cursor_position((inner.x + 2 + m.name.len() as u16, y));
    }
    y += 2;

    // --- Agent prompt ---
    draw_field_label(frame, inner.x, y, fw, "Agent prompt", m.field == ModalField::Prompt);
    y += 1;
    let prompt_display = if m.prompt.is_empty() && m.field != ModalField::Prompt {
        "e.g. Fix the login timeout bug and add tests"
    } else {
        &m.prompt
    };
    let prompt_style = if m.prompt.is_empty() && m.field != ModalField::Prompt {
        Style::default().fg(Color::DarkGray)
    } else if m.field == ModalField::Prompt {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };
    let prompt_widget = Paragraph::new(Span::styled(format!("  {}", prompt_display), prompt_style));
    frame.render_widget(prompt_widget, Rect { x: inner.x, y, width: fw, height: 1 });
    if m.field == ModalField::Prompt {
        frame.set_cursor_position((inner.x + 2 + m.prompt.len() as u16, y));
    }
    y += 2;

    // --- Agent ---
    draw_field_label(frame, inner.x, y, fw, "Agent", m.field == ModalField::Agent);
    y += 1;
    let agent_style = if m.field == ModalField::Agent {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let arrows = if m.field == ModalField::Agent { "  ◂ " } else { "  " };
    let arrows_r = if m.field == ModalField::Agent { " ▸" } else { "" };
    let agent_widget = Paragraph::new(Line::from(vec![
        Span::styled(arrows, Style::default().fg(Color::DarkGray)),
        Span::styled(&m.agent, agent_style),
        Span::styled(arrows_r, Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(agent_widget, Rect { x: inner.x, y, width: fw, height: 1 });
    y += 2;

    // --- Issue URL ---
    draw_field_label(frame, inner.x, y, fw, "Issue URL (Linear, GitHub, Jira)", m.field == ModalField::Issue);
    y += 1;
    let issue_display = if m.issue.is_empty() && m.field != ModalField::Issue {
        "—"
    } else {
        &m.issue
    };
    let issue_style = if m.field == ModalField::Issue {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let issue_widget = Paragraph::new(Span::styled(format!("  {}", issue_display), issue_style));
    frame.render_widget(issue_widget, Rect { x: inner.x, y, width: fw, height: 1 });
    if m.field == ModalField::Issue {
        frame.set_cursor_position((inner.x + 2 + m.issue.len() as u16, y));
    }
    y += 2;

    // --- Auto-approve ---
    let aa_active = m.field == ModalField::AutoApprove;
    let aa_label_style = if aa_active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let check = if m.auto_approve { "✓" } else { " " };
    let check_style = if m.auto_approve {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let marker = if aa_active { "▸" } else { " " };
    let aa_widget = Paragraph::new(Line::from(vec![
        Span::styled(format!("{} ", marker), aa_label_style),
        Span::styled(format!("[{}] ", check), check_style),
        Span::styled("Auto-approve", aa_label_style),
        Span::styled("  skip permission prompts", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(aa_widget, Rect { x: inner.x, y, width: fw, height: 1 });
}

fn draw_field_label(frame: &mut Frame, x: u16, y: u16, w: u16, label: &str, active: bool) {
    let style = if active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let marker = if active { "▸" } else { " " };
    let widget = Paragraph::new(Span::styled(format!("{} {}", marker, label), style));
    frame.render_widget(widget, Rect { x, y, width: w, height: 1 });
}

fn draw_field_input(frame: &mut Frame, x: u16, y: u16, w: u16, value: &str, active: bool) {
    let style = if active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };
    let widget = Paragraph::new(Span::styled(format!("  {}", value), style));
    frame.render_widget(widget, Rect { x, y, width: w, height: 1 });
}

// ── Toast / Help ────────────────────────────────────────────────────────────

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

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = if app.mode == Mode::NewTask {
        Line::from(vec![
            Span::styled(
                " Tab",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(":next  "),
            Span::styled(
                "Enter",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(":create  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(":cancel"),
        ])
    } else if app.focus == Pane::Detail {
        Line::from(vec![
            Span::styled(
                " j/k",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(":navigate  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":expand diff  "),
            Span::styled("h/←", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":back  "),
            Span::styled("n", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":new  "),
            Span::styled("q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":quit"),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " Enter",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(":open  "),
            Span::styled("l/→", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":detail  "),
            Span::styled("b", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":bg  "),
            Span::styled("n", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":new  "),
            Span::styled("d", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":del  "),
            Span::styled("r", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":refresh  "),
            Span::styled("q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":quit"),
        ])
    };

    let bar = Paragraph::new(help).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(bar, area);
}
