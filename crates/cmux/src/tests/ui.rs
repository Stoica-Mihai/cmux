use super::popups::harness::{app_with, render, text, try_render};
use super::*;
use crate::app::{PickerState, RenameState, SpawnState};
use std::path::PathBuf;

/// A mode to render plus the label a failure should name it by.
type ModeCase = (&'static str, fn() -> Mode);

const POPUP_TITLES: [&str; 5] = [
    "cmux keys",
    "Detach session?",
    "Rename session",
    "Spawn claude in a folder",
    "Resume past session",
];

fn rename() -> Mode {
    Mode::Rename(RenameState {
        session_id: 1,
        buf: "x".to_string(),
    })
}

fn spawn() -> Mode {
    Mode::Spawn(SpawnState::new(PathBuf::from("/tmp/project")))
}

fn picker() -> Mode {
    Mode::Picker(Box::new(PickerState::new()))
}

#[test]
fn an_empty_dashboard_says_how_to_start_one() {
    let mut app = app_with(&[]);
    let out = text(&render(80, 24, |f| draw(f, &mut app, &mut Vec::new())));

    assert!(out.contains("cmux"), "no titlebar brand:\n{out}");
    assert!(out.contains("0/0"), "no session counter:\n{out}");
    assert!(
        out.contains("(empty)"),
        "the sidebar is not marked empty:\n{out}"
    );
    assert!(
        out.contains("No sessions yet."),
        "no empty-state body:\n{out}"
    );
    assert!(
        out.contains(crate::keys::PREFIX.label),
        "the footer never says how to reach the chords:\n{out}"
    );
}

#[test]
fn every_session_gets_a_sidebar_row() {
    for labels in [&["alpha"][..], &["alpha", "beta", "gamma"][..]] {
        let mut app = app_with(labels);
        let out = text(&render(100, 30, |f| draw(f, &mut app, &mut Vec::new())));

        assert!(
            out.contains(&format!("1/{}", labels.len())),
            "the counter does not match {} sessions:\n{out}",
            labels.len()
        );
        for (i, label) in labels.iter().enumerate() {
            assert!(
                out.contains(&format!(" {} ", i + 1)),
                "session {} has no sidebar index:\n{out}",
                i + 1
            );
            assert!(out.contains(label), "session {label:?} has no row:\n{out}");
        }
    }
}

#[test]
fn each_mode_draws_its_own_popup_and_no_other() {
    let cases: Vec<ModeCase> = vec![
        ("cmux keys", || Mode::Help),
        ("Detach session?", || Mode::ConfirmDetach(1)),
        ("Rename session", rename),
        ("Spawn claude in a folder", spawn),
        ("Resume past session", picker),
    ];

    for (title, make) in cases {
        let mut app = app_with(&["alpha"]);
        app.mode = make();
        let out = text(&render(100, 30, |f| draw(f, &mut app, &mut Vec::new())));

        assert!(
            out.contains(title),
            "the mode drew no {title:?} popup:\n{out}"
        );
        for other in POPUP_TITLES.iter().filter(|t| **t != title) {
            assert!(
                !out.contains(other),
                "{title:?} also drew {other:?}:\n{out}"
            );
        }
    }
}

#[test]
fn the_bare_modes_draw_no_popup_at_all() {
    let cases: Vec<ModeCase> = vec![
        ("dashboard", || Mode::Dashboard),
        ("reorder", || Mode::Reorder),
        ("scrollback", || Mode::Scrollback(1)),
    ];

    for (name, make) in cases {
        let mut app = app_with(&["alpha"]);
        app.mode = make();
        let out = text(&render(100, 30, |f| draw(f, &mut app, &mut Vec::new())));

        for title in POPUP_TITLES {
            assert!(
                !out.contains(title),
                "{name} drew the {title:?} popup:\n{out}"
            );
        }
    }
}

#[test]
fn a_lost_daemon_covers_whatever_mode_is_open() {
    for make in [(|| Mode::Dashboard) as fn() -> Mode, || Mode::Help] {
        let mut app = app_with(&["alpha"]);
        app.mode = make();
        app.daemon_lost = true;
        let out = text(&render(100, 30, |f| draw(f, &mut app, &mut Vec::new())));
        assert!(
            out.contains("Daemon disconnected"),
            "the daemon-lost modal did not cover this mode:\n{out}"
        );
    }
}

#[test]
fn the_focused_tile_is_the_only_one_reported_back() {
    let mut app = app_with(&["alpha", "beta"]);
    app.focus = 1;
    let mut sizes: TileSizes = vec![(99, 1, 1)];

    let _ = render(100, 30, |f| draw(f, &mut app, &mut sizes));

    assert_eq!(
        sizes.len(),
        1,
        "stale tile sizes survived the redraw: {sizes:?}"
    );
    assert_eq!(sizes[0].0, 1, "the reported tile is not the focused one");
    assert!(
        sizes[0].1 > 0 && sizes[0].2 > 0,
        "the tile has no size: {sizes:?}"
    );
    assert!(
        app.last_tile_area.is_some(),
        "the toast has no tile to anchor to"
    );
}

#[test]
fn the_dashboard_survives_a_terminal_smaller_than_its_chrome() {
    for labels in [&[][..], &["alpha", "beta", "gamma"][..]] {
        for (w, h) in [(20u16, 5u16), (1, 1)] {
            let mut app = app_with(labels);
            try_render(w, h, |f| draw(f, &mut app, &mut Vec::new())).unwrap_or_else(|e| {
                panic!(
                    "the dashboard dies at {w}x{h} with {} sessions: {e}",
                    labels.len()
                )
            });
        }
    }
}

#[test]
fn every_mode_survives_a_terminal_smaller_than_its_popup() {
    let mut dead: Vec<String> = Vec::new();

    for (w, h) in [(20u16, 5u16), (1, 1)] {
        let cases: Vec<ModeCase> = vec![
            ("dashboard", || Mode::Dashboard),
            ("help", || Mode::Help),
            ("confirm detach", || Mode::ConfirmDetach(1)),
            ("rename", rename),
            ("spawn", spawn),
            ("picker", picker),
            ("reorder", || Mode::Reorder),
            ("scrollback", || Mode::Scrollback(1)),
        ];
        for (name, make) in cases {
            let mut app = app_with(&["alpha"]);
            app.mode = make();
            if let Err(e) = try_render(w, h, |f| draw(f, &mut app, &mut Vec::new())) {
                dead.push(format!("{name} at {w}x{h}: {e}"));
            }
        }

        let mut app = app_with(&["alpha"]);
        app.daemon_lost = true;
        if let Err(e) = try_render(w, h, |f| draw(f, &mut app, &mut Vec::new())) {
            dead.push(format!("daemon-lost overlay at {w}x{h}: {e}"));
        }
    }

    assert!(
        dead.is_empty(),
        "these modes kill cmux in a small terminal:\n{}",
        dead.join("\n")
    );
}
