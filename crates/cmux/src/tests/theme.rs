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
            ("glyph::CONNECTION", glyph::CONNECTION),
            ("glyph::EXITED", glyph::EXITED),
            ("glyph::PERMISSION", glyph::PERMISSION),
        ],
        "the help legend lists these as separate sidebar states",
    );
}

/// `CONNECTION` carries running and dormant alike, so the colour beside it is
/// the whole distinction. Two states drawn in one colour would be unreadable.
#[test]
fn the_connection_glyph_relies_on_a_colour_that_differs_per_state() {
    all_distinct(
        &[
            ("ACCENT_GREEN", ACCENT_GREEN),
            ("ACCENT_CYAN", ACCENT_CYAN),
            ("FG_DIM", FG_DIM),
        ],
        "the sidebar draws glyph::CONNECTION in each of these to mean a different state",
    );
}
