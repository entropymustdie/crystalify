use crate::app::{AppState, COVER_HEIGHT_CELLS, COVER_WIDTH_CELLS};
use crate::ui::theme::{BADGE_FG, MUTED, PRIMARY, TEXT};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, Paragraph};
use ratatui::Frame;
use ratatui_image::Image;

pub fn render_now_playing(frame: &mut Frame, state: &AppState, area: Rect) {
    let status_badge = if state.is_playing {
        Span::styled(
            " [PLAYING] ",
            Style::default()
                .bg(PRIMARY)
                .fg(BADGE_FG)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " [PAUSED] ",
            Style::default()
                .bg(MUTED)
                .fg(BADGE_FG)
                .add_modifier(Modifier::BOLD),
        )
    };

    let card_text = vec![
        Line::from(vec![status_badge]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Title:  ", Style::default().fg(MUTED)),
            Span::styled(
                &state.track_title,
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Artist: ", Style::default().fg(MUTED)),
            Span::styled(&state.track_artist, Style::default().fg(PRIMARY)),
        ]),
        Line::from(vec![
            Span::styled("Album:  ", Style::default().fg(MUTED)),
            Span::styled(&state.track_album, Style::default().fg(TEXT)),
        ]),
    ];

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title(" Now Playing ")
        .title_style(Style::default().fg(TEXT));

    let inner_area = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    if inner_area.width == 0 || inner_area.height == 0 {
        return;
    }

    let can_fit_cover =
        inner_area.width >= COVER_WIDTH_CELLS + 14 && inner_area.height >= COVER_HEIGHT_CELLS;

    if can_fit_cover && let Some(ref proto) = state.cover_image {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(10),
                Constraint::Length(1),
                Constraint::Length(COVER_WIDTH_CELLS),
            ])
            .split(inner_area);

        frame.render_widget(Paragraph::new(card_text), horizontal[0]);

        let cover_rect = Rect {
            x: horizontal[2].x,
            y: horizontal[2].y,
            width: horizontal[2].width.min(COVER_WIDTH_CELLS),
            height: horizontal[2].height.min(COVER_HEIGHT_CELLS),
        };
        let image_widget = Image::new(proto);
        frame.render_widget(image_widget, cover_rect);
    } else {
        frame.render_widget(Paragraph::new(card_text), inner_area);
    }
}

pub fn render_progress(frame: &mut Frame, state: &AppState, area: Rect) {
    let elapsed_sec = state.position_ms / 1000;
    let total_sec = state.duration_ms / 1000;
    let ratio = if state.duration_ms > 0 {
        (state.position_ms as f64 / state.duration_ms as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let label = format!(
        "{:02}:{:02} / {:02}:{:02}",
        elapsed_sec / 60,
        elapsed_sec % 60,
        total_sec / 60,
        total_sec % 60
    );

    let widget = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(MUTED))
                .title(" Progress ")
                .title_style(Style::default().fg(TEXT)),
        )
        .gauge_style(Style::default().fg(PRIMARY).bg(MUTED))
        .ratio(ratio)
        .label(Span::styled(
            label,
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(widget, area);
}

pub fn render_details(frame: &mut Frame, state: &AppState, area: Rect) {
    let vol_percent = ((state.volume as f64 / u16::MAX as f64) * 100.0).round() as u8;
    let queue_info = if state.queue.is_empty() {
        "Empty".to_string()
    } else {
        format!("{} / {} tracks", state.queue_index + 1, state.queue.len())
    };
    let text = vec![
        Line::from(vec![
            Span::styled("Audio Pipeline: ", Style::default().fg(MUTED)),
            Span::styled("librespot -> PipeWire", Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Bitrate:        ", Style::default().fg(MUTED)),
            Span::styled("320 kbps (Vorbis)", Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Queue:          ", Style::default().fg(MUTED)),
            Span::styled(queue_info, Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Volume:         ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{}%  ([-]/[+])", vol_percent),
                Style::default()
                    .fg(PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let widget = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(MUTED))
            .title(" Playback Details ")
            .title_style(Style::default().fg(TEXT)),
    );
    frame.render_widget(widget, area);
}
