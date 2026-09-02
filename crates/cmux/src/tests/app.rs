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
