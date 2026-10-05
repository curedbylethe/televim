//! The `:shortcode` completion popup.
//!
//! It grows **upward** out of the input bar into the conversation, because the
//! bar is at the bottom and there is nothing below it but the status line.
//! Giving it the conversation's rectangle rather than the whole frame is what
//! makes the clamp free: it cannot grow past the top of the conversation, and
//! it cannot grow at all on a terminal with no conversation.
//!
//! **`Clear` first**, and bounded to its own area. Without it the popup's rows
//! are drawn over whatever message rows were already there, and a `Clear`-less
//! popup is a smear of two things. A `Clear` larger than the popup paints out
//! things it does not own. The conversation is painted before the popup, so
//! there is something to clear.
//!
//! The anchor — the caret's column, and its row, within the bar — is asked of
//! [`LineEditor::laid_out_in`], the same measurement `input_bar::render` uses, so
//! the popup sits under the `:query` rather than under the middle of the draft.
//! Under [`crate::bidi::BidiMode::Visual`] the column it anchors to is the
//! caret's **visual** one, because the bar's row is drawn permuted there and the
//! logical column is a cell the caret is not on. Nothing here computes the caret
//! again, and nothing is cached: the trigger is a live view of the line.
//!
//! The highlight is `theme.selection`, the same reverse video the chat list and
//! the conversation use, and there is no `highlight_symbol`: nothing else in
//! this workspace draws one, and reverse video belongs to the cursor and to
//! nothing else.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::App;
use crate::emoji::MAX_CANDIDATES;

/// How wide the popup draws.
///
/// A glyph is two columns, a space, and the longest shortcode a reader will
/// recognise in one glance (`slightly_smiling_face`) is twenty-three. 24 is the
/// point where the border starts to look like a panel rather than a list.
const POPUP_WIDTH: u16 = 24;

/// Where the popup goes, or `None` when there is not room for it.
///
/// `above` is the conversation's rectangle and `bar` the input bar's. Every
/// field saturates: `panic = "abort"` turns a `u16` underflow into a wrap in
/// release and a panic in debug, and a short terminal must produce neither.
fn popup_area(app: &App, above: Rect, bar: Rect, frame: Rect) -> Option<Rect> {
    app.completion()?;

    let width = bar.width.saturating_sub(2).max(1);
    let laid_out = app.input.line.laid_out_in(width, app.bidi());
    // Where the caret is **painted**, which under [`crate::bidi::BidiMode::Visual`]
    // is not where it sits in the string: the row reaches the terminal permuted,
    // so a right-to-left draft puts the caret among the cells rather than after
    // the bytes. `visual_column` is `None` in every other mode, and the logical
    // column is then the only answer this program has — which is the right one,
    // because the terminal is doing the reordering in that mode.
    let caret = laid_out.visual_column.unwrap_or(laid_out.column);

    let rows = app
        .completion()
        .map_or(0, |trigger| trigger.candidates.len())
        .min(MAX_CANDIDATES);
    let height = (rows + 2).min(usize::from(above.height));
    // No border plus at least one candidate: below that there is nothing worth
    // drawing, and a one-row popup would be all border.
    if height < 3 {
        return None;
    }

    let y = bar.y.saturating_sub(height as u16);
    let popup_width = POPUP_WIDTH.min(frame.width);
    let x = (bar.x + 1 + app.prompt_prefix().len() as u16 + caret as u16)
        .min(frame.width.saturating_sub(popup_width));

    Some(Rect::new(x, y, popup_width, height as u16))
}

