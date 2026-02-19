use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::core::task::Status;

use super::app::{App, Mode};

pub fn draw(frame: &mut Frame, app: &App) {
    let show_input = app.mode != Mode::Normal;
    let show_error = app.error.is_some();

    let mut constraints = vec![Constraint::Min(5)]; // Task list
    if show_error {
        constraints.push(Constraint::Length(1)); // Error line
    }
    if show_input {
        constraints.push(Constraint::Length(3)); // Input bar
    }
    constraints.push(Constraint::Length(3)); // Help bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    let mut idx = 0;

    draw_task_list(frame, app, chunks[idx]);
    idx += 1;

    if show_error {
        draw_error(frame, app.error.as_deref().unwrap_or(""), chunks[idx]);
        idx += 1;
    }

    if show_input {
        draw_input(frame, app, chunks[idx]);
        idx += 1;
    }

    draw_help_bar(frame, app, chunks[idx]);
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

            let line = Line::from(vec![
                Span::styled(format!(" {} ", status_icon), status_style),
                Span::styled(
                    format!("{:<24}", t.name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{:<10}", t.status), status_style),
                Span::styled(&t.branch, Style::default().fg(Color::DarkGray)),
            ]);
            ListItem::new(line)
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

fn draw_error(frame: &mut Frame, msg: &str, area: Rect) {
    let err = Paragraph::new(Span::styled(
        format!(" ✗ {}", msg),
        Style::default().fg(Color::Red),
    ));
    frame.render_widget(err, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let input = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .title(format!(" {} ", app.input_label))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(input, area);

    // Place cursor at end of input
    frame.set_cursor_position((
        area.x + app.input.len() as u16 + 1,
        area.y + 1,
    ));
}

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = if app.mode != Mode::Normal {
        Line::from(vec![
            Span::styled(
                " Enter",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":confirm  "),
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
