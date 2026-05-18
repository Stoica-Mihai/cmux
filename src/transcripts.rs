use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Transcript {
    pub session_id: String,
    pub cwd: PathBuf,
    pub mtime: SystemTime,
    pub file_size: u64,
}

pub fn scan() -> Vec<Transcript> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let root = PathBuf::from(home).join(".claude").join("projects");
    let Ok(read_root) = std::fs::read_dir(&root) else { return Vec::new() };

    let mut out: Vec<Transcript> = Vec::new();
    for entry in read_root.flatten() {
        let proj_dir = entry.path();
        let Ok(read_proj) = std::fs::read_dir(&proj_dir) else { continue };
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
            let cwd = extract_cwd(&p).unwrap_or_else(|| decode_slug(&proj_dir));
            out.push(Transcript {
                session_id,
                cwd,
                mtime,
                file_size: size,
            });
        }
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.mtime));
    out
}

fn extract_cwd(path: &Path) -> Option<PathBuf> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(f);
    for line in reader.lines().take(40).flatten() {
        let v: serde_json::Value = serde_json::from_str(&line).ok()?;
        if let Some(cwd) = v.get("cwd").and_then(|x| x.as_str())
            && !cwd.is_empty()
        {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

fn decode_slug(dir: &Path) -> PathBuf {
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

pub fn load_preview(path: &std::path::Path, max_lines: usize) -> String {
    use std::io::{BufRead, BufReader};
    let Ok(f) = std::fs::File::open(path) else {
        return String::from("(unable to read transcript)");
    };
    let reader = BufReader::new(f);
    let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
    let start = lines.len().saturating_sub(max_lines * 3);
    let mut out: Vec<String> = Vec::new();
    for raw in &lines[start..] {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let role = v
            .get("type")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("message").and_then(|m| m.get("role")).and_then(|x| x.as_str()))
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
    let dur = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or_default();
    let secs = dur.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}
