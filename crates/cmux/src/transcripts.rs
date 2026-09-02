use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::util::claude_projects_dir;

/// Records scanned from the head of a transcript for the recorded cwd.
const HEAD_LINES: usize = 40;
/// Window read from the end of a transcript for its newest records.
const TAIL_BYTES: u64 = 256 * 1024;
/// Marks a line that carries the session's display name.
const TITLE_RECORD: &str = "\"custom-title\"";

pub struct Transcript {
    pub session_id: String,
    /// The transcript file itself, as found by [`scan`].
    pub path: PathBuf,
    pub cwd: PathBuf,
    /// Session this conversation was forked from, for a transcript that
    /// records a `forkedFrom` origin.
    pub forked_from: Option<String>,
    pub mtime: SystemTime,
    pub file_size: u64,
    /// Display name set via `claude --name <name>`. Stored in the first lines
    /// of the JSONL as `{"type":"custom-title","customTitle":"..."}`.
    pub custom_title: Option<String>,
}

/// The leading segment of a session id, as `claude agents` and the picker's
/// id column show it.
pub fn short_id(session_id: &str) -> &str {
    &session_id[..8.min(session_id.len())]
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
    let Ok(read_root) = std::fs::read_dir(&root) else {
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
            let Header {
                cwd,
                custom_title,
                forked_from,
            } = extract_header(&p);
            let cwd = cwd.unwrap_or_else(|| slug_decode(&proj_dir));
            out.push(Transcript {
                session_id,
                path: p,
                cwd,
                forked_from,
                mtime,
                file_size: size,
                custom_title,
            });
        }
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.mtime));
    out
}

/// What [`scan`] reads out of a transcript's own records.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Header {
    cwd: Option<PathBuf>,
    custom_title: Option<String>,
    forked_from: Option<String>,
}

/// Pull the recorded cwd and fork origin from the head of a transcript and the
/// `--name`-supplied display name from its tail. `claude` re-emits the
/// `custom-title` record as the session runs, and the last copy carries the
/// current name; a copy inside the head window is the fallback.
fn extract_header(path: &Path) -> Header {
    let mut head = extract_head(path);
    head.custom_title = tail_title(path).or(head.custom_title);
    head
}

/// The recorded cwd, fork origin and any `custom-title` inside the first
/// `HEAD_LINES` records.
fn extract_head(path: &Path) -> Header {
    use std::io::{BufRead, BufReader};
    let mut head = Header::default();
    let Ok(f) = std::fs::File::open(path) else {
        return head;
    };
    let reader = BufReader::new(f);
    for line in reader.lines().take(HEAD_LINES).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if head.cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(|x| x.as_str())
            && !c.is_empty()
        {
            head.cwd = Some(PathBuf::from(c));
        }
        if head.forked_from.is_none() {
            head.forked_from = forked_from_of(&v);
        }
        if head.custom_title.is_none() {
            head.custom_title = title_of(&v);
        }
        if head.cwd.is_some() && head.custom_title.is_some() {
            break;
        }
    }
    head
}

/// The session id a `forkedFrom` origin names.
fn forked_from_of(v: &serde_json::Value) -> Option<String> {
    let id = v
        .get("forkedFrom")?
        .get("sessionId")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    Some(id.to_string())
}

/// The last `custom-title` record within the final `TAIL_BYTES` of the file.
/// A partial leading line from the seek is dropped.
fn tail_title(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let window = size.min(TAIL_BYTES);
    f.seek(SeekFrom::End(-(window as i64))).ok()?;
    let mut buf = vec![0u8; window as usize];
    f.read_exact(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let start = if window < size {
        text.find('\n').map(|i| i + 1).unwrap_or(text.len())
    } else {
        0
    };
    text[start..]
        .lines()
        .rev()
        .filter(|line| line.contains(TITLE_RECORD))
        .find_map(|line| {
            let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
            title_of(&v)
        })
}

/// The non-empty `customTitle` of a `custom-title` record.
fn title_of(v: &serde_json::Value) -> Option<String> {
    if v.get("type").and_then(|x| x.as_str()) != Some("custom-title") {
        return None;
    }
    let t = v.get("customTitle").and_then(|x| x.as_str())?;
    (!t.is_empty()).then(|| t.to_string())
}

pub fn load_preview(path: &std::path::Path, max_lines: usize) -> String {
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
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
    crate::util::format_duration_secs(secs)
}

#[cfg(test)]
#[path = "tests/transcripts.rs"]
mod tests;
