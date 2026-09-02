use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::util::claude_projects_dir;

pub struct Transcript {
    pub session_id: String,
    pub cwd: PathBuf,
    pub mtime: SystemTime,
    pub file_size: u64,
    /// Display name set via `claude --name <name>`. Stored in the first lines
    /// of the JSONL as `{"type":"custom-title","customTitle":"..."}`.
    pub custom_title: Option<String>,
}

pub fn slug_encode(cwd: &Path) -> String {
    cwd.display().to_string().replace('/', "-")
}

pub fn slug_decode(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut s = name.replace('-', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    PathBuf::from(s)
}

pub fn scan() -> Vec<Transcript> {
    let Some(root) = claude_projects_dir() else {
        return Vec::new();
    };
    scan_root(&root)
}

/// Walk one `projects` root. Split from `scan` so tests can pass a fixture tree.
fn scan_root(root: &Path) -> Vec<Transcript> {
    let Ok(read_root) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut out: Vec<Transcript> = Vec::new();
    for entry in read_root.flatten() {
        let proj_dir = entry.path();
        let Ok(read_proj) = std::fs::read_dir(&proj_dir) else {
            continue;
        };
        for f in read_proj.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let size = meta.len();
            let session_id = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (cwd, custom_title) = extract_header(&p);
            let cwd = cwd.unwrap_or_else(|| slug_decode(&proj_dir));
            out.push(Transcript {
                session_id,
                cwd,
                mtime,
                file_size: size,
                custom_title,
            });
        }
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.mtime));
    out
}

/// Read the early portion of a transcript and pull both the recorded cwd and
/// the optional `--name`-supplied custom title. Both fields tend to appear in
/// the first handful of records; scanning the same 40-line window once keeps
/// the picker scan I/O-bound on the directory walk, not on per-file reads.
fn extract_header(path: &Path) -> (Option<PathBuf>, Option<String>) {
    use std::io::{BufRead, BufReader};
    let Ok(f) = std::fs::File::open(path) else {
        return (None, None);
    };
    let reader = BufReader::new(f);
    let mut cwd: Option<PathBuf> = None;
    let mut title: Option<String> = None;
    for line in reader.lines().take(40).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(|x| x.as_str())
            && !c.is_empty()
        {
            cwd = Some(PathBuf::from(c));
        }
        if title.is_none()
            && v.get("type").and_then(|x| x.as_str()) == Some("custom-title")
            && let Some(t) = v.get("customTitle").and_then(|x| x.as_str())
            && !t.is_empty()
        {
            title = Some(t.to_string());
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    (cwd, title)
}

pub fn load_preview(path: &std::path::Path, max_lines: usize) -> String {
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
    const TAIL_BYTES: u64 = 256 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::from("(unable to read transcript)");
    };
    let cap = max_lines * 3;
    let file_size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let tail: VecDeque<String> = if file_size > TAIL_BYTES {
        // seek near end, discard partial first line, parse the rest
        if f.seek(SeekFrom::End(-(TAIL_BYTES as i64))).is_err() {
            return String::from("(unable to seek transcript)");
        }
        let mut buf = String::new();
        if f.take(TAIL_BYTES).read_to_string(&mut buf).is_err() {
            return String::from("(unable to read transcript tail)");
        }
        let start = buf.find('\n').map(|i| i + 1).unwrap_or(0);
        let mut tail: VecDeque<String> = VecDeque::with_capacity(cap);
        for line in buf[start..].lines() {
            if tail.len() == cap {
                tail.pop_front();
            }
            tail.push_back(line.to_string());
        }
        tail
    } else {
        let reader = BufReader::new(f);
        let mut tail: VecDeque<String> = VecDeque::with_capacity(cap);
        for line in reader.lines().map_while(Result::ok) {
            if tail.len() == cap {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail
    };
    let mut out: Vec<String> = Vec::new();
    for raw in &tail {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let role = v
            .get("type")
            .and_then(|x| x.as_str())
            .or_else(|| {
                v.get("message")
                    .and_then(|m| m.get("role"))
                    .and_then(|x| x.as_str())
            })
            .unwrap_or("?");
        let text = extract_text(&v);
        if text.trim().is_empty() {
            continue;
        }
        out.push(format!("[{}]", role));
        for line in text.lines().take(8) {
            out.push(format!("  {}", line));
        }
        out.push(String::new());
    }
    if out.is_empty() {
        return String::from("(no readable messages)");
    }
    let tail_start = out.len().saturating_sub(max_lines);
    out[tail_start..].join("\n")
}

fn extract_text(v: &serde_json::Value) -> String {
    if let Some(s) = v.get("summary").and_then(|x| x.as_str()) {
        return s.to_string();
    }
    let Some(msg) = v.get("message") else {
        return String::new();
    };
    let Some(content) = msg.get("content") else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for block in arr {
            if block.get("type").and_then(|x| x.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                    out.push_str(t);
                    out.push('\n');
                }
            } else if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                out.push_str(t);
                out.push('\n');
            }
        }
        return out;
    }
    String::new()
}

