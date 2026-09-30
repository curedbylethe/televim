//! Input bar shown above the status line.
//!
//! The bar is always a draft, never a field and never absent: it holds what the
//! reader last typed, or a hint when there is nothing. That is the whole of why
//! the bar grew — a draft can be several rows tall, it has a caret that moves,
//! and a dimmed line of text is indistinguishable from output unless it is
//! marked as what it is.
//!
//! Four states, and they are four because the bar is answering two questions at
//! once: what a key would do, and whether there is a draft. The key hints go in
//! the status line beside it, which is a row and has room for seventy columns;
//! the bar itself is for the words.
//!
//! One thing on it is not the words, and only while it is being typed in: a
//! space is a cell that paints nothing, and a caret on a blank cell is a bar on
//! a blank cell, so a key that typed one looked like a key that did nothing.
//! The bar stands a dim `·` in for every space in a draft the reader is
//! composing — Vim's `list`, extended past the trailing whitespace it would
//! mark, because a space typed between two words is as invisible as one typed at
//! the end. The conversation does not do this: a message is read as prose, and
//! a sentence with its spaces dotted is not a sentence any more.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Focus, Mode, PromptKind};
use crate::text_row;

/// How many content rows the bar may take before it starts scrolling.
///
/// A draft is the reader's own words and the bar is the only place they can be
/// read back, so it is worth taking rows from the conversation for — up to a
/// point. Six is that point: past it the conversation is what the reader is
/// reading, and a bar that took a quarter of a 24-row screen would be answering
/// a question nobody asked.
pub const INPUT_MAX_ROWS: usize = 6;

/// The hint while the conversation has the focus in Normal mode and the bar is
/// empty.
///
/// A constant so its length can be checked: the hints have to fit one row of the
/// widest terminal the client assumes, and every key that has been added has
/// meant shortening the mode keys rather than letting the line run past the bar
/// and be clipped.
///
/// The row was exactly full — seventy columns, all seventy of the budget — when
/// the account's profile took `D:dismiss` out and `S:acct` in. `D` dismisses a
/// *failed* send, which is a refusal, and a refusal is the status line's to say
/// rather than the hint's: the row has no room for why.
const NORMAL_HINT: &str = " i:ins  r:rep  e:edit  dd:del  v:vis  /:find  ::cmd  q:quit  S:acct";

/// The hint while the chat list has the focus.
///
/// The same spelling of the profile key as the conversation's row. One key, one
/// word, two rows: two spellings for one key is something a reader has to notice
/// and reconcile.
const CHAT_LIST_HINT: &str = " j/k: chat  Enter: open  Tab: pane  h: conversation  S:acct";

/// The hint while a selection is being made over the messages.
const VISUAL_HINT: &str = " d: delete  y: yank  r: reply  Esc: cancel";

/// The hint while the profile panel has the focus.
///
/// Its own row, and the reason is not cosmetic: `hint()` matches on
/// `(focus, mode)`, and a focused profile *is* a focused conversation as far as
/// that pair goes. Without this the panel would name `dd:del` and `e:edit` —
/// keys that do nothing on it, which is worse than saying nothing, because it
/// tells the reader the keys are wrong rather than that they are not shown.
///
/// `d` is named even though six of the eight rows ignore it, because the two
/// that do not are the only interaction the panel has and the hint is the only
/// place a reader learns they exist.
/// A card's own hint, and the account's.
const CARD_SELF_HINT: &str = " j/k: row  h/l: within  v: vis  y: yank  d: act  Esc: back";

/// A contact's card.
///
/// The same keys as the account's own **except** `d`, and the difference is the
/// whole of what a contact's card cannot do: it has no row to act on, so naming
/// `d` here would be naming a key that refuses. And `yy` rather than `y`, because
/// a contact has no actions and yanking the row whole is the only yank it has.
///
/// The two rows differ because their rows differ. A key named on a card that does
/// not answer it tells the reader the key is wrong, so neither names the other's.
const CARD_CONTACT_HINT: &str = " j/k: row  h/l: within  v: vis  y/yy: yank  Esc: back";

