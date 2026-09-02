use super::*;

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
