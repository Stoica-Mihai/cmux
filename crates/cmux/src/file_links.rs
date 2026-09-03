//! `file://` links synthesised over a session's output.
//!
//! claude prints file paths as plain text and wraps none of them in OSC 8, so
//! the hyperlink pass has nothing to carry through for them the way it does for
//! URLs. This finds the paths on screen instead, keeps only the ones that name
//! something that exists, and hands back a target per cell for that pass to
//! wrap.
//!
//! A path is resolved against the session's directory, so the relative forms
//! claude prints most often - `src/main.rs`, `notes.txt` - resolve the same way
//! they would in that session's shell.
//!
//! Under WSL the target has to be one the host can open. Windows reaches the
//! Linux filesystem through the UNC share `\\wsl.localhost\<distro>`, which as
//! a URL is `file://wsl.localhost/<distro>/...`, and reaches a drive mount such
//! as `/mnt/c` as `C:` directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Longest token still worth a look.
const MAX_TOKEN: usize = 512;

/// How long a resolved token stays cached. A file created or removed while a
/// session is open is picked up on the next sweep.
const CACHE_TTL_MS: u64 = 2_000;

/// Cached tokens above which the map is dropped whole.
const CACHE_CAP: usize = 8_192;

/// Characters that never carry into a path token. Alongside whitespace and
/// quoting, this covers the glyphs claude draws its output with - box borders,
/// bullets, arrows - which sit against the text they decorate.
fn is_boundary(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '|' | '\'' | '"' | '`' | ',' | ';')
        || matches!(c,
            '\u{2022}'                 // bullet
            | '\u{2190}'..='\u{21FF}'   // arrows
            | '\u{23B0}'..='\u{23BF}'   // the brackets claude's tool results use
            | '\u{2500}'..='\u{259F}'   // box drawing and blocks
            | '\u{25A0}'..='\u{25FF}'   // geometric shapes
        )
}

/// Characters a path token never opens with.
const LEAD_TRIM: &[char] = &['(', '[', '{', '<', '*', '@', '+', '-', '=', '#'];

/// Characters a path token never closes with.
const TAIL_TRIM: &[char] = &[
    ')', ']', '}', '>', '*', '.', '!', '?', ':', '=', '#', '\u{2026}',
];

/// Synthesised link targets, by cell, for one screen.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileLinks {
    /// Target index per `(row, col)` of the tile, in screen coordinates.
    at: HashMap<(u16, u16), usize>,
    uris: Vec<String>,
}