/// The hint while the bar holds a draft and the conversation has the focus.
///
/// A draft is a draft, and saying so is the difference between it and a message
/// that failed to send — which this program has, and which the conversation
/// draws as `[failed: …]`.
const DRAFT_HINT: &str = " ⏎ draft — i to continue, ^J/⏎ to discard";

/// The hint while the line is being typed in.
///
/// `^J` first, because it works in every terminal and needs no protocol. A
/// shifted `Enter` is the same key where the terminal volunteers the
/// distinction — `xterm` among others does not, and there a shifted `Enter`
/// arrives as a bare one, which sends. See [`crate::line::LineEditor::feed`]
/// for the limitation, which the bar does not have room to repeat.
const INSERT_HINT: &str = " ⏎: send  ^J: newline  shift+⏎: newline where supported";

/// The hint while a `:query` is being completed.
///
/// `Enter` and `Tab` are the message's own keys and they mean something else
/// for as long as this is on the row, so the row says which something.
const COMPLETION_HINT: &str = " ⇥/⏎: pick  ↑/↓: choose  Esc: close";

/// The hint while the line is in its own normal mode.
///
/// No `j`/`k`, and that is not an oversight: `vim-line` makes those history
/// navigation on a one-line buffer, and history is not built. The caret moves
/// between the lines of a message from insert mode, where `Up` and `Down` are
/// motions. See [`crate::line`]'s docs for what the crate actually does.
///
/// `gg` and `G` are here because the library has neither, and a key the line
/// answers with nothing else on screen naming it is a key the hint exists for.
/// They took the room `word` and `chg` had — the row is exactly as full as it
/// was before, which is what the length test below is for.
const LINE_NORMAL_HINT: &str =
    " i/a: ins  w/b/e  x: del  dw/cc  p: paste  gg/G: ends  ⏎: send  Esc";

/// The hint while a selection is being made inside the line.
///
/// A free consequence of the editor being a real one: line-visual gets `d` and
/// `y` because the crate implements them, and all it needs here is the hint.
const LINE_VISUAL_HINT: &str = " y: yank  d: cut  Esc: back";

/// The columns the status line spends on the mode label and the gap after it.
///
/// The widest label the line itself can produce is `NORMAL`, and a `Confirm`
/// never shares this row with a hint, so this is one number rather than a
/// special case per label.
#[cfg(test)]
const MODE_LABEL_WIDTH: usize = 9;

/// The widest terminal the client assumes, which is what a hint is measured
/// against.
///
/// It used to be the bar's inner width instead — the hints were drawn in the bar
/// and had its two borders to themselves. They are on the status line now, where
/// the mode label shares the row, so the budget is two columns smaller and
/// nothing is gained by pretending otherwise.
#[cfg(test)]
const ASSUMED_WIDTH: usize = 80;

/// Every hint, in one list, so the width test cannot forget one.
///
/// Nine entries, and that is not the same as nine before: the confirmation's row
/// was here and was not reachable. A confirmation outranks every hint, so
/// `status_text` returns the prompt's own sentence before it ever asks for one —
/// the string was width-checked by a test and displayed by nothing. The profile's
/// row replaced it, which is what `ALL_HINTS` is for.
#[cfg(test)]
const ALL_HINTS: [&str; 10] = [
    NORMAL_HINT,
    CHAT_LIST_HINT,
    VISUAL_HINT,
    CARD_SELF_HINT,
    CARD_CONTACT_HINT,
    DRAFT_HINT,
    INSERT_HINT,
    COMPLETION_HINT,
    LINE_NORMAL_HINT,
    LINE_VISUAL_HINT,
];

