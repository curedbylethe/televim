//! Stable release offer overlay.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::state::update::{Choice, Stage};

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let stage = app.ui.update.stage();
    let (Stage::Available {
        installed,
        available,
        offered: true,
    }
    | Stage::Failed {
        installed,
        available,
        offered: true,
        ..
    }
    | Stage::Applying {
        installed,
        available,
        ..
    }
    | Stage::Installed {
        installed,
        available,
        ..
    }) = stage
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
    let title = match stage {
        Stage::Applying { .. } => " Updating ",
        Stage::Failed { .. } => " Update failed ",
        Stage::Installed { .. } => " Update installed ",
        _ => " Update available ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.ui.theme.border)
        .title(Span::styled(title, app.ui.theme.text));
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
    render_details(app, stage, installed, available, frame, &rows);
}

fn render_details(
    app: &App,
    stage: &Stage,
    installed: &str,
    available: &str,
    frame: &mut Frame<'_>,
    rows: &[Rect],
) {
    let text = app.ui.theme.text;
    let dim = app.ui.theme.text_dim;
    let headline = match stage {
        Stage::Applying { progress, .. } => match progress {
            crate::state::update::Progress::Downloading => "downloading",
            crate::state::update::Progress::Verifying => "verifying",
            crate::state::update::Progress::Extracting => "extracting",
            crate::state::update::Progress::Checking => "checking",
        },
        Stage::Failed { .. } => "Update could not be staged.",
        Stage::Installed { .. } => "",
        _ => "A newer stable release is available.",
    };
    let headline = if matches!(stage, Stage::Installed { .. }) {
        format!("televim {available} is installed.")
    } else {
        headline.to_owned()
    };
    frame.render_widget(Paragraph::new(headline).style(text), rows[0]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("Installed  {installed}"), dim),
            Span::styled("    ", text),
            Span::styled(format!("Available  {available}"), text),
        ])),
        rows[2],
    );
    match stage {
        Stage::Applying { .. } => {
            frame.render_widget(
                Paragraph::new("Esc: cancel after current step").style(dim),
                rows[5],
            );
        }
        Stage::Failed { reason, .. } => {
            frame.render_widget(Paragraph::new(reason.as_str()).style(text), rows[4]);
        }
        Stage::Installed { .. } => {
            frame.render_widget(
                Paragraph::new(format!(
                    "Restart televim to use {available}. This window keeps running {installed}."
                ))
                .style(text),
                rows[4],
            );
        }
        _ => {
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
        }
    }
    if matches!(stage, Stage::Applying { .. }) {
        return;
    }
    let first = action(
        if matches!(stage, Stage::Installed { .. }) {
            "[ Restart ]"
        } else {
            "[ Update ]"
        },
        app.ui.update.selected() == Choice::Update,
        app,
    );
    let later = action("[ Later ]", app.ui.update.selected() == Choice::Later, app);
    frame.render_widget(
        Paragraph::new(Line::from(vec![first, Span::raw("  "), later]))
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
