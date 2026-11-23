use crate::app::{AppState, COVER_HEIGHT_CELLS, COVER_WIDTH_CELLS, PlaybackStatus};
use crate::spotify::models::TrackLyrics;
use crate::ui::theme::{BADGE_FG, ERROR, MUTED, PRIMARY, TEXT, build_gradient_lyric_line};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::Image;

pub fn render_now_playing(frame: &mut Frame, state: &AppState, area: Rect) {
    let (badge_text, badge_bg) = match &state.playback_status {
        PlaybackStatus::Loading => (" [LOADING] ", TEXT),
        PlaybackStatus::Playing => (" [PLAYING] ", PRIMARY),
        PlaybackStatus::Paused | PlaybackStatus::Idle => (" [PAUSED] ", MUTED),
        PlaybackStatus::Failed(_) => (" [ERROR] ", ERROR),
    };
    let status_badge = Span::styled(
        badge_text,
        Style::default()
            .bg(badge_bg)
            .fg(BADGE_FG)
            .add_modifier(Modifier::BOLD),
    );

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

    let pad_top: u16 = if inner_area.height >= 7 { 1 } else { 0 };
    let pad_left: u16 = if inner_area.width >= 30 {
        2
    } else if inner_area.width > 2 {
        1
    } else {
        0
    };
    let pad_right: u16 = if inner_area.width >= COVER_WIDTH_CELLS + 18 {
        2
    } else if inner_area.width > 2 {
        1
    } else {
        0
    };

    let padded_area = Rect {
        x: inner_area.x.saturating_add(pad_left),
        y: inner_area.y.saturating_add(pad_top),
        width: inner_area.width.saturating_sub(pad_left + pad_right),
        height: inner_area.height.saturating_sub(pad_top),
    };

    if padded_area.width == 0 || padded_area.height == 0 {
        return;
    }

    let header_height = COVER_HEIGHT_CELLS.min(padded_area.height);
    let top_area = Rect {
        x: padded_area.x,
        y: padded_area.y,
        width: padded_area.width,
        height: header_height,
    };

    let cover_gap: u16 = 2;
    let can_fit_cover = padded_area.width >= COVER_WIDTH_CELLS + cover_gap + 10
        && top_area.height >= COVER_HEIGHT_CELLS;

    if can_fit_cover && let Some(ref proto) = state.cover_image {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(10),
                Constraint::Length(cover_gap),
                Constraint::Length(COVER_WIDTH_CELLS),
            ])
            .split(top_area);

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
        frame.render_widget(Paragraph::new(card_text), top_area);
    }

    if let PlaybackStatus::Failed(ref err) = state.playback_status
        && padded_area.height > header_height
    {
        let reserved_for_lyrics = if padded_area.height >= header_height + 4 {
            3
        } else {
            0
        };
        let available_below_header = padded_area
            .height
            .saturating_sub(header_height + reserved_for_lyrics);
        let error_offset = if available_below_header >= 2 { 1 } else { 0 };
        let error_height = available_below_header.saturating_sub(error_offset);
        if error_height > 0 {
            let error_area = Rect {
                x: padded_area.x,
                y: padded_area.y.saturating_add(header_height + error_offset),
                width: padded_area.width,
                height: error_height,
            };
            let error_line = Line::from(vec![
                Span::styled(
                    "Error:  ",
                    Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
                ),
                Span::styled(err.as_str(), Style::default().fg(ERROR)),
            ]);
            frame.render_widget(Paragraph::new(error_line).wrap(Wrap { trim: true }), error_area);
        }
    }

    if padded_area.height >= header_height + 3 && padded_area.width >= 6 {
        let lyrics_area = Rect {
            x: padded_area.x,
            y: padded_area.y + padded_area.height - 3,
            width: padded_area.width,
            height: 3,
        };
        render_lyrics_panel(frame, state, lyrics_area);
    }
}

fn render_lyrics_panel(frame: &mut Frame, state: &AppState, area: Rect) {
    let lyrics_title = match &state.lyrics {
        TrackLyrics::Synced(_) => " Lyrics [Synced] ".to_string(),
        TrackLyrics::Unsynced(lines) if !lines.is_empty() => {
            let idx = state.lyrics_manual_index.min(lines.len() - 1);
            format!(" Lyrics [{}/{} | w/s] ", idx + 1, lines.len())
        }
        _ => " Lyrics ".to_string(),
    };

    let lyrics_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title(lyrics_title)
        .title_style(Style::default().fg(PRIMARY));

    let lyrics_inner = lyrics_block.inner(area);
    frame.render_widget(lyrics_block, area);

    if lyrics_inner.width == 0 || lyrics_inner.height == 0 {
        return;
    }

    let (lyric_text, is_lyric_text) = state.current_lyric_display();
    let line = if is_lyric_text {
        build_gradient_lyric_line(
            &lyric_text,
            state.lyrics_gradient_phase,
            lyrics_inner.width as usize,
        )
    } else {
        let max_w = lyrics_inner.width as usize;
        let truncated: String = lyric_text.chars().take(max_w).collect();
        Line::from(Span::styled(truncated, Style::default().fg(MUTED)))
    };

    let single_line_area = Rect {
        x: lyrics_inner.x,
        y: lyrics_inner.y,
        width: lyrics_inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(line).alignment(Alignment::Center),
        single_line_area,
    );
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
    let queue_info = if state.connect_active
        && let Some((pos, total)) = state.connect_queue_position
    {
        format!("{pos} / {total} tracks")
    } else if !state.queue.is_empty() {
        format!("{} / {} tracks", state.queue_index + 1, state.queue.len())
    } else if let Some((pos, total)) = state.connect_queue_position {
        format!("{pos} / {total} tracks")
    } else {
        "Empty".to_string()
    };
    let pipeline_color = if state.audio_error.is_some() {
        ERROR
    } else {
        TEXT
    };
    let (connect_label, connect_color) = if state.connect_active {
        let label = match state.connect_client.as_deref() {
            Some(client) if !client.is_empty() => format!("Active (via {client})"),
            _ => "Active".to_string(),
        };
        (label, PRIMARY)
    } else if state.connect_ready {
        ("Standby (ready for remote)".to_string(), TEXT)
    } else {
        ("Disabled".to_string(), MUTED)
    };
    let text = vec![
        Line::from(vec![
            Span::styled("Audio Pipeline: ", Style::default().fg(MUTED)),
            Span::styled(
                format!("librespot -> {}", state.audio_backend_label),
                Style::default().fg(pipeline_color),
            ),
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
        Line::from(vec![
            Span::styled("Connect:        ", Style::default().fg(MUTED)),
            Span::styled(connect_label, Style::default().fg(connect_color)),
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