/// The hint for the state the screen is in.
///
/// One place, because three questions share one row: which pane has the focus,
/// what a key would do, and whether the bar holds something the reader wrote and
/// has not sent. An empty bar and a bar with a draft in it are different states,
/// and showing the mode hint over a half-written message is how a draft comes to
/// look like output.
#[must_use]
pub fn hint(app: &App) -> &'static str {
    match (app.focus, app.mode) {
        // Above the mode hints it specialises: while a completion is up, the
        // keys it takes mean something else, and the row has to say so.
        (Focus::Input, _) if app.completion().is_some() => COMPLETION_HINT,
        (Focus::Input, _) if app.line.purpose().is_buffer() => match app.line.status() {
            "VISUAL" => LINE_VISUAL_HINT,
            "NORMAL" => LINE_NORMAL_HINT,
            _ => INSERT_HINT,
        },
        (Focus::ChatList, _) => CHAT_LIST_HINT,
        // Before the conversation's own arms, and matched on the pane as well as
        // the mode: a card is in the right-hand column with the focus on it, so
        // without this the pair below would answer for it. Two rows, because the
        // two subjects do not have the same keys.
        (Focus::Conversation, Mode::Normal) if app.pane.is_profile() => match app.card_subject() {
            crate::card::CardSubject::SelfAccount => CARD_SELF_HINT,
            crate::card::CardSubject::Contact(_) => CARD_CONTACT_HINT,
        },
        (Focus::Conversation, Mode::Visual) => VISUAL_HINT,
        (Focus::Conversation, Mode::Normal) if !app.line.is_empty() => DRAFT_HINT,
        (Focus::Conversation, Mode::Normal) => NORMAL_HINT,
        // The two states that say nothing, and both say it for the same reason.
        // A confirmation outranks every hint on the status line, so the prompt's
        // own sentence is what is drawn and the confirm arm is reached by
        // nothing — it used to return a row, which meant a string was
        // width-checked by a test and displayed by no one. A command or a search
        // line has no keys to name either: they are a line the reader is typing
        // into, and the line answers for itself.
        (Focus::Input, _) | (Focus::Conversation, Mode::Confirm) => "",
    }
}

/// The word the status line names the mode with.
///
/// The line's own when the line has the focus, because the line has a mode of
/// its own and it is not the conversation's: being on the line is no longer the
/// whole of what the line is doing.
#[must_use]
pub fn mode_label(app: &App) -> &'static str {
    match (app.focus, app.mode) {
        (Focus::Input, _) if app.line.purpose().is_buffer() => match app.line.status() {
            "VISUAL" => Mode::Visual.label(),
            "NORMAL" => Mode::Normal.label(),
            _ => "INSERT",
        },
        (Focus::Input, _) => "INSERT",
        // Named rather than wildcarded. A wildcard here is right until the day a
        // second pane of the right-hand column can be in Visual or Confirm, and
        // by then it is a label a reader has been misreading.
        (Focus::Conversation, Mode::Visual) => Mode::Visual.label(),
        (Focus::Conversation, Mode::Confirm) => Mode::Confirm.label(),
        (Focus::ChatList, _) | (Focus::Conversation, Mode::Normal) => Mode::Normal.label(),
    }
}

/// How many content rows the bar takes.
///
/// One, unless the line has the focus and there is more than one row of draft to
/// show: a hint or a dimmed draft the reader is not typing in is one row, and a
/// bar that took six of them for a one-line message would be taking them from
/// the conversation for nothing.
#[must_use]
pub fn content_rows(app: &App, width: u16) -> usize {
    if app.focus != Focus::Input || app.line.text().is_empty() {
        return 1;
    }

    crate::wrap::wrap_keeping_whitespace(app.line.text(), width)
        .len()
        .clamp(1, INPUT_MAX_ROWS)
}