pub fn render(app: &App, above: Rect, bar: Rect, frame: &mut Frame<'_>) {
    let Some(area) = popup_area(app, above, bar, frame.area()) else {
        return;
    };
    let Some(trigger) = app.completion() else {
        return;
    };

    frame.render_widget(Clear, area);

    let items: Vec<ListItem> = trigger
        .candidates
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .map(|emoji| {
            // The shortcode drawn is the emoji's *primary* one, even when the
            // reader matched an alias: `:hankey` shows `:poop:`, because that
            // is what the receiving end will read.
            let code = emoji
                .shortcode()
                .map_or_else(String::new, |code| format!(" {code}"));
            ListItem::new(Line::from(vec![
                Span::styled(emoji.as_str(), app.ui.theme.text),
                Span::styled(code, app.ui.theme.text_dim),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.ui.theme.border),
        )
        .highlight_style(app.ui.theme.selection);

    // The real index, and `List` scrolls it into view itself — the same as the
    // conversation panel, and the reason this widget has no offset arithmetic.
    let mut state = ListState::default();
    state.select(Some(trigger.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bidi::BidiMode;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Constraint, Direction, Layout};

    use crate::widgets::input_bar;

    /// The whole frame, drawn into an in-memory terminal.
    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// Sends one keystroke to the application, as the reader would.
    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Types `text` one character at a time.
    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    /// An application composing `draft`, which opens a completion.
    fn composing(draft: &str) -> App {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, draft);
        assert!(app.completion().is_some(), "{draft:?} opens a popup");
        app
    }

    /// Everything the screen says, as one string.
    fn flat(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The row of the screen carrying `needle`, if any.
    fn row_with(buffer: &Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|&y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains(needle)
        })
    }

    /// Whether two cells would draw the same.
    fn same(a: &Cell, b: &Cell) -> bool {
        a.symbol() == b.symbol() && a.fg == b.fg && a.bg == b.bg && a.modifier == b.modifier
    }

    /// The conversation and bar rectangles, laid out the way `App::render` lays
    /// them out.
    fn panes(app: &App, width: u16, height: u16) -> (Rect, Rect) {
        let full = Rect::new(0, 0, width, height);
        let inner = width.saturating_sub(2).max(1);
        let input = 2 + input_bar::content_rows(app, inner);
        let input = u16::try_from(input).unwrap_or(u16::MAX);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(input),
                Constraint::Length(1),
            ])
            .split(full);

        (vertical[0], vertical[1])
    }

    /// The rectangle the popup drew into, on a screen of this size.
    fn drawn_area(app: &App, width: u16, height: u16) -> Rect {
        let (above, bar) = panes(app, width, height);
        popup_area(app, above, bar, Rect::new(0, 0, width, height))
            .expect("the popup has room here")
    }

    #[test]
    fn the_popup_draws_the_glyph_and_the_shortcode() {
        let app = composing(":cr");

        let shown = flat(&screen(&app, 80, 24));

        assert!(shown.contains("😢"), "the glyph is on the screen: {shown}");
        assert!(shown.contains("cry"), "and so is its shortcode");
    }

    /// The `Clear` is bounded to the popup's own area: a cell outside it is
    /// exactly what it was before the popup existed.
    #[test]
    fn the_popup_does_not_paint_over_the_conversation_outside_itself() {
        let open = composing(":cry");
        let mut closed = composing(":cry");
        press(&mut closed, KeyCode::Esc);
        assert!(closed.completion().is_none(), "the fixture closed it");

        let with = screen(&open, 80, 24);
        let without = screen(&closed, 80, 24);
        let rect = drawn_area(&open, 80, 24);
        // The status line is not the conversation, and it says different things
        // with a completion up; the question here is only what the popup paints.
        let (_, bar) = panes(&open, 80, 24);

        let mut changed = 0;
        for y in 0..bar.y {
            for x in 0..80 {
                if !same(&with[(x, y)], &without[(x, y)]) {
                    changed += 1;
                    assert!(
                        x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom(),
                        "the popup changed ({x}, {y}) outside its own rectangle"
                    );
                }
            }
        }
        assert!(changed > 0, "the popup drew nothing at all");
    }

    #[test]
    fn the_highlighted_row_is_the_selected_candidate() {
        let mut app = composing(":cry");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.completion().expect("up").selected, 1);

        let buffer = screen(&app, 80, 24);
        let marked = app
            .completion()
            .expect("up")
            .chosen()
            .expect("a candidate")
            .as_str();
        let y = row_with(&buffer, marked).expect("the selected glyph is drawn");
        let x = (0..buffer.area.width)
            .find(|&x| buffer[(x, y)].symbol() == marked)
            .expect("its column");

        assert!(
            buffer[(x, y)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "the selected row is the highlighted one"
        );
    }

    #[test]
    fn the_popup_grows_upward_out_of_the_bar() {
        let app = composing(":cr");
        let (_, bar) = panes(&app, 80, 24);

        let rect = drawn_area(&app, 80, 24);

        assert_eq!(
            rect.bottom(),
            bar.y,
            "its bottom border is the row above the bar"
        );
    }

    #[test]
    fn the_popup_stays_inside_a_terminal_with_no_room_for_it() {
        let app = composing(":cr");

        // Two rows of conversation is a top border, a bottom border and no
        // candidate between them: there is nothing to draw.
        let squeezed = popup_area(
            &app,
            Rect::new(0, 0, 20, 2),
            Rect::new(0, 2, 20, 3),
            Rect::new(0, 0, 20, 5),
        );
        assert!(squeezed.is_none(), "no border plus a candidate row");

        // And a real frame too short to hold one renders without panicking.
        screen(&app, 20, 5);
    }

    #[test]
    fn the_popup_stays_inside_a_terminal_narrower_than_it_wants_to_be() {
        let app = composing(":cr");

        let rect = drawn_area(&app, 12, 20);

        assert_eq!(rect.width, 12, "clipped to the frame's width");
        assert_eq!(rect.x, 0, "and held inside it");
        screen(&app, 12, 20);
    }

    #[test]
    fn the_popup_follows_the_caret_across_rows() {
        // The draft wraps at the bar's 28-column inner width, so the caret is
        // on the second row at column three — not at the column it would have
        // if the whole draft were counted.
        let app = composing("abcdefghijklmnopqrstuvwxyz :cr");

        let laid_out = app.input.line.laid_out(28);
        assert_eq!(laid_out.row, 1, "the draft wrapped and the caret is below");
        assert_eq!(laid_out.column, 3, "`:cr` starts a fresh row");
        // The default mode is the terminal's to reorder, so this program has one
        // column to name and `laid_out_in` names the same one.
        assert_eq!(
            app.input.line.laid_out_in(28, BidiMode::Terminal),
            laid_out,
            "and the default mode is the measurement this test already pinned"
        );

        let rect = drawn_area(&app, 30, 20);

        assert_eq!(rect.x, 4, "the popup is under the caret's own row");
        assert!(rect.right() <= 30, "and inside the frame: {}", rect.right());
    }

    /// The popup follows the caret to the cell the caret is **painted** on, which
    /// on a right-to-left draft drawn here is not the cell it would occupy in the
    /// string.
    ///
    /// The draft is Hebrew with a `:cry` at the end, so the row is one reversed
    /// run: the logical end of the draft is drawn at the **left** of it and the
    /// two columns differ, which is what makes this test bite — a popup anchored
    /// to `column` lands under the middle of the draft rather than under the
    /// caret. `TestBackend` has no shaper, so this proves the
    /// [`BidiMode::Visual`] path only (see `docs/known-gaps.md`).
    #[test]
    fn the_popup_follows_the_caret_to_its_visual_column_on_a_right_to_left_draft() {
        let app = composing("שלום :cry").with_bidi(BidiMode::Visual);

        let laid_out = app.input.line.laid_out_in(78, BidiMode::Visual);
        let visual = laid_out
            .visual_column
            .expect("a visual column under Visual mode");
        assert_ne!(
            visual, laid_out.column,
            "the draft reads right-to-left, so the painted caret is not at the \
             logical column: {laid_out:?}"
        );
        assert_eq!(
            app.input.line.laid_out(78).visual_column,
            None,
            "and the default mode has no visual column to give"
        );

        let rect = drawn_area(&app, 80, 24);
        let (_, bar) = panes(&app, 80, 24);

        assert_eq!(
            rect.x,
            bar.x
                + 1
                + u16::try_from(app.prompt_prefix().len()).expect("a prefix fits a u16")
                + u16::try_from(visual).expect("a column fits a u16"),
            "the popup is under the cell the caret is painted on: {rect:?}"
        );
        assert!(
            laid_out.row == 0,
            "the draft is one row, so this is not the across-rows case wearing a hat"
        );
    }

    /// The default mode is unchanged: the terminal does the reordering there, so
    /// the logical column is the cell the caret is drawn in and the popup stands
    /// where it always has.
    #[test]
    fn the_popup_stands_where_it_did_in_the_default_mode() {
        let app = composing("שלום :cry");
        let rect = drawn_area(&app, 80, 24);
        let (_, bar) = panes(&app, 80, 24);
        let column = app.input.line.laid_out(78).column;

        assert_eq!(
            rect.x,
            bar.x
                + 1
                + u16::try_from(app.prompt_prefix().len()).expect("a prefix fits a u16")
                + u16::try_from(column).expect("a column fits a u16"),
            "the popup is under the logical caret's column: {rect:?}"
        );
    }
}
