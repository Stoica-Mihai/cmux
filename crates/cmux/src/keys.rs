//! Keyboard chord registry + encoder.
//!
//! Each user-visible binding is a `Chord` constant. Handlers in `main.rs`
//! match via `Chord::matches(&KeyEvent)`. UI labels in `ui.rs` read
//! `Chord::label`. One edit moves both the binding and its display.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct Chord {
    pub codes: &'static [KeyCode],
    pub mods: KeyModifiers,
    pub label: &'static str,
}

impl Chord {
    pub fn matches(&self, k: &KeyEvent) -> bool {
        k.modifiers.contains(self.mods) && self.codes.contains(&k.code)
    }
}

const NO_MODS: KeyModifiers = KeyModifiers::empty();
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

// ---------------------------------------------------------------------------
// Global
// ---------------------------------------------------------------------------
/// The only global binding. Every command goes through it, so there is one
/// way to do each thing and nothing is bound out from under a session.
pub const PREFIX: Chord = Chord {
    codes: &[KeyCode::Char('a'), KeyCode::Char('A')],
    mods: CTRL,
    label: "ctrl+a",
};

// ---------------------------------------------------------------------------
// Prefix chord (after PREFIX)
// ---------------------------------------------------------------------------
pub const PREFIX_QUIT: Chord = Chord {
    codes: &[KeyCode::Char('q'), KeyCode::Char('Q')],
    mods: NO_MODS,
    label: "q",
};
pub const PREFIX_SPAWN: Chord = Chord {
    codes: &[KeyCode::Char('n'), KeyCode::Char('N')],
    mods: NO_MODS,
    label: "n",
};
pub const PREFIX_DETACH: Chord = Chord {
    codes: &[KeyCode::Char('d'), KeyCode::Char('D')],
    mods: NO_MODS,
    label: "d",
};
pub const PREFIX_TOGGLE_SIDEBAR: Chord = Chord {
    codes: &[KeyCode::Char('z'), KeyCode::Char('Z')],
    mods: NO_MODS,
    label: "z",
};
pub const PREFIX_RENAME: Chord = Chord {
    codes: &[KeyCode::Char('r'), KeyCode::Char('R')],
    mods: NO_MODS,
    label: "r",
};
pub const PREFIX_PICKER: Chord = Chord {
    codes: &[KeyCode::Char('l'), KeyCode::Char('L')],
    mods: NO_MODS,
    label: "l",
};
pub const PREFIX_SEND_CTRL_A: Chord = Chord {
    codes: &[KeyCode::Char('a'), KeyCode::Char('A')],
    mods: NO_MODS,
    label: "a",
};
pub const PREFIX_SCROLLBACK: Chord = Chord {
    codes: &[KeyCode::Char('[')],
    mods: NO_MODS,
    label: "[",
};
pub const PREFIX_HELP: Chord = Chord {
    codes: &[KeyCode::Char('?')],
    mods: NO_MODS,
    label: "?",
};
pub const PREFIX_REORDER: Chord = Chord {
    codes: &[KeyCode::Char('m'), KeyCode::Char('M')],
    mods: NO_MODS,
    label: "m",
};
pub const PREFIX_FOCUS_NEXT: Chord = Chord {
    codes: &[KeyCode::Down],
    mods: NO_MODS,
    label: "↓",
};
pub const PREFIX_FOCUS_PREV: Chord = Chord {
    codes: &[KeyCode::Up],
    mods: NO_MODS,
    label: "↑",
};

// ---------------------------------------------------------------------------
// Scrollback
// ---------------------------------------------------------------------------
pub const SCROLLBACK_UP: Chord = Chord {
    codes: &[KeyCode::Up, KeyCode::Char('k')],
    mods: NO_MODS,
    label: "↑/k",
};
pub const SCROLLBACK_DOWN: Chord = Chord {
    codes: &[KeyCode::Down, KeyCode::Char('j')],
    mods: NO_MODS,
    label: "↓/j",
};
pub const SCROLLBACK_PAGE_UP: Chord = Chord {
    codes: &[KeyCode::PageUp, KeyCode::Char('b')],
    mods: NO_MODS,
    label: "pgup/b",
};
pub const SCROLLBACK_PAGE_DOWN: Chord = Chord {
    codes: &[KeyCode::PageDown, KeyCode::Char('f'), KeyCode::Char(' ')],
    mods: NO_MODS,
    label: "pgdn/f/space",
};
pub const SCROLLBACK_TOP: Chord = Chord {
    codes: &[KeyCode::Home, KeyCode::Char('g')],
    mods: NO_MODS,
    label: "g",
};
pub const SCROLLBACK_BOTTOM: Chord = Chord {
    codes: &[KeyCode::End, KeyCode::Char('G')],
    mods: NO_MODS,
    label: "G",
};
pub const SCROLLBACK_EXIT: Chord = Chord {
    codes: &[KeyCode::Esc, KeyCode::Enter, KeyCode::Char('q')],
    mods: NO_MODS,
    label: "q/esc",
};

// ---------------------------------------------------------------------------
// Reorder
// ---------------------------------------------------------------------------
pub const REORDER_UP: Chord = Chord {
    codes: &[KeyCode::Up, KeyCode::Char('k')],
    mods: NO_MODS,
    label: "↑/k",
};
pub const REORDER_DOWN: Chord = Chord {
    codes: &[KeyCode::Down, KeyCode::Char('j')],
    mods: NO_MODS,
    label: "↓/j",
};
pub const REORDER_EXIT: Chord = Chord {
    codes: &[KeyCode::Esc, KeyCode::Enter, KeyCode::Char('q')],
    mods: NO_MODS,
    label: "esc/enter/q",
};

