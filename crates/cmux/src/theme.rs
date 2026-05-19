use ratatui::style::Color;

// catppuccin-mocha inspired. Truecolor terminals get exact RGB; 256-color
// terminals get nearest match via ratatui/crossterm fallback.

pub const FG: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const FG_DIM: Color = Color::Rgb(0x7f, 0x84, 0x9c);
pub const FG_MUTED: Color = Color::Rgb(0x9c, 0xa0, 0xb0);

pub const BORDER_IDLE: Color = Color::Rgb(0x45, 0x47, 0x5a);
pub const BORDER_FOCUS: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const BORDER_DEAD: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

pub const ACCENT_GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const ACCENT_CYAN: Color = Color::Rgb(0x89, 0xdc, 0xeb);
pub const ACCENT_YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const ACCENT_PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
pub const ACCENT_RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
pub const ACCENT_RED_DIM: Color = Color::Rgb(0xc9, 0x74, 0x8d);
pub const ACCENT_MAGENTA: Color = Color::Rgb(0xcb, 0xa6, 0xf7);

pub const BG_ACTIVE: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const SELECTION_BG: Color = Color::Rgb(0x58, 0x5b, 0x70);

const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn spinner_frame(tick: u64) -> char {
    SPINNER_FRAMES[(tick % SPINNER_FRAMES.len() as u64) as usize]
}

/// Single-char status badges used across the sidebar + tile chrome. Keeping
/// them as named consts means the legend in `popups::help` and the row
/// renderer in `dashboard` agree by reference, not by coincidence.
pub mod glyph {
    pub const IDLE: &str = "○";
    pub const DORMANT: &str = "·";
    pub const EXITED: &str = "✕";
    pub const RESUMED: &str = "↺";
    /// Permission prompt waiting on user (claude blocked).
    pub const PERMISSION: &str = "⚠";
    /// Session launched with `--dangerously-skip-permissions`.
    pub const DANGER: &str = "⚠";
}
