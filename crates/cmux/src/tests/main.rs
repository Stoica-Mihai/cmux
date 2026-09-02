use super::*;

fn saved(label: &str, manually_renamed: bool) -> persist::PersistedSession {
    persist::PersistedSession {
        cwd: PathBuf::from("/tmp"),
        label: label.to_string(),
        dangerous: false,
        resume_id: None,
        manually_renamed,
    }
}

/// Both directions. A name the user typed is pinned, so the probe cannot
/// undo it. A label merely carried over from the last run is not, or the
/// probe can never rename the session and the TUI ends up showing a
/// different name from the browser.
#[test]
fn only_a_user_chosen_name_is_pinned_on_the_daemon() {
    assert!(should_pin_label(&saved("mine", true)));
    assert!(!should_pin_label(&saved("saved-dirname", false)));
    assert!(!should_pin_label(&saved("", true)));
    assert!(!should_pin_label(&saved("", false)));
}

/// A drag that leaves the tile selects up to the edge it crossed. It used to
/// stop updating, freezing the selection mid-drag.
#[test]
fn a_pointer_past_an_edge_clamps_into_the_tile() {
    let tile = ratatui::layout::Rect {
        x: 10,
        y: 5,
        width: 20,
        height: 8,
    };
    assert_eq!(tile_cell(5, 10, tile), Some((0, 0)), "top-left corner");
    assert_eq!(
        tile_cell(12, 29, tile),
        Some((7, 19)),
        "bottom-right corner"
    );
    assert_eq!(tile_cell(0, 0, tile), Some((0, 0)), "above and left of it");
    assert_eq!(tile_cell(200, 200, tile), Some((7, 19)), "below and right");
    assert_eq!(tile_cell(9, 3, tile), Some((4, 0)), "left of the tile");
}

#[test]
fn a_tile_with_no_area_yields_no_cell() {
    let flat = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 4,
    };
    assert_eq!(tile_cell(2, 2, flat), None);
}
