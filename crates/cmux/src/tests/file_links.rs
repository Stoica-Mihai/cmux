use super::*;

/// A screen from lines of text, none of them wrapped.
fn screen(lines: &[&str]) -> Vec<(String, bool)> {
    lines.iter().map(|l| (l.to_string(), false)).collect()
}

/// A directory holding the files a test needs to find, so the existence check
/// has something real to resolve against.
struct Fixture {
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str, files: &[&str]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("cmux-file-links-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture");
        for f in files {
            let path = dir.join(f);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create a parent");
            }
            std::fs::write(&path, b"x").expect("write a fixture file");
        }
        Self { dir }
    }

    fn detect(&self, lines: &[&str]) -> FileLinks {
        let mut cache = Cache::default();
        super::detect(&screen(lines), &self.dir, &mut cache, 0)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// What gets linked
// ---------------------------------------------------------------------------

#[test]
fn a_relative_path_that_exists_is_linked() {
    let fx = Fixture::new("rel", &["notes.txt"]);
    let links = fx.detect(&["I read notes.txt for you"]);

    // "I read notes.txt" - the name starts at column 7 and runs 9 cells.
    assert_eq!(links.cells(), 9, "every cell of the name carries the link");
    let uri = links.uri_at(0, 7).expect("the first cell is linked");
    assert!(uri.ends_with("/notes.txt"), "{uri}");
    assert_eq!(links.uri_at(0, 15), Some(uri), "and so is the last");
    assert_eq!(links.uri_at(0, 6), None, "the space before it is not");
    assert_eq!(links.uri_at(0, 16), None, "nor the space after");
}

#[test]
fn a_nested_relative_path_is_linked() {
    let fx = Fixture::new("nested", &["src/main.rs"]);
    let links = fx.detect(&["edited src/main.rs"]);
    let uri = links.uri_at(0, 7).expect("linked");
    assert!(uri.ends_with("/src/main.rs"), "{uri}");
    assert_eq!(links.cells(), "src/main.rs".len());
}

#[test]
fn a_path_that_does_not_exist_is_not_linked() {
    let fx = Fixture::new("missing", &["notes.txt"]);
    let links = fx.detect(&["see missing.txt and gone/absent.rs"]);
    assert!(links.is_empty(), "nothing on that line exists");
}

/// A bare word is not a candidate. Half the words in a sentence name a
/// directory somewhere, and linking them all would light up the screen.
#[test]
fn bare_words_are_not_linked_even_when_they_name_a_directory() {
    let fx = Fixture::new("bare", &["src/main.rs"]);
    let links = fx.detect(&["the src directory holds it"]);
    assert!(links.is_empty(), "'src' exists but is not path-shaped");
}

#[test]
fn a_line_reference_is_dropped_from_the_target() {
    let fx = Fixture::new("lineref", &["src/main.rs"]);
    for line in ["src/main.rs:426", "src/main.rs:426:12"] {
        let links = fx.detect(&[line]);
        let uri = links.uri_at(0, 0).unwrap_or_else(|| panic!("{line}"));
        assert!(uri.ends_with("/src/main.rs"), "{line} -> {uri}");
        assert_eq!(
            links.cells(),
            "src/main.rs".len(),
            "{line}: only the path is covered, not the line number"
        );
    }
}

#[test]
fn surrounding_punctuation_is_not_part_of_the_link() {
    let fx = Fixture::new("punct", &["notes.txt"]);
    for line in ["(notes.txt)", "`notes.txt`", "[notes.txt]", "notes.txt."] {
        let links = fx.detect(&[line]);
        assert_eq!(
            links.cells(),
            "notes.txt".len(),
            "{line}: the punctuation was swept in"
        );
    }
}

/// claude draws borders and result markers hard against its text, so those
/// glyphs must not glue themselves onto a path and stop it resolving.
#[test]
fn the_glyphs_claude_draws_with_do_not_join_a_path() {
    let fx = Fixture::new("glyphs", &["notes.txt"]);
    for line in [
        "\u{2502}notes.txt",
        "\u{23BF}  notes.txt",
        "\u{2022} notes.txt",
        "\u{2192}notes.txt",
    ] {
        let links = fx.detect(&[line]);
        assert_eq!(
            links.cells(),
            "notes.txt".len(),
            "{line:?}: the glyph was taken as part of the name"
        );
    }
}

#[test]
fn an_absolute_path_is_linked() {
    let fx = Fixture::new("abs", &["notes.txt"]);
    let target = fx.dir.join("notes.txt");
    let line = format!("wrote {}", target.display());
    let links = fx.detect(&[&line]);
    let uri = links.uri_at(0, 6).expect("linked");
    assert!(uri.ends_with("/notes.txt"), "{uri}");
}

/// claude prints URLs already wrapped in OSC 8, so a URL must not also collect
/// a synthesised target.
#[test]
fn a_url_is_left_alone() {
    let fx = Fixture::new("url", &["notes.txt"]);
    let links = fx.detect(&["see https://example.com/notes.txt"]);
    assert!(links.is_empty());
}

#[test]
fn a_path_split_across_a_wrap_is_linked_on_both_rows() {
    let fx = Fixture::new("wrap", &["deep/nested/dir/file.txt"]);
    // The name breaks after "deep/nest", as a narrow tile would wrap it.
    let rows = vec![
        ("open deep/nest".to_string(), true),
        ("ed/dir/file.txt".to_string(), false),
    ];
    let mut cache = Cache::default();
    let links = super::detect(&rows, &fx.dir, &mut cache, 0);

    let uri = links.uri_at(0, 5).expect("the first row carries the link");
    assert!(uri.ends_with("/deep/nested/dir/file.txt"), "{uri}");
    assert_eq!(
        links.uri_at(1, 0),
        Some(uri),
        "the continuation carries the same target"
    );
    assert_eq!(links.cells(), "deep/nested/dir/file.txt".len());
}

#[test]
fn two_occurrences_of_one_path_share_a_target() {
    let fx = Fixture::new("twice", &["notes.txt"]);
    let links = fx.detect(&["notes.txt", "notes.txt"]);
    assert_eq!(links.uri_at(0, 0), links.uri_at(1, 0));
    let (first, _) = links.get(0, 0).expect("linked");
    let (second, _) = links.get(1, 0).expect("linked");
    assert_eq!(first, second, "one id, so a terminal highlights both");
}

#[test]
fn an_empty_screen_yields_nothing() {
    let fx = Fixture::new("empty", &[]);
    assert!(fx.detect(&[]).is_empty());
    assert!(fx.detect(&["", "   "]).is_empty());
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

#[test]
fn a_repeated_sweep_answers_the_same() {
    let fx = Fixture::new("cache", &["notes.txt"]);
    let mut cache = Cache::default();
    let rows = screen(&["notes.txt"]);
    let first = super::detect(&rows, &fx.dir, &mut cache, 0);
    let second = super::detect(&rows, &fx.dir, &mut cache, 1);
    assert_eq!(first, second);
}

/// A file created while a session is open becomes clickable, so the cache must
/// not hold a stale "does not exist" for ever.
#[test]
fn the_cache_expires_so_a_new_file_is_picked_up() {
    let fx = Fixture::new("ttl", &[]);
    let mut cache = Cache::default();
    let rows = screen(&["notes.txt"]);
    assert!(super::detect(&rows, &fx.dir, &mut cache, 0).is_empty());

    std::fs::write(fx.dir.join("notes.txt"), b"x").expect("create it now");
    assert!(
        !super::detect(&rows, &fx.dir, &mut cache, CACHE_TTL_MS + 1).is_empty(),
        "the new file was never picked up"
    );
}

// ---------------------------------------------------------------------------
// The target a terminal receives
// ---------------------------------------------------------------------------

#[test]
fn a_space_in_a_path_is_encoded() {
    let encoded = encode("/home/mcs/my notes.txt");
    assert_eq!(encoded, "/home/mcs/my%20notes.txt");
}

/// Windows has no `/home`, so a Linux path has to go through the
/// distribution's UNC share to be openable from the host at all.
#[test]
fn a_linux_path_goes_through_the_distribution_share_under_wsl() {
    assert_eq!(
        uri_under(Path::new("/home/mcs/notes.txt"), Some("archlinux")),
        "file://wsl.localhost/archlinux/home/mcs/notes.txt"
    );
}

#[test]
fn a_drive_mount_skips_the_share_and_uses_its_drive() {
    assert_eq!(
        uri_under(Path::new("/mnt/c/Users/mcs/notes.txt"), Some("archlinux")),
        "file:///C:/Users/mcs/notes.txt"
    );
}

#[test]
fn off_wsl_the_path_is_the_url_path() {
    assert_eq!(
        uri_under(Path::new("/home/mcs/notes.txt"), None),
        "file:///home/mcs/notes.txt"
    );
}

#[test]
fn a_drive_mount_maps_to_its_drive_letter() {
    assert_eq!(
        drive_mount("/mnt/c/Users/mcs"),
        Some(('C', "/Users/mcs".to_string()))
    );
    assert_eq!(drive_mount("/mnt/c"), Some(('C', "/".to_string())));
    assert_eq!(drive_mount("/mnt/wsl/x"), None, "not a single letter");
    assert_eq!(drive_mount("/home/mcs"), None);
}
