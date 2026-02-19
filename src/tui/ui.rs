use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::core::task::Status;

use super::app::{App, ModalField, Mode};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(frame.area());

    draw_task_list(frame, app, chunks[0]);
    draw_help_bar(frame, app, chunks[1]);

    if app.mode == Mode::NewTask {
        draw_modal(frame, app);
    }

    if let Some(ref err) = app.error {
        draw_error_toast(frame, err);
    }
}

fn draw_task_list(frame: &mut Frame, app: &App, area: Rect) {
    if app.tasks.is_empty() {
        let msg = Paragraph::new("No tasks yet. Press n to create one.")
            .block(
                Block::default()
                    .title(" pit — coding agent orchestrator ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|t| {
            let status_style = match t.status {
                Status::Idle => Style::default().fg(Color::DarkGray),
                Status::Running => Style::default().fg(Color::Green),
                Status::Done => Style::default().fg(Color::Blue),
                Status::Error => Style::default().fg(Color::Red),
            };

            let status_icon = match t.status {
                Status::Idle => "○",
                Status::Running => "▶",
                Status::Done => "✓",
                Status::Error => "✗",
            };

            let mut spans = vec![
                Span::styled(format!(" {} ", status_icon), status_style),
                Span::styled(
                    format!("{:<24}", t.name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{:<10}", t.status), status_style),
            ];

            if !t.prompt.is_empty() {
                let preview: String = t.prompt.chars().take(30).collect();
                let suffix = if t.prompt.len() > 30 { "…" } else { "" };
                spans.push(Span::styled(
                    format!("{}{}", preview, suffix),
                    Style::default().fg(Color::DarkGray),
                ));
            } else {
                spans.push(Span::styled(
                    &t.branch,
                    Style::default().fg(Color::DarkGray),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" pit — coding agent orchestrator ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

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
    } else {
        Line::from(vec![
            Span::styled(
                " Enter",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(":open  "),
            Span::styled("b", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":background  "),
            Span::styled("n", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":new  "),
            Span::styled("d", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(":delete  "),
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
