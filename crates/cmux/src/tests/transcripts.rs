use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn write_transcript(name: &str, lines: &[String]) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cmux-transcript-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

fn filler(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!(r#"{{"type":"user","message":{{"role":"user","content":"turn {i}"}}}}"#))
        .collect()
}

fn title_record(title: &str) -> String {
    format!(r#"{{"type":"custom-title","customTitle":"{title}"}}"#)
}

#[test]
fn a_title_recorded_late_in_the_transcript_is_still_found() {
    let mut lines = vec![r#"{"type":"user","cwd":"/home/mcs"}"#.to_string()];
    lines.extend(filler(60));
    lines.push(title_record("gbsp-813"));
    let path = write_transcript("late-title", &lines);

    let head = extract_header(&path);
    assert_eq!(head.cwd, Some(PathBuf::from("/home/mcs")));
    assert_eq!(head.custom_title.as_deref(), Some("gbsp-813"));
}

#[test]
fn the_newest_title_wins_after_a_rename() {
    let mut lines = vec![r#"{"type":"user","cwd":"/home/mcs"}"#.to_string()];
    lines.push(title_record("old-name"));
    lines.extend(filler(80));
    lines.push(title_record("gbsp-813"));
    let path = write_transcript("renamed", &lines);

    assert_eq!(
        extract_header(&path).custom_title.as_deref(),
        Some("gbsp-813")
    );
}

#[test]
fn a_title_in_the_first_lines_is_still_found() {
    let lines = vec![
        r#"{"type":"user","cwd":"/home/mcs"}"#.to_string(),
        title_record("gbsp-147"),
    ];
    let path = write_transcript("early-title", &lines);

    assert_eq!(
        extract_header(&path).custom_title.as_deref(),
        Some("gbsp-147")
    );
}

#[test]
fn a_forked_transcript_names_the_session_it_came_from() {
    let lines = vec![
        r#"{"type":"user","cwd":"/home/mcs/Documents/he-https","forkedFrom":{"sessionId":"e1896a80-d12f-4174-bdb3-f74ae67f14fb","messageUuid":"9450eca2"}}"#.to_string(),
        title_record("he-https-claro"),
    ];
    let path = write_transcript("forked", &lines);

    let head = extract_header(&path);
    assert_eq!(
        head.forked_from.as_deref(),
        Some("e1896a80-d12f-4174-bdb3-f74ae67f14fb")
    );
    assert_eq!(head.custom_title.as_deref(), Some("he-https-claro"));
}

#[test]
fn a_transcript_with_no_origin_is_not_a_fork() {
    let lines = vec![r#"{"type":"user","cwd":"/home/mcs"}"#.to_string()];
    let path = write_transcript("unforked", &lines);
    assert_eq!(extract_header(&path).forked_from, None);
}

#[test]
fn an_untitled_transcript_reports_no_title() {
    let mut lines = vec![r#"{"type":"user","cwd":"/home/mcs"}"#.to_string()];
    lines.extend(filler(50));
    let path = write_transcript("untitled", &lines);

    let head = extract_header(&path);
    assert_eq!(head.cwd, Some(PathBuf::from("/home/mcs")));
    assert_eq!(head.custom_title, None);
}

static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

/// A temp directory that deletes itself, so a panicking test leaves nothing.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("cmux-transcripts-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent");
    }
    std::fs::write(path, lines.join("\n")).expect("write fixture");
}

fn set_mtime(path: &Path, secs_since_epoch: u64) {
    let f = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open fixture");
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs_since_epoch))
        .expect("set mtime");
}

fn json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).expect("fixture json")
}

