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
    /// Also stands in for dormant, dimmed rather than swapped for another
    /// glyph — the state differs in degree, not in kind.
    pub const IDLE: &str = "○";
    pub const EXITED: &str = "✕";
    pub const RESUMED: &str = "↺";
    /// Permission prompt waiting on user (claude blocked).
    pub const PERMISSION: &str = "⚠";
    /// Session launched with `--dangerously-skip-permissions`.
    pub const DANGER: &str = "⚠";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert every pair in `named` differs, naming both sides on failure.
    fn all_distinct<T: PartialEq + std::fmt::Debug>(named: &[(&str, T)], why: &str) {
        for (i, (a_name, a)) in named.iter().enumerate() {
            for (b_name, b) in &named[i + 1..] {
                assert_ne!(a, b, "{a_name} and {b_name} are the same value; {why}");
            }
        }
    }

    #[test]
    fn spinner_frame_advances_on_every_tick() {
        let cycle = SPINNER_FRAMES.len() as u64;
        for tick in 0..cycle {
            assert_ne!(
                spinner_frame(tick),
                spinner_frame(tick + 1),
                "tick {tick} and tick {} draw the same glyph, so the spinner stalls",
                tick + 1
            );
        }
    }

    #[test]
    fn spinner_frame_uses_every_frame_once_per_cycle() {
        let cycle = SPINNER_FRAMES.len();
        assert_eq!(cycle, 10, "the spinner cycle length changed");

        let seen: Vec<char> = (0..cycle as u64).map(spinner_frame).collect();
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            cycle,
            "one cycle drew {seen:?}, which repeats a frame"
        );
        assert_eq!(
            seen[0], SPINNER_FRAMES[0],
            "tick 0 should draw the first frame"
        );
    }

    #[test]
    fn spinner_frame_wraps_back_to_the_first_frame() {
        let cycle = SPINNER_FRAMES.len() as u64;
        assert_eq!(
            spinner_frame(cycle),
            spinner_frame(0),
            "tick {cycle} should wrap to the first frame"
        );
        assert_eq!(
            spinner_frame(cycle * 3 + 4),
            spinner_frame(4),
            "a tick several cycles in should land on the same frame as its remainder"
        );
    }

    #[test]
    fn spinner_frame_wraps_a_large_tick_without_panicking() {
        let cycle = SPINNER_FRAMES.len() as u64;
        assert_eq!(
            spinner_frame(u64::MAX),
            spinner_frame(u64::MAX % cycle),
            "a large tick did not wrap onto its remainder"
        );
        assert_eq!(
            spinner_frame(1_000_003),
            spinner_frame(3),
            "tick 1000003 should draw the same frame as tick 3"
        );
    }

    #[test]
    fn tile_border_states_never_share_a_colour() {
        all_distinct(
            &[
                ("BORDER_IDLE", BORDER_IDLE),
                ("BORDER_FOCUS", BORDER_FOCUS),
                ("BORDER_DEAD", BORDER_DEAD),
                ("ACCENT_MAGENTA", ACCENT_MAGENTA),
            ],
            "a tile border is the only cue for idle, focused, dead and zoomed",
        );
    }

    #[test]
    fn the_attention_pulse_alternates_two_colours() {
        assert_ne!(
            BORDER_DEAD, ACCENT_RED_DIM,
            "the attention border pulses between these two, so equal values leave it static"
        );
    }

    #[test]
    fn sidebar_badge_colours_never_collide() {
        all_distinct(
            &[
                ("ACCENT_RED", ACCENT_RED),
                ("ACCENT_GREEN", ACCENT_GREEN),
                ("ACCENT_CYAN", ACCENT_CYAN),
                ("ACCENT_YELLOW", ACCENT_YELLOW),
                ("FG_DIM", FG_DIM),
            ],
            "idle, recent and dormant all draw the same glyph, so colour is the only cue",
        );
    }

    #[test]
    fn text_weights_never_collide() {
        all_distinct(
            &[("FG", FG), ("FG_DIM", FG_DIM), ("FG_MUTED", FG_MUTED)],
            "a sidebar row stacks all three weights on consecutive lines",
        );
    }

    #[test]
    fn selected_text_stays_visible_against_its_background() {
        assert_ne!(
            FG, SELECTION_BG,
            "a selected cell with a Reset foreground draws FG on SELECTION_BG"
        );
        assert_ne!(FG, BG_ACTIVE, "a highlighted row draws FG on BG_ACTIVE");
    }

    /// `glyph::DANGER` is left out: it shares its symbol with `PERMISSION` by
    /// design and never renders in the badge column.
    #[test]
    fn legend_glyphs_never_collide() {
        all_distinct(
            &[
                ("glyph::IDLE", glyph::IDLE),
                ("glyph::EXITED", glyph::EXITED),
                ("glyph::RESUMED", glyph::RESUMED),
                ("glyph::PERMISSION", glyph::PERMISSION),
            ],
            "the help legend lists these four as separate sidebar states",
        );
    }

    #[test]
    fn no_spinner_frame_looks_like_a_status_glyph() {
        for (i, frame) in SPINNER_FRAMES.iter().enumerate() {
            let drawn = frame.to_string();
            for (name, glyph) in [
                ("glyph::IDLE", glyph::IDLE),
                ("glyph::EXITED", glyph::EXITED),
                ("glyph::RESUMED", glyph::RESUMED),
                ("glyph::PERMISSION", glyph::PERMISSION),
            ] {
                assert_ne!(
                    drawn, glyph,
                    "spinner frame {i} draws {name}, so a busy session reads as that state"
                );
            }
        }
    }
}
