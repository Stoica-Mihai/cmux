//! TUI rendering. Thin dispatch; per-area drawing lives in submodules.
//!
//! - [`chrome`]: titlebar, footer, toast — chrome around the body.
//! - [`dashboard`]: main tile grid + sidebar.
//! - [`popups`]: every modal overlay.
//! - [`widgets`]: chip helpers, popup frames, text utilities.

mod chrome;
mod dashboard;
pub use dashboard::SIDEBAR_W;
mod popups;
mod widgets;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};

pub use widgets::MARQUEE_STEP_MS;

pub type TileSizes = Vec<(usize, u16, u16)>;

pub fn draw(f: &mut Frame, app: &mut App, tile_sizes: &mut TileSizes) {
    tile_sizes.clear();
    let area = f.area();
    app.last_tile_area = None;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let titlebar = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    chrome::draw_titlebar(f, app, titlebar);
    dashboard::draw_dashboard(f, app, body, tile_sizes);
    f.render_widget(Paragraph::new(chrome::footer_for(app)), footer);

    match &app.mode {
        Mode::Spawn(s) => popups::spawn::draw(f, area, s),
        Mode::Rename(s) => popups::rename::draw(f, area, s, app),
        Mode::Picker(s) => popups::picker::draw(f, area, s),
        Mode::ConfirmDetach(id) => popups::confirm_detach::draw(f, area, *id, app),
        Mode::Help => popups::help::draw(f, area),
        Mode::Dashboard | Mode::Scrollback(_) | Mode::Reorder => {}
    }

    if let (Some(tile), Some(toast)) = (app.last_tile_area, &app.toast) {
        chrome::draw_toast(f, tile, &toast.text);
    }

    if app.daemon_lost {
        popups::daemon_lost::draw(f, area);
    }
}

#[cfg(test)]
#[path = "../tests/ui.rs"]
mod tests;