fn ago(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

#[test]
fn slug_decode_restores_a_dash_free_path() {
    for (slug, want) in [
        ("-home-u-proj", "/home/u/proj"),
        ("-home", "/home"),
        ("-", "/"),
    ] {
        assert_eq!(
            slug_decode(&Path::new("/any/projects").join(slug)),
            PathBuf::from(want),
            "{slug} should decode to {want}"
        );
    }
}

/// The encoding maps every separator to a dash and escapes nothing, so a
/// directory whose own name holds a dash cannot be recovered from its slug.
/// Known and load-bearing: `scan` falls back to this only when the transcript
/// records no cwd.
#[test]
fn slug_decode_cannot_tell_a_dash_from_a_separator() {
    assert_eq!(
        slug_decode(&Path::new("/any/projects/-home-u-my-project")),
        PathBuf::from("/home/u/my/project"),
        "the encoding is lossy, so a dashed path decodes to the wrong directory"
    );
}

#[test]
fn slug_decode_prepends_the_leading_separator() {
    assert_eq!(
        slug_decode(Path::new("/any/projects/home-u")),
        PathBuf::from("/home/u"),
        "a directory name with no leading dash should still decode to an absolute path"
    );
    assert_eq!(
        slug_decode(Path::new("")),
        PathBuf::from("/"),
        "an empty path has no file name and should decode to the root"
    );
}

#[test]
fn extract_header_reads_the_cwd_and_the_custom_title() {
    let dir = TempDir::new("header-ok");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"type":"summary","cwd":"/home/u/proj"}"#,
            r#"{"type":"custom-title","customTitle":"my run"}"#,
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
        ],
    );

    let head = extract_header(&path);
    assert_eq!(
        head.cwd,
        Some(PathBuf::from("/home/u/proj")),
        "the recorded cwd was not read back"
    );
    assert_eq!(
        head.custom_title,
        Some("my run".to_string()),
        "the custom title was not read back"
    );
}

#[test]
fn extract_header_finds_a_header_that_is_not_on_the_first_line() {
    let dir = TempDir::new("header-late");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":"yo"}}"#,
            r#"{"cwd":"/home/u/late"}"#,
        ],
    );

    assert_eq!(
        extract_header(&path).cwd,
        Some(PathBuf::from("/home/u/late")),
        "a cwd below the first line should still be found"
    );
}

#[test]
fn extract_header_skips_a_malformed_line_and_keeps_reading() {
    let dir = TempDir::new("header-bad");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            "{ this line is not json",
            r#"{"cwd":"/home/u/proj"}"#,
            r#"{"type":"custom-title","customTitle":"still found"}"#,
        ],
    );

    let head = extract_header(&path);
    assert_eq!(
        head.cwd,
        Some(PathBuf::from("/home/u/proj")),
        "a malformed line must not stop the scan before the cwd"
    );
    assert_eq!(
        head.custom_title,
        Some("still found".to_string()),
        "a malformed line must not stop the scan before the title"
    );
}

#[test]
fn extract_header_keeps_the_first_cwd_it_sees() {
    let dir = TempDir::new("header-first");
    let path = dir.join("a.jsonl");
    write_jsonl(&path, &[r#"{"cwd":"/first"}"#, r#"{"cwd":"/second"}"#]);

    assert_eq!(
        extract_header(&path).cwd,
        Some(PathBuf::from("/first")),
        "a later record must not overwrite the cwd already found"
    );
}

#[test]
fn extract_header_ignores_empty_cwd_and_title_values() {
    let dir = TempDir::new("header-blank");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"cwd":""}"#,
            r#"{"type":"custom-title","customTitle":""}"#,
        ],
    );

    let head = extract_header(&path);
    assert_eq!(head.cwd, None, "an empty cwd string is not a usable cwd");
    assert_eq!(
        head.custom_title, None,
        "an empty custom title is not a usable title"
    );
}

