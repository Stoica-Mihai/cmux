use super::*;
use crate::session::Session;
use std::sync::mpsc;

/// Detach must remove the tile *and* end the session. While it only
/// dropped the handle, a daemon-hosted session stayed alive and kept
/// showing up in every other client, the browser included.
#[test]
fn detaching_ends_the_session_on_the_daemon_too() {
    let (tx, rx) = mpsc::channel();
    let (sess, _slot) = Session::new_daemon(
        1,
        "s".into(),
        PathBuf::from("/tmp"),
        false,
        None,
        24,
        80,
        None,
        5,
        tx,
    );
    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    app.sessions.push(sess);
    app.focus = 0;

    app.detach_focused();

    assert!(app.sessions.is_empty(), "the tile should be gone");
    match rx.try_recv() {
        Ok(cmux_proto::Request::Detach {
            session_id,
            keep_session,
        }) => {
            assert_eq!(session_id, 5);
            assert!(!keep_session, "detach must end it, not park it");
        }
        Ok(other) => panic!("expected Detach, got {other:?}"),
        Err(_) => panic!("the daemon was never told; the session leaks"),
    }
}

/// The spawn browser reads on a worker thread, so the listing arrives through
/// `poll` rather than being ready at construction.
#[test]
fn the_spawn_browser_lists_a_folder_off_thread() {
    let dir = std::env::temp_dir().join("cmux-spawn-browser-test");
    let _ = std::fs::remove_dir_all(&dir);
    for name in ["alpha", "beta", ".hidden"] {
        std::fs::create_dir_all(dir.join(name)).unwrap();
    }
    std::fs::write(dir.join("a-file"), b"x").unwrap();

    let mut s = SpawnState::new(dir.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while s.entries.is_empty() && std::time::Instant::now() < deadline {
        s.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let names: Vec<String> = s
        .entries
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        names,
        vec!["alpha", "beta"],
        "dotfiles and files must be out"
    );
    assert!(
        !s.reading,
        "still marked as reading after the listing landed"
    );
}

/// Stepping up puts the cursor on the folder just left, once its listing
/// arrives.
#[test]
fn stepping_up_selects_the_folder_just_left() {
    let root = std::env::temp_dir().join("cmux-spawn-ascend-test");
    let _ = std::fs::remove_dir_all(&root);
    for name in ["aaa", "mmm", "zzz"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }

    let mut s = SpawnState::new(root.join("mmm"));
    let settle = |s: &mut SpawnState| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while s.reading && std::time::Instant::now() < deadline {
            s.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    settle(&mut s);
    s.ascend();
    settle(&mut s);

    let picked = s.pick();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(picked.file_name().unwrap(), "mmm", "cursor lost the folder");
}

fn daemon_session(id: u64, label: &str) -> Session {
    let (tx, _rx) = mpsc::channel();
    Session::new_daemon(
        id,
        label.into(),
        PathBuf::from("/tmp"),
        false,
        None,
        24,
        80,
        None,
        id,
        tx,
    )
    .0
}

fn app_with(n: u64) -> App {
    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    for i in 1..=n {
        app.sessions.push(daemon_session(i, &format!("s{i}")));
    }
    app
}

/// A scratch tree of directories, named so the caller can predict the sort.
fn dir_tree(tag: &str, names: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cmux-app-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for name in names {
        std::fs::create_dir_all(root.join(name)).expect("mkdir");
    }
    root
}

/// Block until the browser thread's listing lands, since `new` and `descend`
/// only queue the read.
fn settle(s: &mut SpawnState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while s.reading && std::time::Instant::now() < deadline {
        s.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn cycling_focus_wraps_in_both_directions() {
    let mut app = app_with(3);
    app.focus = 0;
    app.cycle_focus(-1);
    assert_eq!(
        app.focus, 2,
        "going back from the first should wrap to last"
    );
    app.cycle_focus(1);
    assert_eq!(app.focus, 0, "going on from the last should wrap to first");
}

#[test]
fn cycling_focus_with_no_sessions_stays_put() {
    let mut app = app_with(0);
    app.cycle_focus(1);
    assert_eq!(app.focus, 0, "an empty list has nothing to focus");
}

#[test]
fn detaching_the_last_tile_moves_focus_back_and_leaves_no_gap() {
    let mut app = app_with(3);
    app.focus = 2;
    app.detach_focused();
    assert_eq!(app.sessions.len(), 2);
    assert_eq!(app.focus, 1, "focus should follow the list in, not dangle");
}

#[test]
fn detaching_the_only_tile_returns_to_the_dashboard() {
    let mut app = app_with(1);
    app.focus = 0;
    app.mode = Mode::Scrollback(1);
    app.detach_focused();
    assert!(app.sessions.is_empty());
    assert!(
        matches!(app.mode, Mode::Dashboard),
        "with nothing left there is no session mode to stay in"
    );
}

#[test]
fn a_new_tile_never_gets_a_degenerate_size() {
    // Small enough that the naive arithmetic would underflow to zero.
    let mut app = App::new(PathBuf::from("/tmp"), (4, 12));
    for n in 0..8 {
        let (rows, cols) = app.tile_size_for_new();
        assert!(rows >= 4, "tile {n} got {rows} rows");
        assert!(cols >= 10, "tile {n} got {cols} cols");
        app.sessions.push(daemon_session(n + 1, "s"));
    }
}

#[test]
fn tiles_shrink_as_the_grid_fills() {
    let mut app = App::new(PathBuf::from("/tmp"), (60, 200));
    let one = app.tile_size_for_new();
    for i in 1..=4 {
        app.sessions.push(daemon_session(i, "s"));
    }
    let five = app.tile_size_for_new();
    assert!(
        five.0 < one.0 && five.1 < one.1,
        "a fifth tile should be smaller than the first: {one:?} then {five:?}"
    );
}

#[test]
fn the_directory_picker_lists_only_visible_directories() {
    let root = dir_tree("visible", &["alpha", "beta", ".hidden"]);
    std::fs::write(root.join("a-file"), b"x").expect("write");
    let mut state = SpawnState::new(root.clone());
    settle(&mut state);
    let names: Vec<String> = state
        .entries
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["alpha", "beta"], "got {names:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn descending_then_ascending_lands_back_on_the_directory_left() {
    let root = dir_tree("updown", &["alpha", "beta", "gamma"]);
    let mut state = SpawnState::new(root.clone());
    settle(&mut state);
    state.move_sel(1);
    let target = state.pick();
    state.descend();
    settle(&mut state);
    assert_eq!(state.cwd, target, "descend should enter the selection");
    state.ascend();
    settle(&mut state);
    assert_eq!(state.cwd, root);
    assert_eq!(
        state.pick(),
        target,
        "ascending should reselect the directory just left"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn picking_in_an_empty_directory_falls_back_to_that_directory() {
    let root = dir_tree("empty", &[]);
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut state = SpawnState::new(root.clone());
    settle(&mut state);
    assert!(state.entries.is_empty());
    assert_eq!(state.pick(), root, "with nothing listed, pick the cwd");
    let _ = std::fs::remove_dir_all(&root);
}

fn transcript(cwd: &str, title: Option<&str>, id: &str) -> crate::transcripts::Transcript {
    crate::transcripts::Transcript {
        session_id: id.to_string(),
        path: PathBuf::from(cwd).join(format!("{id}.jsonl")),
        cwd: PathBuf::from(cwd),
        forked_from: None,
        mtime: std::time::SystemTime::UNIX_EPOCH,
        file_size: 0,
        custom_title: title.map(str::to_string),
    }
}

fn picker_with(items: Vec<crate::transcripts::Transcript>) -> PickerState {
    let n = items.len();
    let mut p = PickerState::new();
    p.all = items;
    p.items = (0..n).collect();
    p.selected = 0;
    p.dangerous = false;
    p.filter = String::new();
    p.previews.clear();
    p.scanning = false;
    p
}

#[test]
fn the_picker_filter_matches_path_and_title_case_insensitively() {
    let mut p = picker_with(vec![
        transcript("/home/u/Alpha", None, "a"),
        transcript("/home/u/beta", Some("Gamma work"), "b"),
        transcript("/home/u/delta", None, "c"),
    ]);

    p.filter = "ALPHA".into();
    p.apply_filter();
    assert_eq!(p.items, vec![0], "path match should ignore case");

    p.filter = "gamma".into();
    p.apply_filter();
    assert_eq!(p.items, vec![1], "the custom title should match too");

    p.filter = "nothing-here".into();
    p.apply_filter();
    assert!(p.items.is_empty());
}

#[test]
fn clearing_the_picker_filter_restores_every_item() {
    let mut p = picker_with(vec![
        transcript("/a", None, "a"),
        transcript("/b", None, "b"),
    ]);
    p.filter = "a".into();
    p.apply_filter();
    assert_eq!(p.items, vec![0]);
    p.filter.clear();
    p.apply_filter();
    assert_eq!(p.items, vec![0, 1]);
}

/// Filtering down to fewer items than the current selection index used to
/// leave `selected` pointing past the end.
#[test]
fn filtering_pulls_the_selection_back_into_range() {
    let mut p = picker_with(vec![
        transcript("/a", None, "a"),
        transcript("/b", None, "b"),
        transcript("/c", None, "c"),
    ]);
    p.selected = 2;
    p.filter = "/a".into();
    p.apply_filter();
    assert!(
        p.selected < p.items.len().max(1),
        "selected {} is past the {} remaining items",
        p.selected,
        p.items.len()
    );
    assert!(p.current().is_some(), "the selection should resolve");
}

#[test]
fn an_empty_picker_has_nothing_current_and_survives_moving() {
    let mut p = picker_with(Vec::new());
    assert!(p.current().is_none());
    p.move_sel(1);
    p.move_sel(-1);
    assert!(p.current().is_none());
}
