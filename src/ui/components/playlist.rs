use crate::app::{AppState, InputMode};
use crate::spotify::models::{SearchCategory, TrackMetadata};
use crate::ui::theme::{BADGE_FG, BG_HIGHLIGHT, MUTED, PRIMARY, TEXT};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

fn format_duration(duration_ms: u32) -> String {
    let total_sec = duration_ms / 1000;
    format!("{:02}:{:02}", total_sec / 60, total_sec % 60)
}

fn format_track_line(
    idx: usize,
    track: &TrackMetadata,
    current_playing_uri: &str,
) -> ListItem<'static> {
    let is_current = !current_playing_uri.is_empty() && track.uri == current_playing_uri;
    let prefix = if is_current { "▶ " } else { "  " };
    let prefix_style = if is_current {
        Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    };
    let title_style = if is_current {
        Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    };

    let line = Line::from(vec![
        Span::styled(format!("{prefix}{:>2}. ", idx + 1), prefix_style),
        Span::styled(track.title.clone(), title_style),
        Span::styled(" — ", Style::default().fg(MUTED)),
        Span::styled(track.artist.clone(), Style::default().fg(MUTED)),
        Span::styled(format!(" ({}) ", track.album), Style::default().fg(MUTED)),
        Span::styled(
            format!("[{}]", format_duration(track.duration_ms)),
            Style::default().fg(MUTED),
        ),
    ]);

    ListItem::new(line)
}

pub fn render_search_browser(frame: &mut Frame, state: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(4),
        ])
        .split(area);

    render_search_input(frame, state, chunks[0]);
    render_tabs_or_collection_header(frame, state, chunks[1]);
    render_results_list(frame, state, chunks[2]);
}