/// The cwd is read from the first `HEAD_LINES` records only, so both sides of
/// that boundary have to behave. A title, unlike a cwd, is also read from the
/// file's tail and so is not bounded this way.
#[test]
fn extract_header_reads_the_fortieth_line_but_not_the_forty_first() {
    let dir = TempDir::new("header-window");
    let mut at_forty: Vec<&str> = vec![r#"{"type":"user"}"#; 39];
    at_forty.push(r#"{"cwd":"/home/u/edge"}"#);
    let inside = dir.join("inside.jsonl");
    write_jsonl(&inside, &at_forty);

    let mut past_forty: Vec<&str> = vec![r#"{"type":"user"}"#; 40];
    past_forty.push(r#"{"cwd":"/home/u/edge"}"#);
    let outside = dir.join("outside.jsonl");
    write_jsonl(&outside, &past_forty);

    assert_eq!(
        extract_header(&inside).cwd,
        Some(PathBuf::from("/home/u/edge")),
        "line 40 is inside the scan window and should be read"
    );
    assert_eq!(
        extract_header(&outside).cwd,
        None,
        "line 41 is past the scan window and should not be read"
    );
}

#[test]
fn extract_header_returns_nothing_for_an_empty_file() {
    let dir = TempDir::new("header-empty");
    let path = dir.join("a.jsonl");
    std::fs::write(&path, b"").expect("write fixture");

    let head = extract_header(&path);
    assert_eq!(head.cwd, None, "an empty transcript has no cwd to read");
    assert_eq!(
        head.custom_title, None,
        "an empty transcript has no title to read"
    );
    assert_eq!(head.forked_from, None);
}

#[test]
fn extract_header_returns_nothing_for_a_missing_file() {
    let dir = TempDir::new("header-gone");
    let head = extract_header(&dir.join("nope.jsonl"));
    assert_eq!(head.cwd, None, "a missing transcript should read as no cwd");
    assert_eq!(head.custom_title, None);
    assert_eq!(head.forked_from, None);
}

#[test]
fn load_preview_renders_the_role_and_the_text() {
    let dir = TempDir::new("prev-ok");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[r#"{"type":"user","message":{"role":"user","content":"hello"}}"#],
    );

    assert_eq!(
        load_preview(&path, 10),
        "[user]\n  hello\n",
        "a single message should render as its role then its indented text"
    );
}

#[test]
fn load_preview_falls_back_to_the_message_role_when_there_is_no_type() {
    let dir = TempDir::new("prev-role");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"message":{"role":"assistant","content":"hi"}}"#,
            r#"{"message":{"content":"anon"}}"#,
        ],
    );

    let out = load_preview(&path, 10);
    assert!(
        out.contains("[assistant]"),
        "the message role should label a record with no type, got {out:?}"
    );
    assert!(
        out.contains("[?]"),
        "a record with neither type nor role should render as [?], got {out:?}"
    );
}

#[test]
fn load_preview_skips_a_malformed_line_and_keeps_the_rest() {
    let dir = TempDir::new("prev-bad");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"first"}}"#,
            "not json at all",
            r#"{"type":"user","message":{"role":"user","content":"third"}}"#,
        ],
    );

    let out = load_preview(&path, 20);
    assert!(
        out.contains("first"),
        "the message before the malformed line was lost, got {out:?}"
    );
    assert!(
        out.contains("third"),
        "the message after the malformed line was lost, got {out:?}"
    );
    assert!(
        !out.contains("not json"),
        "the malformed line should not reach the preview, got {out:?}"
    );
}

#[test]
fn load_preview_keeps_only_the_last_max_lines() {
    let dir = TempDir::new("prev-tail");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"m1"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"m2"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"m3"}}"#,
        ],
    );

    let out = load_preview(&path, 4);
    assert_eq!(
        out, "\n[user]\n  m3\n",
        "only the last 4 rendered lines should survive the cap"
    );
    assert!(
        !out.contains("m1"),
        "the oldest message should have been trimmed, got {out:?}"
    );
}

