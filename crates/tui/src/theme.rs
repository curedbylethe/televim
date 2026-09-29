//! Colour scheme.
//!
//! # The order styles compose in
//!
//! Four of them can land on one cell — the cursor's reverse video, a selection's
//! background, a search match's colour and the plain text under all three — and
//! they are applied in this order, each `patch`ed over the last:
//!
//! 1. `text` — the baseline everything starts from;
//! 2. `match_hit` — a row a search found;
//! 3. `selection_bg` — a slice of a row a selection covers;
//! 4. `selection` — the cursor's `REVERSED` row, drawn by the list widget.
//!
//! Later wins, because `Style::patch` takes the other style's fields wherever the
//! other style sets one. Two consequences worth stating rather than leaving to be
//! discovered:
//!
//! - `selection_bg` never uses `REVERSED`, and neither does `match_hit`. The
//!   cursor owns reverse video; a second user of it would leave the reader unable
//!   to tell which row the cursor is on.
//! - `selection_bg` is a background and `match_hit` a foreground, so a cell that
//!   is both is legible rather than one of them winning outright.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub border: Style,
    pub border_focused: Style,
    pub text: Style,
    pub text_dim: Style,
    pub selection: Style,
    /// A row a search matched.
    ///
    /// Bold and a colour of its own rather than `selection`'s `REVERSED`: the
    /// cursor can stand on a match, and the two styles are composed for that
    /// row, so this has to stay legible underneath the reverse video.
    pub match_hit: Style,
    /// A slice of a row a selection covers.
    ///
    /// A background rather than a foreground, for the same reason
    /// [`Theme::match_hit`] is a foreground: the cursor can stand inside a
    /// selection, and the two have to compose for that cell. A background is what
    /// leaves a match's colour legible on top of it.
    pub selection_bg: Style,
    pub mode_normal: Style,
    pub mode_insert: Style,
    pub mode_visual: Style,
    pub mode_confirm: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            border: Style::default().fg(Color::DarkGray),
            border_focused: Style::default().fg(Color::Cyan),
            text: Style::default().fg(Color::White),
            text_dim: Style::default().fg(Color::Gray),
            selection: Style::default().add_modifier(Modifier::REVERSED),
            match_hit: Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            selection_bg: Style::default().bg(Color::Magenta),
            mode_normal: Style::default().bg(Color::Blue).fg(Color::White),
            mode_insert: Style::default().bg(Color::Green).fg(Color::Black),
            mode_visual: Style::default().bg(Color::Yellow).fg(Color::Black),
            mode_confirm: Style::default().bg(Color::Red).fg(Color::White),
        }
    }
}
