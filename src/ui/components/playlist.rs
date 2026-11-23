use crate::app::{AppState, InputMode};
use crate::spotify::models::{
    AlbumItem, BrowserView, LibraryCategory, PlaylistItem, SearchCategory, TrackMetadata,
};
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

pub fn compute_list_window(total: usize, selected: usize, viewport_height: usize) -> (usize, usize) {
    if total == 0 || viewport_height == 0 {
        return (0, 0);
    }
    let height = viewport_height.max(1).min(total);
    let clamped_selected = selected.min(total - 1);
    let half = height / 2;
    let mut start = clamped_selected.saturating_sub(half);
    if start + height > total {
        start = total.saturating_sub(height);
    }
    let end = (start + height).min(total);
    (start, end)
}

fn format_track_line(
    idx: usize,
    num_width: usize,
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
        Span::styled(
            format!("{prefix}{:>width$}. ", idx + 1, width = num_width),
            prefix_style,
        ),
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

fn format_album_line(idx: usize, num_width: usize, album: &AlbumItem) -> ListItem<'static> {
    let meta_str = match (album.total_tracks, album.year) {
        (t, Some(y)) if t > 0 => format!(" ({y}, {t} tracks)   "),
        (t, None) if t > 0 => format!(" ({t} tracks)   "),
        (_, Some(y)) => format!(" ({y})   "),
        _ => "   ".to_string(),
    };
    let line = Line::from(vec![
        Span::styled(
            format!("  {:>width$}. ", idx + 1, width = num_width),
            Style::default().fg(MUTED),
        ),
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
    ListItem::new(line)
}

fn format_playlist_line(idx: usize, num_width: usize, pl: &PlaylistItem) -> ListItem<'static> {
    let tracks_meta = if pl.total_tracks > 0 {
        format!("({} tracks)   ", pl.total_tracks)
    } else {
        "   ".to_string()
    };
    let line = Line::from(vec![
        Span::styled(
            format!("  {:>width$}. ", idx + 1, width = num_width),
            Style::default().fg(MUTED),
        ),
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

    if state.browser_view == BrowserView::Library && !is_editing {
        let title = if state.library_loading || state.library.liked_loading_more {
            " My Library [Loading...] "
        } else {
            " My Library "
        };
        let line = Line::from(vec![
            Span::styled(
                " [LIBRARY] ",
                Style::default()
                    .bg(PRIMARY)
                    .fg(BADGE_FG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", state.username),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(MUTED)),
            Span::styled(
                "[r] Refresh  [/] Search  [l] Switch to Search",
                Style::default().fg(MUTED),
            ),
        ]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(PRIMARY))
            .title(title)
            .title_style(Style::default().fg(TEXT).add_modifier(Modifier::BOLD));
        frame.render_widget(Paragraph::new(line).block(block), area);
        return;
    }

    let border_color = if is_editing { PRIMARY } else { MUTED };

    let title = if state.is_loading {
        " Search [Loading...] "
    } else if is_editing {
        " Search (Type query & press Enter | Esc to cancel) "
    } else {
        " Search (Press [/] to search | [l] Library) "
    };

    let content_line = if state.search_query.is_empty() && !is_editing {
        Line::from(Span::styled(
            "Press [/] to type a search query or [l] to open your Library...",
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
        let loaded = col.tracks.len();
        let count_info = if col.is_loading_more {
            match col.total_tracks {
                Some(total) if total > loaded => format!("{loaded} / {total} tracks — loading..."),
                _ => format!("{loaded}+ tracks — loading..."),
            }
        } else {
            format!("{loaded} tracks")
        };

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
                format!("— {} ({count_info})  ", col.subtitle),
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

    if state.browser_view == BrowserView::Library {
        let liked_len = state.library.liked_tracks.len();
        let liked_label = if state.library.liked_loading_more {
            match state.library.liked_total_expected {
                Some(total) if total > liked_len => {
                    format!("[2] Liked Songs ({liked_len}/{total}...)")
                }
                _ => format!("[2] Liked Songs ({liked_len}+...)"),
            }
        } else {
            format!("[2] Liked Songs ({liked_len})")
        };

        let tabs = [
            (
                state.library_category == LibraryCategory::Playlists,
                format!("[1] Playlists ({})", state.library.playlists.len()),
            ),
            (
                state.library_category == LibraryCategory::LikedSongs,
                liked_label,
            ),
            (
                state.library_category == LibraryCategory::SavedAlbums,
                format!("[3] Saved Albums ({})", state.library.saved_albums.len()),
            ),
        ];
        render_tab_bar(frame, area, " Library (Switch with [Tab] or [1-3]) ", &tabs);
        return;
    }

    let tabs = [
        (
            state.search_category == SearchCategory::Tracks,
            format!("[1] Tracks ({})", state.search_results.tracks.len()),
        ),
        (
            state.search_category == SearchCategory::Albums,
            format!("[2] Albums ({})", state.search_results.albums.len()),
        ),
        (
            state.search_category == SearchCategory::Artists,
            format!("[3] Artists ({})", state.search_results.artists.len()),
        ),
        (
            state.search_category == SearchCategory::Playlists,
            format!("[4] Playlists ({})", state.search_results.playlists.len()),
        ),
    ];
    render_tab_bar(
        frame,
        area,
        " Categories (Switch with [Tab] or [1-4]) ",
        &tabs,
    );
}

fn render_tab_bar(frame: &mut Frame, area: Rect, title: &str, tabs: &[(bool, String)]) {
    let mut spans = Vec::new();
    for (idx, (is_active, label)) in tabs.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("   ", Style::default().fg(MUTED)));
        }
        let style = if *is_active {
            Style::default()
                .bg(PRIMARY)
                .fg(BADGE_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title(title)
        .title_style(Style::default().fg(TEXT));
    frame.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn render_results_list(frame: &mut Frame, state: &AppState, area: Rect) {
    let total = state.current_list_len();
    let selected = state.active_selected_index();
    let viewport_height = area.height.saturating_sub(2).max(1) as usize;
    let (start, end) = compute_list_window(total, selected, viewport_height);
    let num_width = total.max(1).to_string().len().max(2);

    let mut items: Vec<ListItem> = Vec::with_capacity(end.saturating_sub(start));

    if let Some(ref col) = state.opened_collection {
        for idx in start..end {
            if let Some(track) = col.tracks.get(idx) {
                items.push(format_track_line(idx, num_width, track, &state.track_uri));
            }
        }
    } else if state.browser_view == BrowserView::Library {
        match state.library_category {
            LibraryCategory::Playlists => {
                for idx in start..end {
                    if let Some(pl) = state.library.playlists.get(idx) {
                        items.push(format_playlist_line(idx, num_width, pl));
                    }
                }
            }
            LibraryCategory::LikedSongs => {
                for idx in start..end {
                    if let Some(track) = state.library.liked_tracks.get(idx) {
                        items.push(format_track_line(idx, num_width, track, &state.track_uri));
                    }
                }
            }
            LibraryCategory::SavedAlbums => {
                for idx in start..end {
                    if let Some(album) = state.library.saved_albums.get(idx) {
                        items.push(format_album_line(idx, num_width, album));
                    }
                }
            }
        }
    } else {
        match state.search_category {
            SearchCategory::Tracks => {
                for idx in start..end {
                    if let Some(track) = state.search_results.tracks.get(idx) {
                        items.push(format_track_line(idx, num_width, track, &state.track_uri));
                    }
                }
            }
            SearchCategory::Albums => {
                for idx in start..end {
                    if let Some(album) = state.search_results.albums.get(idx) {
                        items.push(format_album_line(idx, num_width, album));
                    }
                }
            }
            SearchCategory::Artists => {
                for idx in start..end {
                    if let Some(artist) = state.search_results.artists.get(idx) {
                        let line = Line::from(vec![
                            Span::styled(
                                format!("  {:>width$}. ", idx + 1, width = num_width),
                                Style::default().fg(MUTED),
                            ),
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
            }
            SearchCategory::Playlists => {
                for idx in start..end {
                    if let Some(pl) = state.search_results.playlists.get(idx) {
                        items.push(format_playlist_line(idx, num_width, pl));
                    }
                }
            }
        }
    }

    let block_title = if let Some(ref msg) = state.status_message {
        format!(" Results — {msg} ")
    } else if let Some(ref col) = state.opened_collection {
        format!(" {} Tracks ({}) ", col.kind.label(), col.tracks.len())
    } else if state.browser_view == BrowserView::Library {
        format!(" {} ", state.library_category.title())
    } else {
        format!(" {} ", state.search_category.title())
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title(block_title)
        .title_style(Style::default().fg(TEXT));

    if total == 0 || items.is_empty() {
        let empty_msg = if state.is_loading || state.library_loading || state.library.liked_loading_more {
            "Loading from Spotify..."
        } else if state.browser_view == BrowserView::Library {
            "No items in this library section. Press [r] to refresh or [/] to search."
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
    list_state.select(Some(selected.saturating_sub(start)));
    frame.render_stateful_widget(list, area, &mut list_state);
}