#[test]
fn load_preview_caps_one_message_at_eight_text_lines() {
    let dir = TempDir::new("prev-lines");
    let path = dir.join("a.jsonl");
    let body: String = (1..=10)
        .map(|i| format!("L{i}"))
        .collect::<Vec<_>>()
        .join("\\n");
    let record = format!(r#"{{"type":"user","message":{{"content":"{body}"}}}}"#);
    write_jsonl(&path, &[record.as_str()]);

    let out = load_preview(&path, 50);
    assert!(
        out.contains("  L8"),
        "the eighth text line should be kept, got {out:?}"
    );
    assert!(
        !out.contains("  L9"),
        "the ninth text line is past the per-message cap, got {out:?}"
    );
}

#[test]
fn load_preview_reports_a_missing_file() {
    let dir = TempDir::new("prev-gone");
    assert_eq!(
        load_preview(&dir.join("nope.jsonl"), 10),
        "(unable to read transcript)",
        "a missing transcript should report itself rather than panicking"
    );
}

#[test]
fn load_preview_reports_an_empty_file() {
    let dir = TempDir::new("prev-empty");
    let path = dir.join("a.jsonl");
    std::fs::write(&path, b"").expect("write fixture");

    assert_eq!(
        load_preview(&path, 10),
        "(no readable messages)",
        "an empty transcript has nothing to preview"
    );
}

#[test]
fn load_preview_reports_a_file_whose_records_carry_no_text() {
    let dir = TempDir::new("prev-blank");
    let path = dir.join("a.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"type":"user","message":{"content":[]}}"#,
            r#"{"type":"user","message":{"content":"   "}}"#,
        ],
    );

    assert_eq!(
        load_preview(&path, 10),
        "(no readable messages)",
        "records with no text should not render as empty role headers"
    );
}

#[test]
fn extract_text_prefers_a_summary() {
    assert_eq!(
        extract_text(&json(
            r#"{"summary":"picked","message":{"content":"ignored"}}"#
        )),
        "picked",
        "a summary record should win over its message content"
    );
}

