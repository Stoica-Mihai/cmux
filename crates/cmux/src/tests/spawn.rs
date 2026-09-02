use super::*;
use crate::ui::popups::harness::{
    assert_inside, assert_legible, chips, painted_bounds, render, row, text, try_render,
};
use ratatui::style::Color;
use std::path::PathBuf;

const FULL: Rect = Rect {
    x: 0,
    y: 0,
    width: 80,
    height: 24,
};
const ROOT: &str = "/tmp/cmux-spawn-test";

/// A settled picker over a fixed listing. Built through `new` and then
/// overwritten, because the reader thread it starts would otherwise decide
/// what the test sees.
fn state(names: &[&str], selected: usize, dangerous: bool) -> SpawnState {
    let mut spawn = SpawnState::new(PathBuf::from(ROOT));
    spawn.entries = names.iter().map(|n| PathBuf::from(ROOT).join(n)).collect();
    spawn.selected = selected;
    spawn.dangerous = dangerous;
    spawn.reading = false;
    spawn
}

#[test]
fn it_shows_the_folder_it_would_spawn_in_and_what_is_under_it() {
    let spawn = state(&["docs", "src"], 1, false);
    let buf = render(80, 24, |f| draw(f, FULL, &spawn));
    let out = text(&buf);

    assert!(
        out.contains("Spawn claude in a folder"),
        "no popup title:\n{out}"
    );
    assert!(
        out.contains(&collapse_cwd(ROOT)),
        "the cwd is not shown:\n{out}"
    );
    assert!(out.contains("docs/"), "a subdirectory is missing:\n{out}");
    assert!(out.contains("src/"), "a subdirectory is missing:\n{out}");
    assert_legible(&buf, "spawn");
}

#[test]
fn the_dangerous_row_follows_the_state_it_was_given() {
    let on = text(&render(80, 24, |f| {
        draw(f, FULL, &state(&["src"], 0, true))
    }));
    let off = text(&render(80, 24, |f| {
        draw(f, FULL, &state(&["src"], 0, false))
    }));

    assert!(
        on.contains("ON"),
        "spawn with dangerous set reads off:\n{on}"
    );
    assert!(
        off.contains("OFF"),
        "spawn without dangerous reads armed:\n{off}"
    );
}

#[test]
fn a_selected_dir_row_is_accented_on_the_selection_background() {
    let rect = Rect::new(0, 0, 40, 1);
    let sel = render(40, 1, |f| draw_dir_row(f, rect, "src", true));
    let plain = render(40, 1, |f| draw_dir_row(f, rect, "src", false));

    assert!(
        text(&sel).contains("src/"),
        "the selected row lost its name"
    );
    assert!(text(&plain).contains("src/"), "the plain row lost its name");

    assert_eq!(
        sel[(0u16, 0u16)].bg,
        theme::BG_ACTIVE,
        "the selected row has no selection background"
    );
    assert_eq!(
        plain[(0u16, 0u16)].bg,
        Color::Reset,
        "the unselected row is painted with a selection background"
    );

    let name_x = 2u16;
    assert_eq!(
        sel[(name_x, 0u16)].fg,
        theme::ACCENT_CYAN,
        "the selected name is not accented"
    );
    assert!(
        sel[(name_x, 0u16)].modifier.contains(Modifier::BOLD),
        "the selected name is not bold"
    );
    assert_eq!(
        plain[(name_x, 0u16)].fg,
        theme::FG,
        "the unselected name is not plain foreground"
    );
    assert!(
        !plain[(name_x, 0u16)].modifier.contains(Modifier::BOLD),
        "the unselected name is bold, so selection is invisible"
    );
}

#[test]
fn an_empty_folder_says_what_the_keys_still_do() {
    let spawn = state(&[], 0, false);
    let buf = render(60, 5, |f| draw_dir_list(f, Rect::new(0, 0, 60, 5), &spawn));
    let out = text(&buf);
    assert!(
        out.contains("(no subdirectories. Enter spawns here"),
        "an empty folder gives the user no way forward:\n{out}"
    );
}

#[test]
fn the_dir_list_scrolls_to_keep_the_selection_visible() {
    let names: Vec<String> = (0..20).map(|i| format!("d{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let spawn = state(&refs, 19, false);

    let buf = render(40, 5, |f| draw_dir_list(f, Rect::new(0, 0, 40, 5), &spawn));
    let out = text(&buf);

    assert!(
        out.contains("d19/"),
        "the selected folder scrolled off:\n{out}"
    );
    assert!(
        out.contains("d15/"),
        "the window is not five rows deep:\n{out}"
    );
    assert!(
        !out.contains("d14/"),
        "the list did not scroll, so the selection sits off-screen:\n{out}"
    );
}

#[test]
fn the_hint_row_names_every_key_it_offers() {
    let buf = render(100, 1, |f| draw_hint_row(f, Rect::new(0, 0, 100, 1)));
    let listed = chips(&buf);
    let out = text(&buf);

    for key in [
        keys::SPAWN_UP.label,
        keys::SPAWN_DESCEND.label,
        keys::SPAWN_ASCEND.label,
        keys::SPAWN_TOGGLE_DANGER.label,
        keys::SPAWN_PICK.label,
        keys::SPAWN_CANCEL.label,
    ] {
        assert!(
            listed.iter().any(|chip| chip == key),
            "the hint row has no chip for {key:?}; chips are {listed:?}"
        );
    }
    for label in ["select", "descend", "ascend", "danger", "pick", "cancel"] {
        assert!(out.contains(label), "the hint row lacks {label:?}:\n{out}");
    }
}

#[test]
fn the_hint_row_fits_a_narrow_popup() {
    let buf = render(30, 1, |f| draw_hint_row(f, Rect::new(0, 0, 30, 1)));
    assert_eq!(
        row(&buf, 0).chars().count(),
        30,
        "the hint row no longer matches the width it was given"
    );
}

#[test]
fn it_stays_inside_the_rect_it_is_handed() {
    let spawn = state(&["docs", "src"], 0, true);
    let area = Rect::new(5, 2, 90, 26);
    let buf = render(100, 30, |f| draw(f, area, &spawn));
    assert_inside(&buf, area, "the spawn picker");
}

#[test]
fn it_survives_a_terminal_smaller_than_the_popup() {
    let spawn = state(&["docs", "src"], 1, false);

    let small = try_render(20, 5, |f| draw(f, Rect::new(0, 0, 20, 5), &spawn))
        .unwrap_or_else(|e| panic!("the spawn picker dies in a 20x5 terminal: {e}"));
    assert!(
        text(&small).contains("Spawn"),
        "at 20x5 the popup drew nothing readable:\n{}",
        text(&small)
    );

    let tiny = try_render(1, 1, |f| draw(f, Rect::new(0, 0, 1, 1), &spawn))
        .unwrap_or_else(|e| panic!("the spawn picker dies in a 1x1 terminal: {e}"));
    assert!(
        painted_bounds(&tiny).is_some(),
        "at 1x1 the popup drew nothing at all"
    );
}