pub fn humanize_age(mtime: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or_default()
        .as_secs();
    crate::util::format_duration_secs(secs, " ago")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

    /// A temp directory that deletes itself, so a panicking test leaves nothing.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("cmux-transcripts-{tag}-{}-{n}", std::process::id()));
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

    /// Encode a path and decode the directory name it produces.
    fn round_trip(p: &str) -> PathBuf {
        let encoded = slug_encode(Path::new(p));
        slug_decode(&Path::new("/any/projects").join(encoded))
    }

    fn ago(secs: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(secs)
    }

    #[test]
    fn slug_encode_replaces_every_separator_with_a_dash() {
        assert_eq!(slug_encode(Path::new("/home/u/proj")), "-home-u-proj");
        assert_eq!(slug_encode(Path::new("/")), "-");
        assert_eq!(slug_encode(Path::new("rel/path")), "rel-path");
    }

    #[test]
    fn slug_decode_restores_a_dash_free_path() {
        assert_eq!(
            round_trip("/home/u/proj"),
            PathBuf::from("/home/u/proj"),
            "a nested dash-free path should survive encode then decode"
        );
        assert_eq!(
            round_trip("/home"),
            PathBuf::from("/home"),
            "a single-component path should survive encode then decode"
        );
        assert_eq!(
            round_trip("/"),
            PathBuf::from("/"),
            "the root path should survive encode then decode"
        );
    }

    #[test]
    fn slug_decode_turns_a_dash_in_the_original_path_into_a_separator() {
        assert_eq!(
            slug_encode(Path::new("/home/u/my-project")),
            "-home-u-my-project",
            "encoding does not escape a dash that was already in the path"
        );
        assert_eq!(
            round_trip("/home/u/my-project"),
            PathBuf::from("/home/u/my/project"),
            "known asymmetry: the encoding is lossy, so a dashed path decodes to the wrong directory"
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

        let (cwd, title) = extract_header(&path);
        assert_eq!(
            cwd,
            Some(PathBuf::from("/home/u/proj")),
            "the recorded cwd was not read back"
        );
        assert_eq!(
            title,
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

        let (cwd, _) = extract_header(&path);
        assert_eq!(
            cwd,
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

        let (cwd, title) = extract_header(&path);
        assert_eq!(
            cwd,
            Some(PathBuf::from("/home/u/proj")),
            "a malformed line must not stop the scan before the cwd"
        );
        assert_eq!(
            title,
            Some("still found".to_string()),
            "a malformed line must not stop the scan before the title"
        );
    }

    #[test]
    fn extract_header_keeps_the_first_cwd_it_sees() {
        let dir = TempDir::new("header-first");
        let path = dir.join("a.jsonl");
        write_jsonl(&path, &[r#"{"cwd":"/first"}"#, r#"{"cwd":"/second"}"#]);

        let (cwd, _) = extract_header(&path);
        assert_eq!(
            cwd,
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

        let (cwd, title) = extract_header(&path);
        assert_eq!(cwd, None, "an empty cwd string is not a usable cwd");
        assert_eq!(title, None, "an empty custom title is not a usable title");
    }

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
            extract_header(&inside).0,
            Some(PathBuf::from("/home/u/edge")),
            "line 40 is inside the scan window and should be read"
        );
        assert_eq!(
            extract_header(&outside).0,
            None,
            "line 41 is past the scan window and should not be read"
        );
    }

    #[test]
    fn extract_header_returns_nothing_for_an_empty_file() {
        let dir = TempDir::new("header-empty");
        let path = dir.join("a.jsonl");
        std::fs::write(&path, b"").expect("write fixture");

        assert_eq!(
            extract_header(&path),
            (None, None),
            "an empty transcript has no header to read"
        );
    }

    #[test]
    fn extract_header_returns_nothing_for_a_missing_file() {
        let dir = TempDir::new("header-gone");
        assert_eq!(
            extract_header(&dir.join("nope.jsonl")),
            (None, None),
            "a missing transcript should read as no header rather than panicking"
        );
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
        assert_eq!(humanize_age(SystemTime::now()), "0s ago");
        assert_eq!(humanize_age(ago(59)), "59s ago");
        assert_eq!(humanize_age(ago(60)), "1m ago");
        assert_eq!(humanize_age(ago(3599)), "59m ago");
        assert_eq!(humanize_age(ago(3600)), "1h ago");
        assert_eq!(humanize_age(ago(86_399)), "23h ago");
        assert_eq!(humanize_age(ago(86_400)), "1d ago");
        assert_eq!(humanize_age(ago(10 * 86_400)), "10d ago");
    }

    #[test]
    fn humanize_age_reads_a_future_time_as_no_age_at_all() {
        let future = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(
            humanize_age(future),
            "0s ago",
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
}
