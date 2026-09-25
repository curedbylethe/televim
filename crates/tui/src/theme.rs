//! Colour scheme.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub border: Style,
    pub border_focused: Style,
    pub text: Style,
    pub text_dim: Style,
    pub selection: Style,
    pub mode_normal: Style,
    pub mode_insert: Style,
    pub mode_visual: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            border: Style::default().fg(Color::DarkGray),
            border_focused: Style::default().fg(Color::Cyan),
            text: Style::default().fg(Color::White),
            text_dim: Style::default().fg(Color::Gray),
            selection: Style::default().add_modifier(Modifier::REVERSED),
            mode_normal: Style::default().bg(Color::Blue).fg(Color::White),
            mode_insert: Style::default().bg(Color::Green).fg(Color::Black),
            mode_visual: Style::default().bg(Color::Yellow).fg(Color::Black),
        }
    }
}
