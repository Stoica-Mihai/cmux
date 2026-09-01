use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Format an elapsed-seconds count as a compact `s/m/h/d` cell. Caller picks
/// the suffix — `" ago"` reads naturally on a timestamp column, `""` on a
/// sidebar status line.
pub fn format_duration_secs(secs: u64, suffix: &str) -> String {
    if secs < 60 {
        format!("{}s{}", secs, suffix)
    } else if secs < 3600 {
        format!("{}m{}", secs / 60, suffix)
    } else if secs < 86_400 {
        format!("{}h{}", secs / 3600, suffix)
    } else {
        format!("{}d{}", secs / 86_400, suffix)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn wrap_index(cur: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    ((cur as i32 + delta).rem_euclid(len as i32)) as usize
}

pub fn debug_enabled() -> bool {
    std::env::var_os("CMUX_DEBUG").is_some()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("cmux"))
}

pub fn claude_projects_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".claude").join("projects"))
}

pub fn claude_sessions_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".claude").join("sessions"))
}

#[macro_export]
macro_rules! debug_log {
    ($path:expr, $($arg:tt)*) => {
        if $crate::util::debug_enabled() {
            if let Ok(mut __f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open($path)
            {
                use std::io::Write;
                let _ = writeln!(__f, $($arg)*);
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_secs_buckets() {
        assert_eq!(format_duration_secs(0, ""), "0s");
        assert_eq!(format_duration_secs(59, " ago"), "59s ago");
        assert_eq!(format_duration_secs(60, ""), "1m");
        assert_eq!(format_duration_secs(3599, " ago"), "59m ago");
        assert_eq!(format_duration_secs(3600, ""), "1h");
        assert_eq!(format_duration_secs(86_399, " ago"), "23h ago");
        assert_eq!(format_duration_secs(86_400, ""), "1d");
        assert_eq!(format_duration_secs(10 * 86_400, " ago"), "10d ago");
    }

    #[test]
    fn wrap_index_handles_empty() {
        assert_eq!(wrap_index(0, 0, 1), 0);
        assert_eq!(wrap_index(5, 0, -3), 0);
    }

    #[test]
    fn wrap_index_wraps_both_directions() {
        assert_eq!(wrap_index(0, 4, 1), 1);
        assert_eq!(wrap_index(3, 4, 1), 0); // forward wrap
        assert_eq!(wrap_index(0, 4, -1), 3); // backward wrap
        assert_eq!(wrap_index(2, 4, 5), 3); // big delta
        assert_eq!(wrap_index(2, 4, -7), 3); // big negative delta
    }
}
