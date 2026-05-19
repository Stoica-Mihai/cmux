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
        let label = match role {
            "user" => "▸ user",
            "assistant" => "◂ assistant",
            "system" => "· system",
            "summary" => "· summary",
            other => other,
        };
        let text = extract_text(&v);
        if text.trim().is_empty() {
            continue;
        }
        out.push(format!("[{}]", label));
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
