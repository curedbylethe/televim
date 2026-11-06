//! Stable release offer overlay.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::state::update::{Choice, Stage};

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let Stage::Available {
        installed,
        available,
        offered: true,
    } = app.ui.update.stage()
    else {
        return;
    };
    let width = 53.min(area.width);
    let height = 13.min(area.height);
    if width < 20 || height < 7 {
        return;
    }
    let dialog = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, dialog);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.ui.theme.border)
        .title(Span::styled(" Update available ", app.ui.theme.text));
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
    let text = app.ui.theme.text;
    let dim = app.ui.theme.text_dim;
    frame.render_widget(
        Paragraph::new("A newer stable release is available.").style(text),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("Installed  {installed}"), dim),
            Span::styled("    ", text),
            Span::styled(format!("Available  {available}"), text),
        ])),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new("Update downloads the verified archive and").style(text),
        rows[4],
    );
    frame.render_widget(
        Paragraph::new("replaces the binary. This window keeps").style(text),
        rows[5],
    );
    frame.render_widget(
        Paragraph::new(format!("running {installed} until you restart.")).style(text),
        rows[6],
    );
    let update = action(
        "[ Update ]",
        app.ui.update.selected() == Choice::Update,
        app,
    );
    let later = action("[ Later ]", app.ui.update.selected() == Choice::Later, app);
    frame.render_widget(
        Paragraph::new(Line::from(vec![update, Span::raw("  "), later]))
            .alignment(Alignment::Center),
        rows[7],
    );
    frame.render_widget(
        Paragraph::new("h/l: choose  ⏎: confirm  Esc: later")
            .style(dim)
            .alignment(Alignment::Center),
        rows[8],
    );
}

fn action<'a>(label: &'a str, selected: bool, app: &App) -> Span<'a> {
    Span::styled(
        label,
        if selected {
            app.ui.theme.selection.add_modifier(Modifier::REVERSED)
        } else {
            app.ui.theme.text
        },
    )
}