fn render_search_input(frame: &mut Frame, state: &AppState, area: Rect) {
    let is_editing = state.input_mode == InputMode::EditingSearch;
    let border_color = if is_editing { PRIMARY } else { MUTED };

    let title = if state.is_loading {
        " Search [Loading...] "
    } else if is_editing {
        " Search (Type query & press Enter | Esc to cancel) "
    } else {
        " Search (Press [/] to search songs, albums, artists, playlists) "
    };

    let content_line = if state.search_query.is_empty() && !is_editing {
        Line::from(Span::styled(
            "Press [/] to type a search query...",
            Style::default().fg(MUTED),
        ))
    } else {
        let mut spans = vec![Span::styled(
            state.search_query.clone(),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )];
        if is_editing {
            spans.push(Span::styled(
                "_",
                Style::default()
                    .fg(PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Line::from(spans)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title)
        .title_style(if is_editing {
            Style::default()
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        });

    let widget = Paragraph::new(content_line).block(block);
    frame.render_widget(widget, area);
}

fn render_tabs_or_collection_header(frame: &mut Frame, state: &AppState, area: Rect) {
    if let Some(ref col) = state.opened_collection {
        let line = Line::from(vec![
            Span::styled(
                format!(" [{}] ", col.kind.label()),
                Style::default()
                    .bg(PRIMARY)
                    .fg(BADGE_FG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", col.title),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("— {} ({} tracks)  ", col.subtitle, col.tracks.len()),
                Style::default().fg(MUTED),
            ),
            Span::styled(
                "[Enter] Play Track  [p] Play All  [Backspace/Esc] Back",
                Style::default().fg(PRIMARY),
            ),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(MUTED))
            .title(" Opened Collection ")
            .title_style(Style::default().fg(TEXT));
        frame.render_widget(Paragraph::new(line).block(block), area);
        return;
    }

    let tabs = [
        (
            SearchCategory::Tracks,
            format!("[1] Tracks ({})", state.search_results.tracks.len()),
        ),
        (
            SearchCategory::Albums,
            format!("[2] Albums ({})", state.search_results.albums.len()),
        ),
        (
            SearchCategory::Artists,
            format!("[3] Artists ({})", state.search_results.artists.len()),
        ),
        (
            SearchCategory::Playlists,
            format!("[4] Playlists ({})", state.search_results.playlists.len()),
        ),
    ];

    let mut spans = Vec::new();
    for (idx, (cat, label)) in tabs.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("   ", Style::default().fg(MUTED)));
        }
        if *cat == state.search_category {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default()
                    .bg(PRIMARY)
                    .fg(BADGE_FG)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(MUTED),
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title(" Categories (Switch with [Tab] or [1-4]) ")
        .title_style(Style::default().fg(TEXT));
    frame.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn render_results_list(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut items: Vec<ListItem> = Vec::new();

    if let Some(ref col) = state.opened_collection {
        for (idx, track) in col.tracks.iter().enumerate() {
            items.push(format_track_line(idx, track, &state.track_uri));
        }
    } else {
        match state.search_category {
            SearchCategory::Tracks => {
                for (idx, track) in state.search_results.tracks.iter().enumerate() {
                    items.push(format_track_line(idx, track, &state.track_uri));
                }
            }
            SearchCategory::Albums => {
                for (idx, album) in state.search_results.albums.iter().enumerate() {
                    let meta_str = match (album.total_tracks, album.year) {
                        (t, Some(y)) if t > 0 => format!(" ({y}, {t} tracks)   "),
                        (t, None) if t > 0 => format!(" ({t} tracks)   "),
                        (_, Some(y)) => format!(" ({y})   "),
                        _ => "   ".to_string(),
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("  {:>2}. ", idx + 1), Style::default().fg(MUTED)),
                        Span::styled(
                            album.name.clone(),
                            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" — ", Style::default().fg(MUTED)),
                        Span::styled(album.artist.clone(), Style::default().fg(MUTED)),
                        Span::styled(meta_str, Style::default().fg(MUTED)),
                        Span::styled(
                            "[Enter: View Tracks | p: Play Album]",
                            Style::default().fg(MUTED),
                        ),
                    ]);
                    items.push(ListItem::new(line));
                }
            }
            SearchCategory::Artists => {
                for (idx, artist) in state.search_results.artists.iter().enumerate() {
                    let line = Line::from(vec![
                        Span::styled(format!("  {:>2}. ", idx + 1), Style::default().fg(MUTED)),
                        Span::styled(
                            artist.name.clone(),
                            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("   ", Style::default().fg(MUTED)),
                        Span::styled(
                            "[Enter: Top Tracks | p: Play Artist]",
                            Style::default().fg(MUTED),
                        ),
                    ]);
                    items.push(ListItem::new(line));
                }
            }
            SearchCategory::Playlists => {
                for (idx, pl) in state.search_results.playlists.iter().enumerate() {
                    let tracks_meta = if pl.total_tracks > 0 {
                        format!("({} tracks)   ", pl.total_tracks)
                    } else {
                        "   ".to_string()
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("  {:>2}. ", idx + 1), Style::default().fg(MUTED)),
                        Span::styled(
                            pl.name.clone(),
                            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" — by {} ", pl.owner),
                            Style::default().fg(MUTED),
                        ),
                        Span::styled(tracks_meta, Style::default().fg(MUTED)),
                        Span::styled(
                            "[Enter: View Tracks | p: Play Playlist]",
                            Style::default().fg(MUTED),
                        ),
                    ]);
                    items.push(ListItem::new(line));
                }
            }
        }
    }

    let block_title = if let Some(ref msg) = state.status_message {
        format!(" Results — {msg} ")
    } else if let Some(ref col) = state.opened_collection {
        format!(" {} Tracks ({}) ", col.kind.label(), col.tracks.len())
    } else {
        format!(" {} ", state.search_category.title())
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title(block_title)
        .title_style(Style::default().fg(TEXT));

    if items.is_empty() {
        let empty_msg = if state.is_loading {
            "Loading from Spotify..."
        } else {
            "No items to display. Press [/] to search for songs, albums, artists, or playlists."
        };
        let paragraph = Paragraph::new(Line::from(Span::styled(
            empty_msg,
            Style::default().fg(MUTED),
        )))
        .block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(BG_HIGHLIGHT)
                .fg(PRIMARY)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("❯ ");

    let mut list_state = ListState::default();
    list_state.select(Some(state.active_selected_index()));
    frame.render_stateful_widget(list, area, &mut list_state);
}
