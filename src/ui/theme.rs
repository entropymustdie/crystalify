use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub const PRIMARY: Color = Color::Rgb(236, 153, 255);
pub const TEXT: Color = Color::Rgb(221, 230, 246);
pub const MUTED: Color = Color::Rgb(87, 124, 171);
pub const BG_HIGHLIGHT: Color = Color::Rgb(45, 40, 60);
pub const BADGE_FG: Color = Color::Rgb(20, 20, 30);
pub const ERROR: Color = Color::Rgb(255, 107, 129);

pub fn sample_lyrics_gradient(t: f32) -> Color {
    let stops: [(f32, f32, f32); 3] = [
        (186.0, 104.0, 255.0),
        (77.0, 121.0, 255.0),
        (92.0, 225.0, 255.0),
    ];
    let wrapped = t.rem_euclid(1.0);
    let scaled = wrapped * 3.0;
    let idx = (scaled.floor() as usize) % 3;
    let next_idx = (idx + 1) % 3;
    let local = scaled - scaled.floor();
    let smooth = 0.5 - 0.5 * (local * std::f32::consts::PI).cos();

    let (r0, g0, b0) = stops[idx];
    let (r1, g1, b1) = stops[next_idx];
    let r = (r0 + (r1 - r0) * smooth).round().clamp(0.0, 255.0) as u8;
    let g = (g0 + (g1 - g0) * smooth).round().clamp(0.0, 255.0) as u8;
    let b = (b0 + (b1 - b0) * smooth).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(r, g, b)
}

pub fn build_gradient_lyric_line(text: &str, phase: f32, max_width: usize) -> Line<'static> {
    if max_width == 0 || text.is_empty() {
        return Line::from("");
    }
    let raw_chars: Vec<char> = text.chars().collect();
    let display_chars: Vec<char> = if raw_chars.len() > max_width {
        if max_width > 1 {
            let mut truncated: Vec<char> = raw_chars.into_iter().take(max_width - 1).collect();
            truncated.push('…');
            truncated
        } else {
            vec!['…']
        }
    } else {
        raw_chars
    };

    let span_len = (display_chars.len().max(16)) as f32;
    let spans: Vec<Span<'static>> = display_chars
        .into_iter()
        .enumerate()
        .map(|(i, ch)| {
            let t = (i as f32 / span_len - phase).rem_euclid(1.0);
            let color = sample_lyrics_gradient(t);
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_lyrics_gradient_and_flowing_line_animation() {
        assert_eq!(sample_lyrics_gradient(0.0), Color::Rgb(186, 104, 255));
        assert_eq!(sample_lyrics_gradient(1.0 / 3.0), Color::Rgb(77, 121, 255));
        assert_eq!(sample_lyrics_gradient(2.0 / 3.0), Color::Rgb(92, 225, 255));
        assert_eq!(sample_lyrics_gradient(1.0), Color::Rgb(186, 104, 255));

        let line_a = build_gradient_lyric_line("I know the pieces fit", 0.0, 40);
        let line_b = build_gradient_lyric_line("I know the pieces fit", 0.25, 40);
        assert_eq!(line_a.spans.len(), "I know the pieces fit".chars().count());

        let colors_a: Vec<Option<Color>> = line_a.spans.iter().map(|s| s.style.fg).collect();
        let colors_b: Vec<Option<Color>> = line_b.spans.iter().map(|s| s.style.fg).collect();
        assert_ne!(colors_a[0], colors_a[colors_a.len() - 1]);
        assert_ne!(colors_a, colors_b);

        let truncated = build_gradient_lyric_line("This is a very long lyric line that must fit", 0.0, 12);
        let rendered: String = truncated.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rendered.chars().count(), 12);
        assert!(rendered.ends_with('…'));
    }
}