/// The title on the bar while the reader is composing, or while there is a draft
/// they are not composing.
///
/// The chat's name rather than the prompt's alone, because a draft belongs to no
/// conversation (see [`App::select_chat_none`]) and the one thing a reader
/// cannot work out for themselves is where it will be sent. A reply says what it
/// answers, because a reply the reader cannot see the target of is a reply they
/// have to guess at.
#[must_use]
pub fn title(app: &App) -> String {
    match (app.focus, app.line.purpose(), app.line.is_empty()) {
        (Focus::Input, PromptKind::Message, _) => match app.open_chat_name() {
            Some(name) => format!(" Message to {name} "),
            None => " Message ".to_owned(),
        },
        (Focus::Input, PromptKind::Reply, _) => " Reply ".to_owned(),
        (Focus::Input, PromptKind::Edit, _) => " Edit ".to_owned(),
        (Focus::Input, PromptKind::Command, _) => " Command ".to_owned(),
        (Focus::Input, PromptKind::Search, _) => " Find ".to_owned(),
        (_, _, false) => " draft ".to_owned(),
        _ => " Input ".to_owned(),
    }
}

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let focused = app.focus == Focus::Input;

    // The columns and rows the text has, which is the area less the border.
    let width = area.width.saturating_sub(2).max(1);
    let height = usize::from(area.height).saturating_sub(2).max(1);

    let laid_out = app.line.laid_out(width);
    let first = laid_out.first_row(height);

    let prefix = app.prompt_prefix();

    let lines: Vec<Line> = laid_out
        .rows
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(row, range)| {
            // The `:` or `/` goes in front of the first row only: it names the
            // whole line, and a reader who types two of them has a question
            // rather than a command.
            let lead = if row == 0 && !prefix.is_empty() {
                Some(prefix)
            } else {
                None
            };
            body_row(app, range, row == laid_out.row, focused, lead)
        })
        .collect();

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border(app))
            .title(title(app)),
    );

    frame.render_widget(paragraph, area);

    // The caret is painted by `body_row`, into the buffer, as an ordinary cell.
    //
    // It used to be asked of the terminal with `set_cursor_position`, and the two
    // are not the same thing. A terminal cursor has exactly one shape, and this
    // program needs two: a block while the line is being composed and a hollow in
    // the line's own Normal mode. Asking for a block gave a filled block in both,
    // so the state the reader is most often in was the one that was wrong. It also
    // cannot be tested: `TestBackend` does not model a terminal cursor at all, so
    // a painted caret is a cell an assertion can look at and a real one is not.
}

/// One row of the draft.
///
/// The text, the selection and the caret are [`text_row`]'s, because a card row
/// wants the same three and doing them here alone is how two panels come to
/// disagree about what a selection looks like. Only the prompt's prefix is this
/// panel's.
fn body_row<'a>(
    app: &'a App,
    range: &Range<usize>,
    caret_row: bool,
    focused: bool,
    lead: Option<&'a str>,
) -> Line<'a> {
    let text = app.line.text();
    // The prefix is drawn in front of the first row, so the text on it has that
    // many columns fewer — the same reason a message's sender is subtracted
    // before its rows are cut rather than clipped after.
    let lead_columns = lead.map_or(0, str::len);
    let body = (range.start + lead_columns).min(range.end)..range.end;

    let mut spans = Vec::new();
    if let Some(lead) = lead {
        spans.push(Span::styled(lead, app.theme.text_dim));
    }

    spans.extend(text_row::spans(&text_row::TextRow {
        text,
        range: body,
        // A draft is not a search result: `/` searches the window and the server,
        // never the line the reader is typing in.
        matched: false,
        selected: app.line.selection(),
        // The bar is never reversed, so the caret is its own style rather than a
        // hole in a row of reverse video. It needs the focus as well as the row:
        // a caret on a draft nobody is typing into is a promise the program does
        // not keep.
        caret: (focused && caret_row).then(|| app.line.caret()),
        reversed: false,
        // `status()` rather than a mode of this panel's own, because the line's
        // mode is the line's and the bar already reads it to pick its hint.
        ink: text_row::Ink::draft(&app.theme, focused, app.line.status() == "NORMAL"),
    }));

    Line::from(spans)
}