impl FileLinks {
    /// The link a cell carries, as an id and a target. The id groups every
    /// occurrence of one path, so a terminal highlights them together.
    pub fn get(&self, row: u16, col: u16) -> Option<(String, &str)> {
        let idx = *self.at.get(&(row, col))?;
        Some((format!("f{idx}"), self.uris[idx].as_str()))
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    #[cfg(test)]
    pub fn uri_at(&self, row: u16, col: u16) -> Option<&str> {
        self.at.get(&(row, col)).map(|&i| self.uris[i].as_str())
    }

    #[cfg(test)]
    pub fn cells(&self) -> usize {
        self.at.len()
    }
}

/// Tokens already resolved, so a screen redrawn many times a second is not
/// re-checked against the filesystem every frame.
#[derive(Debug, Default)]
pub struct Cache {
    resolved: HashMap<(PathBuf, String), Option<String>>,
    filled_ms: u64,
}

impl Cache {
    /// The target a token names, or `None` when it names nothing that exists.
    fn uri_for(&mut self, cwd: &Path, token: &str, now_ms: u64) -> Option<String> {
        if now_ms.saturating_sub(self.filled_ms) > CACHE_TTL_MS || self.resolved.len() > CACHE_CAP {
            self.resolved.clear();
            self.filled_ms = now_ms;
        }
        let key = (cwd.to_path_buf(), token.to_string());
        if let Some(hit) = self.resolved.get(&key) {
            return hit.clone();
        }
        let uri = resolve(token, cwd).map(|abs| file_uri(&abs));
        self.resolved.insert(key, uri.clone());
        uri
    }
}

/// Find the paths on a screen and build a link target for each cell they cover.
///
/// `rows` comes from `cmux_term::grid_rows`: one entry per screen row, full
/// width, with the flag saying the terminal wrapped it into the row below. A
/// path split across a wrap is found whole and linked on both rows.
pub fn detect(rows: &[(String, bool)], cwd: &Path, cache: &mut Cache, now_ms: u64) -> FileLinks {
    let mut links = FileLinks::default();
    let mut targets: HashMap<String, usize> = HashMap::new();
    // Reused across logical lines, so a screen costs one allocation, not one
    // per line.
    let mut chars: Vec<char> = Vec::new();
    let mut widths: Vec<usize> = Vec::new();
    let mut token = String::new();

    let mut start = 0;
    while start < rows.len() {
        let next = gather(rows, start, &mut chars, &mut widths);
        for (offset, span) in tokens(&chars) {
            let Some((lead, body)) = trim(span) else {
                continue;
            };
            if !plausible(body) {
                continue;
            }
            token.clear();
            token.extend(body.iter());
            let Some(uri) = cache.uri_for(cwd, &token, now_ms) else {
                continue;
            };
            let idx = *targets.entry(uri.clone()).or_insert_with(|| {
                links.uris.push(uri);
                links.uris.len() - 1
            });
            let from = offset + lead;
            for off in from..from + body.len() {
                if let Some((row, col)) = place(start, &widths, off) {
                    links.at.insert((row, col), idx);
                }
            }
        }
        start = next;
    }
    links
}

/// Join the wrapped run of rows starting at `start` into `chars`, recording how
/// many characters each row contributed in `widths`. Returns the row the next
/// logical line starts at.
fn gather(
    rows: &[(String, bool)],
    start: usize,
    chars: &mut Vec<char>,
    widths: &mut Vec<usize>,
) -> usize {
    chars.clear();
    widths.clear();
    let mut i = start;
    loop {
        let (text, wrapped) = &rows[i];
        let before = chars.len();
        chars.extend(text.chars());
        widths.push(chars.len() - before);
        i += 1;
        if !wrapped || i >= rows.len() {
            return i;
        }
    }
}

/// The screen cell a character offset within a logical line sits at.
fn place(start: usize, widths: &[usize], offset: usize) -> Option<(u16, u16)> {
    let mut left = offset;
    for (i, &w) in widths.iter().enumerate() {
        if left < w {
            return Some(((start + i).try_into().ok()?, left.try_into().ok()?));
        }
        left -= w;
    }
    None
}

/// The whitespace-and-punctuation separated runs of a logical line, each with
/// its character offset.
fn tokens(chars: &[char]) -> Vec<(usize, &[char])> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &c) in chars.iter().enumerate() {
        if is_boundary(c) {
            if let Some(s) = start.take() {
                out.push((s, &chars[s..i]));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((s, &chars[s..]));
    }
    out
}

/// A token with its surrounding punctuation removed, plus how many characters
/// came off the front. A `:12` or `:12:3` line reference is dropped, so
/// `main.rs:426` links `main.rs`.
fn trim(token: &[char]) -> Option<(usize, &[char])> {
    let lead = token
        .iter()
        .position(|c| !LEAD_TRIM.contains(c))
        .unwrap_or(token.len());
    let mut body = &token[lead..];
    while body.last().is_some_and(|c| TAIL_TRIM.contains(c)) {
        body = &body[..body.len() - 1];
    }
    while let Some(colon) = body.iter().rposition(|&c| c == ':') {
        let tail = &body[colon + 1..];
        if tail.is_empty() || !tail.iter().all(char::is_ascii_digit) {
            break;
        }
        body = &body[..colon];
    }
    (!body.is_empty()).then_some((lead, body))
}

/// Whether a token is shaped enough like a path to be worth a filesystem
/// check. A bare word is not: half the words in a sentence would name a
/// directory somewhere and light up as a link.
fn plausible(token: &[char]) -> bool {
    if token.is_empty() || token.len() > MAX_TOKEN {
        return false;
    }
    if token.windows(3).any(|w| w == [':', '/', '/']) {
        return false;
    }
    if token.contains(&'/') || token[0] == '~' {
        return true;
    }
    // A bare filename needs an extension: something before the dot and
    // something after it.
    match token.iter().rposition(|&c| c == '.') {
        Some(i) => i > 0 && i + 1 < token.len(),
        None => false,
    }
}

/// The absolute path a token names, when it names one that exists. Relative
/// tokens resolve against `cwd`, and `~` against the home directory.
fn resolve(token: &str, cwd: &Path) -> Option<PathBuf> {
    let path = if token == "~" {
        crate::util::home()?
    } else if let Some(rest) = token.strip_prefix("~/") {
        crate::util::home()?.join(rest)
    } else if token.starts_with('/') {
        PathBuf::from(token)
    } else {
        cwd.join(token)
    };
    std::fs::canonicalize(path).ok()
}

/// A `file://` target the host terminal can open.
///
/// Under WSL a Linux path goes through the distribution's UNC share and a drive
/// mount goes straight to its drive letter. Elsewhere the path is the URL's
/// path as it stands.
pub fn file_uri(abs: &Path) -> String {
    uri_under(abs, wsl_distro().as_deref())
}

/// The same, with the distribution passed in rather than read from the
/// environment.
fn uri_under(abs: &Path, distro: Option<&str>) -> String {
    let path = abs.to_string_lossy();
    match distro {
        Some(distro) => match drive_mount(&path) {
            Some((letter, rest)) => format!("file:///{letter}:{}", encode(&rest)),
            None => format!("file://wsl.localhost/{}{}", encode(distro), encode(&path)),
        },
        None => format!("file://{}", encode(&path)),
    }
}

/// The WSL distribution this is running under, if any.
fn wsl_distro() -> Option<String> {
    std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|s| !s.is_empty())
}

/// The drive letter and remainder of a `/mnt/<letter>` path.
fn drive_mount(path: &str) -> Option<(char, String)> {
    let rest = path.strip_prefix("/mnt/")?;
    let mut chars = rest.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    match chars.next() {
        None => Some((letter, "/".to_string())),
        Some('/') => Some((letter, format!("/{}", chars.as_str()))),
        Some(_) => None,
    }
}

/// Percent-encode everything a URL path may not carry literally.
fn encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/file_links.rs"]
mod tests;
