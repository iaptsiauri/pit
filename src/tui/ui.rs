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

    // Draw modal overlay
    if app.mode == Mode::NewTask {
        draw_modal(frame, app);
    }

    // Draw error at the bottom of the task list area
    if let Some(ref err) = app.error {
        let err_area = Rect {
            x: chunks[0].x + 1,
            y: chunks[0].y + chunks[0].height.saturating_sub(2),
            width: chunks[0].width.saturating_sub(2),
            height: 1,
        };
        let err_widget = Paragraph::new(Span::styled(
            format!(" ✗ {}", err),
            Style::default().fg(Color::Red),
        ));
        frame.render_widget(err_widget, err_area);
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
                // Show first 30 chars of prompt
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

    // Center the modal: 60 wide, 13 tall
    let modal_width = 60u16.min(area.width.saturating_sub(4));
    let modal_height = 13u16.min(area.height.saturating_sub(2));

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

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    // Modal border
    let block = Block::default()
        .title(" New Task ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(block, modal_area);

    // Inner area (inside the border)
    let inner = Rect {
        x: modal_area.x + 2,
        y: modal_area.y + 1,
        width: modal_area.width.saturating_sub(4),
        height: modal_area.height.saturating_sub(2),
    };

    // Layout: 3 fields (label + input each) + help line
    let fields = [
        (ModalField::Name, &app.modal.name),
        (ModalField::Prompt, &app.modal.prompt),
        (ModalField::Issue, &app.modal.issue),
    ];

    let mut y = inner.y;
    let field_width = inner.width;

    for (field, value) in &fields {
        let is_active = app.modal.field == *field;

        // Label
        let label_style = if is_active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let label_text = if is_active {
            format!("▸ {}", field.label())
        } else {
            format!("  {}", field.label())
        };

        let label = Paragraph::new(Span::styled(label_text, label_style));
        let label_area = Rect {
            x: inner.x,
            y,
            width: field_width,
            height: 1,
        };
        frame.render_widget(label, label_area);
        y += 1;

        // Input value
        let input_style = if is_active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };

        let display_value = if value.is_empty() && !is_active {
            match field {
                ModalField::Issue => "—".to_string(),
                _ => "—".to_string(),
            }
        } else {
            value.to_string()
        };

        let input = Paragraph::new(Span::styled(format!("  {}", display_value), input_style));
        let input_area = Rect {
            x: inner.x,
            y,
            width: field_width,
            height: 1,
        };
        frame.render_widget(input, input_area);

        // Cursor for active field
        if is_active {
            frame.set_cursor_position((
                inner.x + 2 + value.len() as u16,
                y,
            ));
        }

        y += 2; // blank line between fields
    }
}

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = if app.mode == Mode::NewTask {
        Line::from(vec![
            Span::styled(
                " Tab",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":next field  "),
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
                "b",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":background  "),
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
            Span::raw(":delete  "),
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

    let bar = Paragraph::new(help).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(bar, area);
}