#[test]
fn extract_text_reads_string_content() {
    assert_eq!(
        extract_text(&json(r#"{"message":{"content":"plain"}}"#)),
        "plain"
    );
}

#[test]
fn extract_text_joins_the_text_blocks_of_array_content() {
    assert_eq!(
        extract_text(&json(
            r#"{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#
        )),
        "a\nb\n",
        "each text block should contribute one line"
    );
}

#[test]
fn extract_text_reads_a_non_text_block_that_still_carries_text() {
    assert_eq!(
        extract_text(&json(
            r#"{"message":{"content":[{"type":"thinking","text":"pondering"}]}}"#
        )),
        "pondering\n",
        "a block with a text field should be shown whatever its type says"
    );
}

#[test]
fn extract_text_ignores_a_block_with_no_text_field() {
    assert_eq!(
        extract_text(&json(
            r#"{"message":{"content":[{"type":"tool_use","name":"Bash"},{"type":"text","text":"kept"}]}}"#
        )),
        "kept\n",
        "a tool-use block has nothing to show and should be skipped"
    );
}

#[test]
fn extract_text_returns_nothing_for_shapes_it_does_not_know() {
    assert_eq!(
        extract_text(&json(r#"{"type":"user"}"#)),
        "",
        "a record with no message should yield no text"
    );
    assert_eq!(
        extract_text(&json(r#"{"message":{"role":"user"}}"#)),
        "",
        "a message with no content should yield no text"
    );
    assert_eq!(
        extract_text(&json(r#"{"message":{"content":42}}"#)),
        "",
        "content that is neither a string nor an array should yield no text"
    );
}

#[test]
fn humanize_age_buckets_by_size() {
    assert_eq!(humanize_age(SystemTime::now()), "0s");
    assert_eq!(humanize_age(ago(59)), "59s");
    assert_eq!(humanize_age(ago(60)), "1m");
    assert_eq!(humanize_age(ago(3599)), "59m");
    assert_eq!(humanize_age(ago(3600)), "1h");
    assert_eq!(humanize_age(ago(86_399)), "23h");
    assert_eq!(humanize_age(ago(86_400)), "1d");
    assert_eq!(humanize_age(ago(10 * 86_400)), "10d");
}

#[test]
fn humanize_age_reads_a_future_time_as_no_age_at_all() {
    let future = SystemTime::now() + Duration::from_secs(3600);
    assert_eq!(
        humanize_age(future),
        "0s",
        "a mtime ahead of the clock should read as fresh, not wrap around"
    );
}

#[test]
fn scan_root_returns_nothing_for_a_missing_root() {
    let dir = TempDir::new("scan-gone");
    assert!(
        scan_root(&dir.join("nope")).is_empty(),
        "a missing projects root should scan as empty rather than panicking"
    );
}

#[test]
fn scan_root_returns_nothing_for_an_empty_root() {
    let dir = TempDir::new("scan-empty");
    assert!(
        scan_root(dir.path()).is_empty(),
        "a projects root with no project directories should scan as empty"
    );
}

#[test]
fn scan_root_reads_the_header_of_each_transcript() {
    let dir = TempDir::new("scan-header");
    let jsonl = dir.join("-home-u-proj").join("sess-1.jsonl");
    write_jsonl(
        &jsonl,
        &[
            r#"{"cwd":"/home/u/actual"}"#,
            r#"{"type":"custom-title","customTitle":"named run"}"#,
        ],
    );

    let found = scan_root(dir.path());
    assert_eq!(found.len(), 1, "expected exactly one transcript");
    assert_eq!(
        found[0].session_id, "sess-1",
        "session id comes from the file stem"
    );
    assert_eq!(
        found[0].cwd,
        PathBuf::from("/home/u/actual"),
        "the header cwd should win over the directory name"
    );
    assert_eq!(
        found[0].custom_title,
        Some("named run".to_string()),
        "the custom title should be carried through"
    );
    assert_eq!(
        found[0].file_size,
        std::fs::metadata(&jsonl).expect("stat fixture").len(),
        "file_size should match the file on disk"
    );
    assert_eq!(
        found[0].path, jsonl,
        "the transcript's own path is recorded"
    );
}

#[test]
fn scan_root_falls_back_to_the_directory_name_when_the_header_has_no_cwd() {
    let dir = TempDir::new("scan-fallback");
    write_jsonl(
        &dir.join("-home-u-proj").join("sess-1.jsonl"),
        &[r#"{"type":"user","message":{"content":"hi"}}"#],
    );

    let found = scan_root(dir.path());
    assert_eq!(found.len(), 1, "expected exactly one transcript");
    assert_eq!(
        found[0].cwd,
        PathBuf::from("/home/u/proj"),
        "with no cwd in the header the directory name should be decoded instead"
    );
    assert_eq!(
        found[0].custom_title, None,
        "a transcript with no title record should have no title"
    );
}

#[test]
fn scan_root_ignores_files_that_are_not_jsonl() {
    let dir = TempDir::new("scan-filter");
    let proj = dir.join("-home-u-proj");
    write_jsonl(&proj.join("sess-1.jsonl"), &[r#"{"cwd":"/home/u/proj"}"#]);
    std::fs::write(proj.join("notes.txt"), b"not a transcript").expect("write fixture");
    std::fs::write(proj.join("noext"), b"not a transcript").expect("write fixture");
    std::fs::write(dir.join("stray.jsonl"), b"{}").expect("write fixture");

    let found = scan_root(dir.path());
    assert_eq!(
        found.len(),
        1,
        "only the .jsonl inside a project directory should be picked up, got {:?}",
        found.iter().map(|t| &t.session_id).collect::<Vec<_>>()
    );
}

#[test]
fn scan_root_sorts_the_newest_transcript_first() {
    let dir = TempDir::new("scan-sort");
    let proj = dir.join("-home-u-proj");
    let old = proj.join("old.jsonl");
    let new = proj.join("new.jsonl");
    write_jsonl(&old, &[r#"{"cwd":"/home/u/proj"}"#]);
    write_jsonl(&new, &[r#"{"cwd":"/home/u/proj"}"#]);
    set_mtime(&old, 1_000);
    set_mtime(&new, 2_000);

    let found = scan_root(dir.path());
    let ids: Vec<&str> = found.iter().map(|t| t.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["new", "old"],
        "transcripts should come back newest first"
    );
}