/// The border style.
///
/// The bar does not take the focus visually, it *is* the focus, so it is the
/// one pane whose border is on exactly when the line is.
fn border(app: &App) -> Style {
    if app.focus == Focus::Input {
        app.theme.border_focused
    } else {
        app.theme.border
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

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

    /// Sends `code` with control held, as `Ctrl+J` arrives.
    fn press_ctrl(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::CONTROL));
    }

    /// Types `text` one character at a time.
    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    /// Types `text`, leaves the line, and comes back to the conversation.
    fn drafted(app: &mut App, text: &str) {
        press(app, KeyCode::Char('i'));
        type_text(app, text);
        press(app, KeyCode::Esc);
        press(app, KeyCode::Esc);
    }

    /// Everything the screen says, as one string, so a test can look for a
    /// phrase without caring which row or column it landed in.
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

    /// The rows the screen draws, top to bottom, with the borders left on.
    fn rows_of(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// The row the bar's top border is on, which is the row carrying its title.
    ///
    /// Found on the screen rather than computed from the layout, because the
    /// question this answers is whether the layout gave the bar the rows the
    /// wrapper asked for — a `Length` that was not honoured is invisible to any
    /// test of the number.
    fn bar_top(rows: &[String]) -> usize {
        rows.iter()
            .position(|row| row.contains("Message to"))
            .expect("the bar draws a title, and this one is a message's")
    }

    /// The row the bar's top border is on while it holds a draft nobody is
    /// typing in, which titles itself a draft rather than a message.
    fn draft_top(rows: &[String]) -> usize {
        rows.iter()
            .position(|row| row.contains(" draft "))
            .expect("the bar marks a draft, and this is one")
    }

    // ---- the hints -------------------------------------------------------

    /// Every hint has to fit the row it is drawn on beside the mode label, and a
    /// hint longer than that is clipped mid-word — which reads as a bug rather
    /// than as a hint. The list is a constant so adding a hint without measuring
    /// it does not compile.
    #[test]
    fn every_hint_fits_the_row_they_are_drawn_on() {
        for hint in ALL_HINTS {
            assert!(
                hint.chars().count() <= ASSUMED_WIDTH - MODE_LABEL_WIDTH,
                "the hint is {} columns, and the status line has {}: {hint:?}",
                hint.chars().count(),
                ASSUMED_WIDTH - MODE_LABEL_WIDTH
            );
        }
    }

    /// A hint that names a key nothing else on the screen names is a hint that
    /// has to be there, so `dw`, `cc`, `w` and `gg`/`G` are asserted on
    /// individually — a test that only measures length would pass on a hint that
    /// had lost them. `gg` and `G` are the wrapper's own rather than the
    /// library's, so nothing else on the screen names them.
    #[test]
    fn the_line_hints_name_the_keys_the_line_and_nothing_else_answers() {
        for key in ["i/a", "w/b/e", "x", "dw", "cc", "p", "gg", "G", "⏎", "Esc"] {
            assert!(
                LINE_NORMAL_HINT.contains(key),
                "{key:?} is bound in the line and nowhere else, and the hint has dropped it"
            );
        }
        for key in ["y", "d", "Esc"] {
            assert!(LINE_VISUAL_HINT.contains(key), "{key:?} is missing from it");
        }
    }

    #[test]
    fn a_held_line_says_what_its_own_mode_does() {
        let mut app = App::mock();

        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hi");
        assert_eq!(
            hint(&app),
            INSERT_HINT,
            "insert: how to send, and how to stay"
        );
        assert_eq!(
            mode_label(&app),
            "INSERT",
            "and the line names its own mode"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(hint(&app), LINE_NORMAL_HINT, "the line's own normal mode");
        assert_eq!(mode_label(&app), Mode::Normal.label());

        press(&mut app, KeyCode::Char('v'));
        assert_eq!(hint(&app), LINE_VISUAL_HINT, "and a selection inside it");
        assert_eq!(mode_label(&app), Mode::Visual.label());
    }

    /// A popup that silently takes `Enter` away is a key answering a different
    /// question than the reader asked, so the row names the keys it takes — and
    /// goes back to the insert hint the moment it is closed.
    #[test]
    fn the_status_line_says_the_completion_has_those_keys() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(hint(&app), INSERT_HINT);

        type_text(&mut app, ":cr");
        assert_eq!(hint(&app), COMPLETION_HINT, "the popup names its keys");

        press(&mut app, KeyCode::Esc);
        assert_eq!(hint(&app), INSERT_HINT, "and the row goes back");
    }

    /// An operator waiting for its motion is still editing, so the bar keeps
    /// saying so — and the mode names the operator, which is the only place a
    /// reader learns they have half-typed a `d`.
    #[test]
    fn an_operator_waiting_for_its_motion_is_still_editing() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('d'));

        assert_eq!(app.line.status(), "d...");
        assert_ne!(mode_label(&app), Mode::Normal.label());
    }

    // ---- what is on the bar ----------------------------------------------

    #[test]
    fn an_unfocused_bar_marks_a_draft_as_one() {
        let mut app = App::mock();
        drafted(&mut app, "half a th");

        let shown = flat(&screen(&app, 80, 24));

        assert!(shown.contains("half a th"), "the draft is on show");
        assert!(
            shown.contains("draft"),
            "and it is marked, or it is indistinguishable from a failed send: {shown}"
        );
    }

    #[test]
    fn an_unfocused_bar_with_nothing_in_it_says_it_is_empty() {
        let app = App::mock();
        let shown = flat(&screen(&app, 80, 24));

        assert!(shown.contains("i:ins"), "the mode hint is on show");
        assert!(!shown.contains("draft"), "and nothing claims to be a draft");
    }

    /// A draft is a draft whether or not the reader is typing in it, so the
    /// focused one is on show too. The spaces are dots, because that is what the
    /// bar draws while the line has the focus.
    #[test]
    fn a_draft_is_shown_while_the_line_is_focused_too() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "half a th");

        assert!(flat(&screen(&app, 80, 24)).contains("half·a·th"));
    }

    // ---- the draft's subject --------------------------------------------

    /// A draft belongs to no conversation, so the one thing a reader cannot work
    /// out is where it will be sent. The title has to say.
    #[test]
    fn the_title_names_the_conversation_a_message_would_be_sent_to() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(title(&app), " Reply ", "a reply says what it answers");

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('i'));

        let title = title(&app);
        assert!(
            title.contains("Message to"),
            "a message names its chat: {title:?}"
        );
        assert!(
            title.contains("Ada Lovelace"),
            "by the chat's name and not by a number: {title:?}"
        );
    }

    #[test]
    fn an_unfocused_bar_titles_itself_a_draft() {
        let mut app = App::mock();
        drafted(&mut app, "half a th");

        assert_eq!(title(&app), " draft ", "and the mark is in the title");
    }

    // ---- the height ------------------------------------------------------

    #[test]
    fn an_unfocused_draft_is_one_row_whatever_its_length() {
        let mut app = App::mock();
        drafted(&mut app, "one\ntwo\nthree\nfour");

        assert_eq!(
            content_rows(&app, 78),
            1,
            "a draft the reader is not typing in is one row; the height is for the caret"
        );
    }

    #[test]
    fn a_held_draft_grows_the_bar_a_row_at_a_time() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "one");
        assert_eq!(content_rows(&app, 78), 1);

        press_ctrl(&mut app, KeyCode::Char('j'));
        type_text(&mut app, "two");
        assert_eq!(content_rows(&app, 78), 2);

        press_ctrl(&mut app, KeyCode::Char('j'));
        type_text(&mut app, "three");
        assert_eq!(content_rows(&app, 78), 3);
    }

    #[test]
    fn a_draft_stops_growing_at_the_ceiling() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        for line in 0..20 {
            if line > 0 {
                press_ctrl(&mut app, KeyCode::Char('j'));
            }
            type_text(&mut app, "a line");
        }

        assert_eq!(
            content_rows(&app, 78),
            INPUT_MAX_ROWS,
            "the conversation is what the reader is reading past this"
        );
    }

    /// The arithmetic rather than the rectangle is not enough: a `Length` the
    /// layout did not honour is invisible to any test of the number, so this
    /// looks for the rows on the screen.
    #[test]
    fn a_taller_bar_costs_the_conversation_its_rows() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "one");
        press_ctrl(&mut app, KeyCode::Char('j'));
        type_text(&mut app, "two");
        press_ctrl(&mut app, KeyCode::Char('j'));
        type_text(&mut app, "three");

        let rows = rows_of(&screen(&app, 80, 24));
        let top = bar_top(&rows);

        assert_eq!(content_rows(&app, 78), 3);
        assert!(
            rows[top].contains("Message to"),
            "the bar's title is on its top border: {:?}",
            rows[top]
        );
        for (below, draft) in ["one", "two", "three"].into_iter().enumerate() {
            assert!(
                rows[top + 1 + below].contains(draft),
                "{draft:?} is drawn on row {}: {:?}",
                top + 1 + below,
                rows[top + 1 + below]
            );
        }
        assert!(
            rows[top + 4].contains("─"),
            "and the bar is closed below its three rows: {:?}",
            rows[top + 4]
        );
    }

    // ---- the spaces -------------------------------------------------------

    /// A space is a cell that paints nothing, and a caret on a blank cell is a
    /// bar on a blank cell — so a key that typed one looked like a key that did
    /// nothing at all. The bar has to show it.
    #[test]
    fn a_space_being_typed_is_drawn_as_a_dot() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "a b");

        let rows = rows_of(&screen(&app, 80, 24));
        let top = bar_top(&rows);

        assert!(
            rows[top + 1].contains("a·b"),
            "the space between the words is on show: {:?}",
            rows[top + 1]
        );
    }

    /// The cells of the bar that carry a caret, as `(x, y, modifier)`.
    ///
    /// A painted caret is an ordinary cell with a modifier on it, which is the
    /// whole reason it is painted: this is a thing an assertion can find, and a
    /// terminal cursor is not a thing at all in a `TestBackend` buffer.
    fn carets(buffer: &Buffer, top: u16) -> Vec<(u16, u16, Modifier)> {
        let mut found = Vec::new();
        for y in top..buffer.area.height {
            for x in 0..buffer.area.width {
                let modifier = buffer[(x, y)].modifier;
                if modifier.contains(Modifier::REVERSED) || modifier.contains(Modifier::UNDERLINED)
                {
                    found.push((x, y, modifier));
                }
            }
        }
        found
    }

    /// The design's rule, in the one place it used to be untestable. The line
    /// asks the terminal for nothing; it paints a block while it is being
    /// composed, and that block is a cell in the buffer.
    #[test]
    fn a_line_being_composed_paints_its_caret_as_a_reversed_cell() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hi");

        let buffer = screen(&app, 80, 24);
        let top = bar_top(&rows_of(&buffer)) as u16;
        let found = carets(&buffer, top);

        assert_eq!(found.len(), 1, "one caret, on one cell: {found:?}");
        assert!(
            found[0].2.contains(Modifier::REVERSED),
            "an insert caret is a block: {:?}",
            found[0].2
        );
        // One past the last character, because that is where a reader types.
        assert_eq!(
            found[0].0, 3,
            "the draft is drawn at column 1, so this is its end"
        );
    }

    /// The two carets are two shapes, which is the reason neither of them asks
    /// the terminal for one: a terminal cursor is a block in both modes, and the
    /// mode the reader is most often in is the one that was wrong.
    #[test]
    fn the_line_in_its_own_normal_mode_marks_its_caret_rather_than_reversing_it() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hi");
        press(&mut app, KeyCode::Esc);

        let buffer = screen(&app, 80, 24);
        let top = bar_top(&rows_of(&buffer)) as u16;
        let found = carets(&buffer, top);

        assert_eq!(found.len(), 1, "one caret, on one cell: {found:?}");
        assert!(
            found[0].2.contains(Modifier::UNDERLINED) && !found[0].2.contains(Modifier::REVERSED),
            "a normal caret marks its cell rather than filling it: {:?}",
            found[0].2
        );
    }

    /// A caret the reader cannot move is a decoration, and this is the first
    /// thing that can say it is not one.
    #[test]
    fn a_caret_moves_when_the_caret_moves() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hi");

        let buffer = screen(&app, 80, 24);
        let top = bar_top(&rows_of(&buffer)) as u16;
        let before = carets(&buffer, top)[0].0;

        press(&mut app, KeyCode::Left);

        let buffer = screen(&app, 80, 24);
        let after = carets(&buffer, top)[0].0;

        assert_eq!(
            after,
            before - 1,
            "one cell back, and the cell moved with it"
        );
    }

    /// A bar nobody is typing in is read, not edited, so it has no caret to
    /// show — a caret on a draft the reader cannot type into is a promise the
    /// program does not keep.
    #[test]
    fn a_draft_nobody_is_typing_in_has_no_caret() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hi");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);

        let buffer = screen(&app, 80, 24);
        let top = draft_top(&rows_of(&buffer)) as u16;

        assert!(
            carets(&buffer, top).is_empty(),
            "a draft is not being edited"
        );
    }

    /// The case with nothing else on the bar to go on: a draft of spaces has no
    /// visible character in it at all, so without the dot the reader cannot tell
    /// the key was entered from the key being dropped.
    #[test]
    fn a_draft_of_nothing_but_spaces_is_not_an_empty_bar() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "   ");

        let rows = rows_of(&screen(&app, 80, 24));
        let top = bar_top(&rows);

        assert!(
            rows[top + 1].contains("···"),
            "three spaces are three dots: {:?}",
            rows[top + 1]
        );
    }

    /// A draft nobody is typing in is read before it is sent, and it is read as
    /// prose. The conversation is prose too, and it gets no dots.
    #[test]
    fn a_draft_nobody_is_typing_in_keeps_its_spaces_as_they_are() {
        let mut app = App::mock();
        drafted(&mut app, "half  a th");

        let rows = rows_of(&screen(&app, 80, 24));
        let top = draft_top(&rows);

        assert!(
            rows[top + 1].contains("half  a th"),
            "the draft is on show as it was typed: {:?}",
            rows[top + 1]
        );
        assert!(
            !rows[top + 1].contains('·'),
            "and nothing marks it: {:?}",
            rows[top + 1]
        );
    }

    /// The dot has to be the cell the space already had. One column more or
    /// less and the caret and the wrap drift away from what is drawn — the
    /// arithmetic the caret position is computed from is in [`App`] and in
    /// [`crate::line`], and this is what keeps the drawing inside it.
    #[test]
    fn a_dot_takes_the_space_s_own_column_and_no_other() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "a b ");

        let rows = rows_of(&screen(&app, 80, 24));
        let top = bar_top(&rows);
        let drawn = rows[top + 1]
            .trim()
            .trim_start_matches('│')
            .trim_end_matches('│')
            .trim_end();

        assert_eq!(
            drawn.chars().count(),
            app.line.text().chars().count(),
            "two spaces are two columns, and the trailing one is on show: {drawn:?}"
        );
    }

    // ---- the caret --------------------------------------------------------

    /// `TestBackend` does not model a terminal cursor, so the cursor cannot be
    /// asserted here and has to be checked by hand. What can be asserted is that
    /// the bar asks for a position, and the arithmetic behind it is tested in
    /// `App` and in `crate::line`.
    #[test]
    fn a_long_draft_is_drawn_on_the_rows_the_caret_is_on() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        for line in 0..10 {
            if line > 0 {
                press_ctrl(&mut app, KeyCode::Char('j'));
            }
            type_text(&mut app, "a line of text");
        }

        let laid_out = app.line.laid_out(78);
        let first = laid_out.first_row(INPUT_MAX_ROWS);
        let drawn = flat(&screen(&app, 80, 24));

        assert_eq!(laid_out.rows.len(), 10);
        assert!(
            laid_out.row >= first && laid_out.row < first + INPUT_MAX_ROWS,
            "the caret is on screen: row {} of {first}..{}",
            laid_out.row,
            first + INPUT_MAX_ROWS
        );
        assert!(
            !drawn.contains("a line of text\na line of text"),
            "and the rows above it are not drawn under the bar: {drawn}"
        );
    }
}