// ---------------------------------------------------------------------------
// Confirm-detach
// ---------------------------------------------------------------------------
pub const CONFIRM_YES: Chord = Chord {
    codes: &[KeyCode::Char('y'), KeyCode::Char('Y'), KeyCode::Enter],
    mods: NO_MODS,
    label: "y/enter",
};
pub const CONFIRM_NO: Chord = Chord {
    codes: &[KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc],
    mods: NO_MODS,
    label: "n/esc",
};

// ---------------------------------------------------------------------------
// Spawn (cwd picker)
// ---------------------------------------------------------------------------
pub const SPAWN_UP: Chord = Chord {
    codes: &[KeyCode::Up, KeyCode::Char('k')],
    mods: NO_MODS,
    label: "↑/k",
};
pub const SPAWN_DOWN: Chord = Chord {
    codes: &[KeyCode::Down, KeyCode::Char('j')],
    mods: NO_MODS,
    label: "↓/j",
};
pub const SPAWN_PGUP: Chord = Chord {
    codes: &[KeyCode::PageUp],
    mods: NO_MODS,
    label: "pgup",
};
pub const SPAWN_PGDOWN: Chord = Chord {
    codes: &[KeyCode::PageDown],
    mods: NO_MODS,
    label: "pgdn",
};
pub const SPAWN_HOME: Chord = Chord {
    codes: &[KeyCode::Home],
    mods: NO_MODS,
    label: "home",
};
pub const SPAWN_END: Chord = Chord {
    codes: &[KeyCode::End],
    mods: NO_MODS,
    label: "end",
};
pub const SPAWN_DESCEND: Chord = Chord {
    codes: &[KeyCode::Right, KeyCode::Char('l')],
    mods: NO_MODS,
    label: "→/l",
};
pub const SPAWN_ASCEND: Chord = Chord {
    codes: &[KeyCode::Left, KeyCode::Char('h'), KeyCode::Backspace],
    mods: NO_MODS,
    label: "←/h",
};
pub const SPAWN_PICK: Chord = Chord {
    codes: &[KeyCode::Enter],
    mods: NO_MODS,
    label: "enter",
};
pub const SPAWN_CANCEL: Chord = Chord {
    codes: &[KeyCode::Esc],
    mods: NO_MODS,
    label: "esc",
};
pub const SPAWN_TOGGLE_DANGER: Chord = Chord {
    codes: &[KeyCode::Char(' ')],
    mods: NO_MODS,
    label: "space",
};

// ---------------------------------------------------------------------------
// Picker (resume)
// ---------------------------------------------------------------------------
pub const PICKER_UP: Chord = Chord {
    codes: &[KeyCode::Up],
    mods: NO_MODS,
    label: "↑",
};
pub const PICKER_DOWN: Chord = Chord {
    codes: &[KeyCode::Down],
    mods: NO_MODS,
    label: "↓",
};
pub const PICKER_PGUP: Chord = Chord {
    codes: &[KeyCode::PageUp],
    mods: NO_MODS,
    label: "pgup",
};
pub const PICKER_PGDOWN: Chord = Chord {
    codes: &[KeyCode::PageDown],
    mods: NO_MODS,
    label: "pgdn",
};
pub const PICKER_HOME: Chord = Chord {
    codes: &[KeyCode::Home],
    mods: NO_MODS,
    label: "home",
};
pub const PICKER_END: Chord = Chord {
    codes: &[KeyCode::End],
    mods: NO_MODS,
    label: "end",
};
pub const PICKER_PICK: Chord = Chord {
    codes: &[KeyCode::Enter],
    mods: NO_MODS,
    label: "enter",
};
pub const PICKER_CANCEL: Chord = Chord {
    codes: &[KeyCode::Esc],
    mods: NO_MODS,
    label: "esc",
};
pub const PICKER_FILTER_CLEAR: Chord = Chord {
    codes: &[KeyCode::Backspace],
    mods: NO_MODS,
    label: "backspace",
};
pub const PICKER_TOGGLE_DANGER: Chord = Chord {
    codes: &[KeyCode::Tab],
    mods: NO_MODS,
    label: "tab",
};

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------
pub const RENAME_SAVE: Chord = Chord {
    codes: &[KeyCode::Enter],
    mods: NO_MODS,
    label: "enter",
};
pub const RENAME_CANCEL: Chord = Chord {
    codes: &[KeyCode::Esc],
    mods: NO_MODS,
    label: "esc",
};

pub fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let mut out: Vec<u8> = Vec::new();
    if alt {
        out.push(0x1b);
    }

    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let b = match c {
                    ' ' => 0x00,
                    'a'..='z' => (c as u8) - b'a' + 1,
                    'A'..='Z' => (c as u8) - b'A' + 1,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' => 0x1e,
                    '_' => 0x1f,
                    '?' => 0x7f,
                    _ => return None,
                };
                out.push(b);
            } else {
                let mut buf = [0u8; 4];
                let s = if shift {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                };
                let s = s.encode_utf8(&mut buf);
                out.extend_from_slice(s.as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::F(n) => {
            let seq: &[u8] = match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => return None,
            };
            out.extend_from_slice(seq);
        }
        _ => return None,
    }
    Some(out)
}
