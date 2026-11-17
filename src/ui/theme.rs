use ratatui::style::Color;

/// Primary dominant accent color (#EC99FF — pink-violet)
pub const PRIMARY: Color = Color::Rgb(236, 153, 255);

/// Secondary light color for main text (#DDE6F6)
pub const TEXT: Color = Color::Rgb(221, 230, 246);

/// Tertiary muted blue for secondary accents, labels, borders, and inactive elements (#577CAB)
pub const MUTED: Color = Color::Rgb(87, 124, 171);

/// Dark background highlight for selected list rows (derived from palette)
pub const BG_HIGHLIGHT: Color = Color::Rgb(45, 40, 60);

/// High-contrast dark foreground for badges on colored backgrounds
pub const BADGE_FG: Color = Color::Rgb(20, 20, 30);
