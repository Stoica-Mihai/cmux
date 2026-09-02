//! Modal overlays. Each submodule owns one popup. Anything reused across
//! popups (the dangerous-flag toggle row shared by spawn + picker) lives in
//! [`dangerous`].

pub(super) mod confirm_detach;
pub(super) mod daemon_lost;
pub(super) mod dangerous;
pub(super) mod help;
pub(super) mod picker;
pub(super) mod rename;
pub(super) mod spawn;

/// Render-into-a-`TestBackend` helpers shared by every ui test module.
#[cfg(test)]
pub(in crate::ui) mod harness {
    use ratatui::Frame;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Position, Rect};
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;
    use std::sync::mpsc;

    use crate::app::App;
    use crate::session::Session;

    /// Left and right rounded caps emitted by `widgets::kbd_chip` and
    /// `widgets::action_chip`.
    pub(in crate::ui) const CAP_LEFT: &str = "\u{E0B6}";
    pub(in crate::ui) const CAP_RIGHT: &str = "\u{E0B4}";

    /// Draw one frame into a `w` by `h` test terminal and return its buffer.
    pub(in crate::ui) fn render(w: u16, h: u16, body: impl FnOnce(&mut Frame)) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
        term.draw(body).expect("draw");
        term.backend().buffer().clone()
    }

    /// Draw one frame, turning a panic inside the render into an `Err` so the
    /// caller can name the size that broke it.
    pub(in crate::ui) fn try_render(
        w: u16,
        h: u16,
        body: impl FnOnce(&mut Frame),
    ) -> Result<Buffer, String> {
        let guarded = std::panic::AssertUnwindSafe(|| render(w, h, body));
        std::panic::catch_unwind(guarded).map_err(|e| {
            e.downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panicked with a non-string payload".to_string())
        })
    }

    /// Row `y` of the buffer as a string.
    pub(in crate::ui) fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// Every row of the buffer, newline-joined.
    pub(in crate::ui) fn text(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| row(buf, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Contents of every chip in the buffer, one entry per cap-delimited run.
    pub(in crate::ui) fn chips(buf: &Buffer) -> Vec<String> {
        let mut out = Vec::new();
        for y in 0..buf.area.height {
            let line = row(buf, y);
            for after_open in line.split(CAP_LEFT).skip(1) {
                if let Some(inner) = after_open.split(CAP_RIGHT).next() {
                    out.push(inner.to_string());
                }
            }
        }
        out
    }

    /// Smallest rect covering every painted cell, or `None` if nothing drew.
    pub(in crate::ui) fn painted_bounds(buf: &Buffer) -> Option<Rect> {
        let mut bounds: Option<(u16, u16, u16, u16)> = None;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if !is_painted(&buf[(x, y)]) {
                    continue;
                }
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
        bounds.map(|(x0, y0, x1, y1)| Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }

    fn is_painted(cell: &ratatui::buffer::Cell) -> bool {
        cell.symbol() != " "
            || cell.fg != Color::Reset
            || cell.bg != Color::Reset
            || cell.modifier != Modifier::empty()
    }

    /// An `App` holding one daemon-backed session per label, ids from 1.
    pub(in crate::ui) fn app_with(labels: &[&str]) -> App {
        let mut app = App::new(PathBuf::from("/tmp"), (24, 80));
        for (i, label) in labels.iter().enumerate() {
            let (tx, _rx) = mpsc::channel();
            let id = i as u64 + 1;
            let (sess, _slot) = Session::new_daemon(
                id,
                (*label).to_string(),
                PathBuf::from("/tmp/project"),
                false,
                None,
                24,
                80,
                None,
                id,
                tx,
            );
            app.sessions.push(sess);
        }
        app.next_id = labels.len() as u64 + 1;
        app
    }

    /// Assert nothing was painted outside `inside`, naming the first strays.
    pub(in crate::ui) fn assert_inside(buf: &Buffer, inside: Rect, what: &str) {
        let mut stray: Vec<String> = Vec::new();
        let mut total = 0usize;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if inside.contains(Position::new(x, y)) || !is_painted(&buf[(x, y)]) {
                    continue;
                }
                total += 1;
                if stray.len() < 8 {
                    stray.push(format!("({x},{y}) {:?}", buf[(x, y)].symbol()));
                }
            }
        }
        assert!(
            total == 0,
            "{what}: {total} cells painted outside {inside:?}, first are {}",
            stray.join(", ")
        );
    }

    /// Assert every glyph-bearing cell has a foreground unlike its background.
    /// Chip caps are exempt: they are drawn in the chip's own colour so the
    /// rounded edge blends into whatever panel sits behind it.
    pub(in crate::ui) fn assert_legible(buf: &Buffer, what: &str) {
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                if sym.trim().is_empty() || sym == CAP_LEFT || sym == CAP_RIGHT {
                    continue;
                }
                if cell.fg == Color::Reset && cell.bg == Color::Reset {
                    continue;
                }
                assert_ne!(
                    cell.fg, cell.bg,
                    "{what}: cell ({x},{y}) draws {sym:?} in its own background colour"
                );
            }
        }
    }
}
