//! Single-line status bar showing mode + message.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, ConnectionState, Mode};
use crate::widgets::input_bar;

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    // The label is the mode, and it belongs to whichever pane the key is going
    // to — so it is the line's own when the line has the focus, because a line
    // has a mode of its own and being on it is no longer the whole of what it is
    // doing. A confirmation is a question about the whole screen. Which *pane*
    // has the focus is the border's to say, not this row's.
    let label = input_bar::mode_label(app);

    let style = match (app.ui.focus == crate::app::Focus::Input, app.ui.mode) {
        // Named rather than wildcarded, for the same reason `mode_label` names
        // them: a mode that only the conversation can be in should say so.
        (false, Mode::Visual) => app.ui.theme.mode_visual,
        (false, Mode::Confirm) => app.ui.theme.mode_confirm,
        // The line's insert is the one mode that is not the conversation's, and
        // it is the one the bar's border is on as well — a reader who cannot
        // see where the caret is should at least be able to see which mode the
        // keys they are about to press will mean.
        (true, Mode::Normal) if label != Mode::Normal.label() => app.ui.theme.mode_insert,
        _ => app.ui.theme.mode_normal,
    };

    // The connection dot: always drawn, beside the ranked sentence rather
    // than in it, so no rank can hide it. Green while the feed delivers,
    // yellow while a bring-up or a rebuild is under way, red once the budget
    // is spent. The sentence beside it still says what happened in words.
    let connection = match app.connection() {
        ConnectionState::Connected => app.ui.theme.conn_connected,
        ConnectionState::Connecting | ConnectionState::Reconnecting => app.ui.theme.conn_transient,
        ConnectionState::Offline => app.ui.theme.conn_offline,
    };

    let line = Line::from(vec![
        Span::styled(format!(" {label} "), style),
        Span::raw(" "),
        Span::styled("● ", connection),
        Span::styled(app.status_text(), app.ui.theme.text_dim),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{FetchDirection, REVALIDATING_LABEL};
    use domain::message::Message;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    /// The whole frame, drawn into an in-memory terminal at the design's size.
    fn screen(app: &App) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");
        terminal.backend().buffer().clone()
    }

    /// The status bar is the frame's last row.
    fn bottom_row(buffer: &Buffer) -> (u16, String) {
        let y = buffer.area.height - 1;
        let row = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_owned();
        (y, row)
    }

    /// The dot's ink on the status bar, or a panic saying what was drawn.
    fn dot_fg(buffer: &Buffer) -> Color {
        let (y, row) = bottom_row(buffer);
        let x = (0..buffer.area.width)
            .find(|&x| buffer[(x, y)].symbol() == "●")
            .unwrap_or_else(|| panic!("no dot on the status bar: {row:?}"));
        buffer[(x, y)].fg
    }

    fn conn_fg(app: &App) -> Color {
        dot_fg(&screen(app))
    }

    /// Connected: the dot is drawn, in the connected ink.
    #[test]
    fn the_dot_is_green_while_the_feed_delivers() {
        let mut app = App::mock();
        app.set_connection(ConnectionState::Connected);

        let expected = app
            .ui
            .theme
            .conn_connected
            .fg
            .expect("the dot is a foreground");
        assert_eq!(conn_fg(&app), expected, "connected is green");
    }

    /// Connecting and reconnecting share one yellow: both are the same wait.
    #[test]
    fn the_dot_is_yellow_while_nothing_has_answered() {
        let expected = App::mock()
            .ui
            .theme
            .conn_transient
            .fg
            .expect("the dot is a foreground");
        for state in [ConnectionState::Connecting, ConnectionState::Reconnecting] {
            let mut app = App::mock();
            app.set_connection(state);
            assert_eq!(conn_fg(&app), expected, "{state:?} is yellow");
        }
    }

    /// Offline: the dot is drawn, in the offline ink — the sentence beside it
    /// still says what happened.
    #[test]
    fn the_dot_is_red_once_the_budget_is_spent() {
        let mut app = App::mock();
        app.set_connection(ConnectionState::Offline);
        app.flash("offline: the feed ended");

        let expected = app
            .ui
            .theme
            .conn_offline
            .fg
            .expect("the dot is a foreground");
        let buffer = screen(&app);
        assert_eq!(dot_fg(&buffer), expected, "offline is red");
        let (_, row) = bottom_row(&buffer);
        assert!(
            row.contains("offline: the feed ended"),
            "the ranked sentence is still beside it: {row:?}"
        );
    }

    /// A conversation drawn from the cache says on the status line that its
    /// newest page is on its way, and a failure written there talks over it.
    #[test]
    fn a_cached_conversation_says_it_is_waiting_for_its_page() {
        let mut app = App::mock();
        let seed: Vec<Message> = app
            .conversation
            .conversation
            .window
            .iter()
            .map(|message| Message {
                chat_id: 2,
                ..message.clone()
            })
            .collect();
        app.select_chat(1);
        assert!(app.seed_from_cache(2, seed));
        app.begin_fetch(FetchDirection::Latest);

        let (_, row) = bottom_row(&screen(&app));
        assert!(row.contains(REVALIDATING_LABEL), "{row:?}");

        app.flash("offline: the feed ended");
        let (_, row) = bottom_row(&screen(&app));
        assert!(
            row.contains("offline: the feed ended") && !row.contains(REVALIDATING_LABEL),
            "the failure owns the line: {row:?}"
        );
    }
}
