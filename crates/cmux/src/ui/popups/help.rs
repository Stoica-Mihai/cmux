//! `Ctrl+A ?` — the cheat sheet.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::keys;
use crate::theme;

use crate::ui::widgets::{kbd_chip, open_popup};

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(4).clamp(64, 80);
    let h = area.height.saturating_sub(2).clamp(22, 30);
    let inner = open_popup(f, area, w, h, " ⌘ cmux keys ", theme::ACCENT_YELLOW);

    let header = |s: &str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default()
                .fg(theme::ACCENT_YELLOW)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |chord: &str, desc: &str| {
        let chord_chip = kbd_chip(chord);
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        spans.extend(chord_chip);
        let chord_width: usize = chord.chars().count() + 4;
        let pad = 12usize.saturating_sub(chord_width);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(
            format!(" {}", desc),
            Style::default().fg(theme::FG),
        ));
        Line::from(spans)
    };
    let note = |s: &str| {
        Line::from(Span::styled(
            format!("  {}", s),
            Style::default().fg(theme::FG_DIM),
        ))
    };

    let lines: Vec<Line> = vec![
        header(&format!(" Prefix chords  ({} then…)", keys::PREFIX.label)),
        row(keys::PREFIX_SPAWN.label, "spawn new claude in a folder"),
        row(keys::PREFIX_PICKER.label, "resume picker (past sessions)"),
        row(keys::PREFIX_RENAME.label, "rename focused session"),
        row(keys::PREFIX_DETACH.label, "detach focused (with confirm)"),
        row(keys::PREFIX_SCROLLBACK.label, "enter scrollback mode"),
        row(
            keys::PREFIX_REORDER.label,
            "enter reorder mode (move sessions)",
        ),
        row("↑↓", "cycle focused session"),
        row("1-9", "jump to session N"),
        row(keys::PREFIX_TOGGLE_SIDEBAR.label, "toggle sidebar"),
        row(
            keys::PREFIX_SEND_CTRL_A.label,
            "send literal Ctrl+A to focused claude",
        ),
        row(keys::PREFIX_HELP.label, "this help"),
        row(
            keys::PREFIX_QUIT.label,
            "quit (daemon sessions keep running)",
        ),
        Line::from(""),
        header(" Mouse"),
        row("drag", "copy selection via OSC 52"),
        row("⇧+drag", "bypass cmux, outer terminal selection"),
        Line::from(""),
        header(" Sidebar badges"),
        note("⠋ green  busy (claude working)"),
        note(&format!(
            "{} cyan   idle (claude waiting for input)",
            theme::glyph::IDLE
        )),
        note(&format!(
            "{} red    permission prompt waiting",
            theme::glyph::PERMISSION
        )),
        note(&format!(
            "{} gray   dormant (idle over 30s)",
            theme::glyph::IDLE
        )),
        note(&format!("{} red    session exited", theme::glyph::EXITED)),
        note(&format!("{} cyan   resumed session", theme::glyph::RESUMED)),
        Line::from(""),
        note("press any key to close"),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}
