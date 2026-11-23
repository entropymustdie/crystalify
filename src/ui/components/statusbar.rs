use crate::app::AppState;
use crate::ui::theme::{ERROR, MUTED, PRIMARY, TEXT};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render_header(frame: &mut Frame, state: &AppState, area: Rect) {
    let header_text = vec![Line::from(vec![
        Span::styled(" Device: ", Style::default().fg(MUTED)),
        Span::styled(
            &state.device_name,
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  |  User: ", Style::default().fg(MUTED)),
        Span::styled(
            &state.username,
            Style::default()
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  |  Audio Backend: ", Style::default().fg(MUTED)),
        Span::styled(
            &state.audio_backend_label,
            Style::default().fg(if state.audio_error.is_some() {
                ERROR
            } else {
                PRIMARY
            }),
        ),
    ])];

    let widget = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(MUTED)),
    );
    frame.render_widget(widget, area);
}

pub fn render_footer(frame: &mut Frame, area: Rect) {
    let footer_text = vec![Line::from(vec![
        Span::styled(
            " [/] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Search  ", Style::default().fg(TEXT)),
        Span::styled(
            "[l] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Library  ", Style::default().fg(TEXT)),
        Span::styled(
            "[Tab/1-4] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Tabs  ", Style::default().fg(TEXT)),
        Span::styled(
            "[Up/Down] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Select  ", Style::default().fg(TEXT)),
        Span::styled(
            "[Enter] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Play/Open  ", Style::default().fg(TEXT)),
        Span::styled(
            "[p] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Play All  ", Style::default().fg(TEXT)),
        Span::styled(
            "[n/b] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Next/Prev  ", Style::default().fg(TEXT)),
        Span::styled(
            "[Space] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Pause  ", Style::default().fg(TEXT)),
        Span::styled(
            "[w/s] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Lyrics  ", Style::default().fg(TEXT)),
        Span::styled(
            "[q] ",
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Quit", Style::default().fg(TEXT)),
    ])];

    let widget = Paragraph::new(footer_text)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(MUTED)),
        );
    frame.render_widget(widget, area);
}
