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
    label: "Ctrl+A",
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
    label: "PgUp/b",
};
pub const SCROLLBACK_PAGE_DOWN: Chord = Chord {
    codes: &[KeyCode::PageDown, KeyCode::Char('f'), KeyCode::Char(' ')],
    mods: NO_MODS,
    label: "PgDn/f/Space",
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
    label: "q/Esc",
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
    label: "Esc/Enter/q",
};

// ---------------------------------------------------------------------------
// Confirm-detach
// ---------------------------------------------------------------------------
pub const CONFIRM_YES: Chord = Chord {
    codes: &[KeyCode::Char('y'), KeyCode::Char('Y'), KeyCode::Enter],
    mods: NO_MODS,
    label: "y/Enter",
};
pub const CONFIRM_NO: Chord = Chord {
    codes: &[KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc],
    mods: NO_MODS,
    label: "n/Esc",
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
    label: "PgUp",
};
pub const SPAWN_PGDOWN: Chord = Chord {
    codes: &[KeyCode::PageDown],
    mods: NO_MODS,
    label: "PgDn",
};
pub const SPAWN_HOME: Chord = Chord {
    codes: &[KeyCode::Home],
    mods: NO_MODS,
    label: "Home",
};
pub const SPAWN_END: Chord = Chord {
    codes: &[KeyCode::End],
    mods: NO_MODS,
    label: "End",
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
    label: "Enter",
};
pub const SPAWN_CANCEL: Chord = Chord {
    codes: &[KeyCode::Esc],
    mods: NO_MODS,
    label: "Esc",
};
pub const SPAWN_TOGGLE_DANGER: Chord = Chord {
    codes: &[KeyCode::Char(' ')],
    mods: NO_MODS,
    label: "Space",
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
    label: "PgUp",
};
pub const PICKER_PGDOWN: Chord = Chord {
    codes: &[KeyCode::PageDown],
    mods: NO_MODS,
    label: "PgDn",
};
pub const PICKER_HOME: Chord = Chord {
    codes: &[KeyCode::Home],
    mods: NO_MODS,
    label: "Home",
};
pub const PICKER_END: Chord = Chord {
    codes: &[KeyCode::End],
    mods: NO_MODS,
    label: "End",
};
pub const PICKER_PICK: Chord = Chord {
    codes: &[KeyCode::Enter],
    mods: NO_MODS,
    label: "Enter",
};
pub const PICKER_CANCEL: Chord = Chord {
    codes: &[KeyCode::Esc],
    mods: NO_MODS,
    label: "Esc",
};
pub const PICKER_FILTER_CLEAR: Chord = Chord {
    codes: &[KeyCode::Backspace],
    mods: NO_MODS,
    label: "Backspace",
};
pub const PICKER_TOGGLE_DANGER: Chord = Chord {
    codes: &[KeyCode::Tab],
    mods: NO_MODS,
    label: "Tab",
};

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------
pub const RENAME_SAVE: Chord = Chord {
    codes: &[KeyCode::Enter],
    mods: NO_MODS,
    label: "Enter",
};
pub const RENAME_CANCEL: Chord = Chord {
    codes: &[KeyCode::Esc],
    mods: NO_MODS,
    label: "Esc",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    type Named = (&'static str, &'static Chord);

    const GLOBAL_CHORDS: &[Named] = &[("PREFIX", &PREFIX)];

    const PREFIX_CHORDS: &[Named] = &[
        ("PREFIX_QUIT", &PREFIX_QUIT),
        ("PREFIX_SPAWN", &PREFIX_SPAWN),
        ("PREFIX_DETACH", &PREFIX_DETACH),
        ("PREFIX_TOGGLE_SIDEBAR", &PREFIX_TOGGLE_SIDEBAR),
        ("PREFIX_RENAME", &PREFIX_RENAME),
        ("PREFIX_PICKER", &PREFIX_PICKER),
        ("PREFIX_SEND_CTRL_A", &PREFIX_SEND_CTRL_A),
        ("PREFIX_SCROLLBACK", &PREFIX_SCROLLBACK),
        ("PREFIX_HELP", &PREFIX_HELP),
        ("PREFIX_REORDER", &PREFIX_REORDER),
        ("PREFIX_FOCUS_NEXT", &PREFIX_FOCUS_NEXT),
        ("PREFIX_FOCUS_PREV", &PREFIX_FOCUS_PREV),
    ];

    const SCROLLBACK_CHORDS: &[Named] = &[
        ("SCROLLBACK_UP", &SCROLLBACK_UP),
        ("SCROLLBACK_DOWN", &SCROLLBACK_DOWN),
        ("SCROLLBACK_PAGE_UP", &SCROLLBACK_PAGE_UP),
        ("SCROLLBACK_PAGE_DOWN", &SCROLLBACK_PAGE_DOWN),
        ("SCROLLBACK_TOP", &SCROLLBACK_TOP),
        ("SCROLLBACK_BOTTOM", &SCROLLBACK_BOTTOM),
        ("SCROLLBACK_EXIT", &SCROLLBACK_EXIT),
    ];

    const REORDER_CHORDS: &[Named] = &[
        ("REORDER_UP", &REORDER_UP),
        ("REORDER_DOWN", &REORDER_DOWN),
        ("REORDER_EXIT", &REORDER_EXIT),
    ];

    const CONFIRM_CHORDS: &[Named] = &[("CONFIRM_YES", &CONFIRM_YES), ("CONFIRM_NO", &CONFIRM_NO)];

    const SPAWN_CHORDS: &[Named] = &[
        ("SPAWN_UP", &SPAWN_UP),
        ("SPAWN_DOWN", &SPAWN_DOWN),
        ("SPAWN_PGUP", &SPAWN_PGUP),
        ("SPAWN_PGDOWN", &SPAWN_PGDOWN),
        ("SPAWN_HOME", &SPAWN_HOME),
        ("SPAWN_END", &SPAWN_END),
        ("SPAWN_DESCEND", &SPAWN_DESCEND),
        ("SPAWN_ASCEND", &SPAWN_ASCEND),
        ("SPAWN_PICK", &SPAWN_PICK),
        ("SPAWN_CANCEL", &SPAWN_CANCEL),
        ("SPAWN_TOGGLE_DANGER", &SPAWN_TOGGLE_DANGER),
    ];

    const PICKER_CHORDS: &[Named] = &[
        ("PICKER_UP", &PICKER_UP),
        ("PICKER_DOWN", &PICKER_DOWN),
        ("PICKER_PGUP", &PICKER_PGUP),
        ("PICKER_PGDOWN", &PICKER_PGDOWN),
        ("PICKER_HOME", &PICKER_HOME),
        ("PICKER_END", &PICKER_END),
        ("PICKER_PICK", &PICKER_PICK),
        ("PICKER_CANCEL", &PICKER_CANCEL),
        ("PICKER_FILTER_CLEAR", &PICKER_FILTER_CLEAR),
        ("PICKER_TOGGLE_DANGER", &PICKER_TOGGLE_DANGER),
    ];

    const RENAME_CHORDS: &[Named] = &[
        ("RENAME_SAVE", &RENAME_SAVE),
        ("RENAME_CANCEL", &RENAME_CANCEL),
    ];

    /// One dispatch context per entry: `main.rs` consults exactly one of these
    /// lists for a given key, so collisions only matter within a list.
    const MODES: &[(&str, &[Named])] = &[
        ("prefix", PREFIX_CHORDS),
        ("scrollback", SCROLLBACK_CHORDS),
        ("reorder", REORDER_CHORDS),
        ("confirm-detach", CONFIRM_CHORDS),
        ("spawn", SPAWN_CHORDS),
        ("picker", PICKER_CHORDS),
        ("rename", RENAME_CHORDS),
    ];

    /// `PREFIX_FOCUS_NEXT` / `PREFIX_FOCUS_PREV` are drawn in the help popup
    /// from a literal "↑↓" row, so their constants are never named there.
    const HELP_LITERAL_LABEL_CHORDS: &[&str] = &["PREFIX_FOCUS_NEXT", "PREFIX_FOCUS_PREV"];

    fn all_chords() -> Vec<Named> {
        let mut v: Vec<Named> = GLOBAL_CHORDS.to_vec();
        for (_, group) in MODES {
            v.extend_from_slice(group);
        }
        v
    }

    /// Names of every `pub const … : Chord` declared in this file.
    fn declared_chord_names() -> Vec<&'static str> {
        include_str!("keys.rs")
            .lines()
            .filter_map(|l| l.strip_prefix("pub const "))
            .filter_map(|l| l.split_once(": Chord"))
            .map(|(name, _)| name)
            .collect()
    }

    /// How a non-character key is expected to be spelled in a label.
    fn label_token(code: KeyCode) -> Option<&'static str> {
        match code {
            KeyCode::Up => Some("↑"),
            KeyCode::Down => Some("↓"),
            KeyCode::Left => Some("←"),
            KeyCode::Right => Some("→"),
            KeyCode::Enter => Some("enter"),
            KeyCode::Esc => Some("esc"),
            KeyCode::Home => Some("home"),
            KeyCode::End => Some("end"),
            KeyCode::PageUp => Some("pgup"),
            KeyCode::PageDown => Some("pgdn"),
            KeyCode::Tab => Some("tab"),
            KeyCode::Backspace => Some("backspace"),
            _ => None,
        }
    }

    #[test]
    fn the_registry_lists_every_chord_declared_in_this_file() {
        let declared = declared_chord_names();
        let registered: Vec<&str> = all_chords().iter().map(|(n, _)| *n).collect();
        for name in &declared {
            assert!(
                registered.contains(name),
                "{name} is declared in keys.rs but no group in this test module lists it, \
                 so the collision, prefix-only and help-coverage gates never see it"
            );
        }
        assert_eq!(
            registered.len(),
            declared.len(),
            "registry holds {registered:?} for declared chords {declared:?}"
        );
    }

    #[test]
    fn no_two_chords_in_one_mode_answer_the_same_key() {
        for (mode, group) in MODES {
            for (name, chord) in *group {
                for code in chord.codes {
                    let key = ev(*code, chord.mods);
                    let hits: Vec<&str> = group
                        .iter()
                        .filter(|(_, c)| c.matches(&key))
                        .map(|(n, _)| *n)
                        .collect();
                    assert_eq!(
                        hits,
                        vec![*name],
                        "in {mode} mode {code:?} is answered by {hits:?}; \
                         a key must reach exactly one chord"
                    );
                }
            }
        }
    }

    #[test]
    fn the_prefix_is_the_only_chord_that_carries_a_modifier() {
        for (mode, group) in MODES {
            for (name, chord) in *group {
                assert!(
                    chord.mods.is_empty(),
                    "{name} in {mode} mode requires {:?}; a modifier makes it reachable \
                     without the prefix, and every command must go through {}",
                    chord.mods,
                    PREFIX.label
                );
            }
        }
        let globals: Vec<&str> = GLOBAL_CHORDS.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            globals,
            vec!["PREFIX"],
            "the global chords are {globals:?}; {} must stay the only one",
            PREFIX.label
        );
    }

    #[test]
    fn ctrl_q_is_not_a_binding_and_reaches_the_focused_session() {
        let ctrl_q = ev(KeyCode::Char('q'), CTRL);
        for (name, chord) in GLOBAL_CHORDS {
            assert!(
                !chord.matches(&ctrl_q),
                "{name} answers Ctrl+Q; quit is reachable only as {} then {}",
                PREFIX.label,
                PREFIX_QUIT.label
            );
        }
        assert_eq!(
            encode(ctrl_q),
            Some(vec![0x11]),
            "Ctrl+Q must pass through to the session as byte 0x11"
        );
    }

    #[test]
    fn the_help_popup_lists_every_prefix_chord() {
        let help = include_str!("ui/popups/help.rs");
        for (name, _) in PREFIX_CHORDS {
            if HELP_LITERAL_LABEL_CHORDS.contains(name) {
                continue;
            }
            let needle = format!("keys::{name}.label");
            assert!(
                help.contains(&needle),
                "the {} popup never reads {needle}, so {name} is undiscoverable",
                PREFIX_HELP.label
            );
        }
    }

    #[test]
    fn every_chord_label_names_a_key_the_chord_matches() {
        for (name, chord) in all_chords() {
            let label = chord.label.to_lowercase();
            assert!(!label.is_empty(), "{name} has an empty label");
            let named = chord.codes.iter().any(|code| match code {
                KeyCode::Char(' ') => label.contains("space"),
                KeyCode::Char(c) => label.contains(c.to_lowercase().next().unwrap_or(*c)),
                other => label_token(*other).is_some_and(|t| label.contains(t)),
            });
            assert!(
                named,
                "{name} is labelled {:?} but that names none of its keys {:?}; \
                 the UI would advertise a key the chord does not answer",
                chord.label, chord.codes
            );
        }
    }

    #[test]
    fn prefix_chords_answer_both_letter_cases() {
        for (name, chord) in GLOBAL_CHORDS.iter().chain(PREFIX_CHORDS.iter()) {
            for code in chord.codes {
                let KeyCode::Char(c) = code else { continue };
                if !c.is_ascii_alphabetic() {
                    continue;
                }
                let flipped = if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                };
                assert!(
                    chord.codes.contains(&KeyCode::Char(flipped)),
                    "{name} lists {c:?} but not {flipped:?}; a held Shift or Caps Lock \
                     would drop the chord"
                );
            }
        }
    }

    #[test]
    fn the_prefix_matches_ctrl_a_in_either_case() {
        assert!(
            PREFIX.matches(&ev(KeyCode::Char('a'), CTRL)),
            "Ctrl+A must open the prefix"
        );
        assert!(
            PREFIX.matches(&ev(KeyCode::Char('A'), CTRL | KeyModifiers::SHIFT)),
            "Ctrl+Shift+A must open the prefix; crossterm reports the shifted char"
        );
    }

    #[test]
    fn the_prefix_does_not_match_a_bare_a_or_another_ctrl_letter() {
        assert!(
            !PREFIX.matches(&ev(KeyCode::Char('a'), NO_MODS)),
            "a bare 'a' must be typed into the session"
        );
        assert!(
            !PREFIX.matches(&ev(KeyCode::Char('b'), CTRL)),
            "Ctrl+B is not the prefix"
        );
    }

    #[test]
    fn a_chord_requires_its_modifiers_and_ignores_the_rest() {
        assert!(
            PREFIX.matches(&ev(KeyCode::Char('a'), CTRL | KeyModifiers::ALT)),
            "a held Alt must not break the prefix"
        );
        assert!(
            PREFIX_QUIT.matches(&ev(KeyCode::Char('q'), KeyModifiers::SHIFT)),
            "a NO_MODS chord must survive a held Shift"
        );
        assert!(
            PREFIX_QUIT.matches(&ev(KeyCode::Char('q'), CTRL)),
            "NO_MODS is a subset test, so a NO_MODS chord answers the key with Ctrl held too"
        );
    }

    #[test]
    fn a_chord_ignores_a_key_outside_its_code_list() {
        assert!(
            !PREFIX_QUIT.matches(&ev(KeyCode::Char('x'), NO_MODS)),
            "'x' is not a quit key"
        );
        assert!(
            !SCROLLBACK_EXIT.matches(&ev(KeyCode::Char('j'), NO_MODS)),
            "'j' scrolls, it must not leave scrollback"
        );
    }

    #[test]
    fn encode_maps_ctrl_letters_to_their_control_bytes() {
        assert_eq!(
            encode(ev(KeyCode::Char('a'), CTRL)),
            Some(vec![0x01]),
            "Ctrl+A"
        );
        assert_eq!(
            encode(ev(KeyCode::Char('z'), CTRL)),
            Some(vec![0x1a]),
            "Ctrl+Z"
        );
        assert_eq!(
            encode(ev(KeyCode::Char('C'), CTRL | KeyModifiers::SHIFT)),
            Some(vec![0x03]),
            "Ctrl+Shift+C encodes like Ctrl+C"
        );
    }

    #[test]
    fn encode_maps_ctrl_punctuation_and_ctrl_space() {
        let cases: &[(char, u8)] = &[
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('?', 0x7f),
        ];
        for (c, want) in cases {
            assert_eq!(
                encode(ev(KeyCode::Char(*c), CTRL)),
                Some(vec![*want]),
                "Ctrl+{c:?} must encode to {want:#04x}"
            );
        }
    }

    #[test]
    fn encode_writes_plain_characters_as_utf8() {
        assert_eq!(
            encode(ev(KeyCode::Char('x'), NO_MODS)).as_deref(),
            Some(&b"x"[..]),
            "a plain char is its own byte"
        );
        assert_eq!(
            encode(ev(KeyCode::Char('x'), KeyModifiers::SHIFT)).as_deref(),
            Some(&b"X"[..]),
            "Shift uppercases the char"
        );
        assert_eq!(
            encode(ev(KeyCode::Char('é'), NO_MODS)).as_deref(),
            Some("é".as_bytes()),
            "a multibyte char keeps all its bytes"
        );
    }

    #[test]
    fn encode_maps_named_keys_to_their_terminal_sequences() {
        let cases: &[(KeyCode, &[u8])] = &[
            (KeyCode::Enter, b"\r"),
            (KeyCode::Tab, b"\t"),
            (KeyCode::BackTab, b"\x1b[Z"),
            (KeyCode::Backspace, &[0x7f]),
            (KeyCode::Esc, &[0x1b]),
            (KeyCode::Left, b"\x1b[D"),
            (KeyCode::Right, b"\x1b[C"),
            (KeyCode::Up, b"\x1b[A"),
            (KeyCode::Down, b"\x1b[B"),
            (KeyCode::Home, b"\x1b[H"),
            (KeyCode::End, b"\x1b[F"),
            (KeyCode::PageUp, b"\x1b[5~"),
            (KeyCode::PageDown, b"\x1b[6~"),
            (KeyCode::Insert, b"\x1b[2~"),
            (KeyCode::Delete, b"\x1b[3~"),
        ];
        for (code, want) in cases {
            assert_eq!(
                encode(ev(*code, NO_MODS)).as_deref(),
                Some(*want),
                "{code:?} encodes to the wrong sequence"
            );
        }
    }

    #[test]
    fn encode_maps_function_keys_and_stops_after_f12() {
        assert_eq!(
            encode(ev(KeyCode::F(1), NO_MODS)).as_deref(),
            Some(&b"\x1bOP"[..]),
            "F1 uses the SS3 form"
        );
        assert_eq!(
            encode(ev(KeyCode::F(5), NO_MODS)).as_deref(),
            Some(&b"\x1b[15~"[..]),
            "F5 switches to the CSI form"
        );
        assert_eq!(
            encode(ev(KeyCode::F(12), NO_MODS)).as_deref(),
            Some(&b"\x1b[24~"[..]),
            "F12 is the last mapped function key"
        );
        assert_eq!(
            encode(ev(KeyCode::F(13), NO_MODS)),
            None,
            "F13 has no sequence"
        );
    }

    #[test]
    fn encode_prefixes_alt_chords_with_escape() {
        assert_eq!(
            encode(ev(KeyCode::Char('b'), KeyModifiers::ALT)).as_deref(),
            Some(&b"\x1bb"[..]),
            "Alt+b is ESC then b"
        );
        assert_eq!(
            encode(ev(KeyCode::Char('b'), KeyModifiers::ALT | CTRL)),
            Some(vec![0x1b, 0x02]),
            "Alt+Ctrl+b is ESC then the control byte"
        );
        assert_eq!(
            encode(ev(KeyCode::Left, KeyModifiers::ALT)).as_deref(),
            Some(&b"\x1b\x1b[D"[..]),
            "Alt+Left is ESC then the arrow sequence"
        );
    }

    #[test]
    fn encode_returns_none_for_keys_with_no_sequence() {
        assert_eq!(
            encode(ev(KeyCode::Char('1'), CTRL)),
            None,
            "Ctrl+digit has no control byte"
        );
        assert_eq!(
            encode(ev(KeyCode::Null, NO_MODS)),
            None,
            "Null is not typeable"
        );
        assert_eq!(
            encode(ev(KeyCode::CapsLock, NO_MODS)),
            None,
            "a modifier-only key sends nothing"
        );
        assert_eq!(
            encode(ev(KeyCode::Null, KeyModifiers::ALT)),
            None,
            "Alt must not leak a bare escape when the key itself is unmapped"
        );
    }

    #[test]
    fn the_send_literal_chord_agrees_with_the_encoded_prefix_key() {
        assert_eq!(
            encode(ev(KeyCode::Char('a'), CTRL)),
            Some(vec![0x01]),
            "main.rs writes a literal 0x01 for {}, so the prefix key must encode to the same byte",
            PREFIX_SEND_CTRL_A.label
        );
    }
}
