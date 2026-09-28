//! The text being composed, and the editor working on it.
//!
//! A spike, kept. `vim-line` is declared as `7.7`, which is a caret range and
//! not a pin, so the four questions this workspace depends on are asked of
//! whatever resolves — and the answers are written down here, because every
//! design decision below rests on them and none of them is documented
//! behaviour the crate promises to keep.
//!
//! **1. Does `Esc` in insert mode move to normal, and does it emit
//! `Action::Cancel`?** It moves to Normal, and it emits nothing at all: no
//! `Cancel`, no edits. The cursor moves one character left, as Vim does on
//! leaving insert. `Action::Cancel` comes from `Ctrl+C` only, and only in
//! Normal mode — in insert mode `Ctrl+C` is a mode change and nothing else.
//!
//! **2. Does `Enter` in insert mode emit `Action::Submit`?** No. In insert mode
//! `Enter` is a newline, and `Shift+Enter` is the same newline, because
//! insert mode does not look at the shift modifier at all. `Action::Submit`
//! comes from `Enter` in **Normal** mode without shift, and `Enter` with shift
//! in Normal mode is a newline *and* a move to Insert.
//!
//! **3. What does `status()` return?** Seven strings, all upper case:
//! `"NORMAL"`, `"INSERT"`, `"d..."`, `"c..."`, `"y..."`, `"VISUAL"`,
//! `"r..."` — plus `"COMMAND"`, which is unreachable because the editor is
//! never built with its command mode enabled. The three dotted ones are an
//! operator waiting for its motion, and are the only reason the wrapper
//! compares against `Normal` rather than against a list.
//!
//! **4. Do `j` and `k` in Normal mode emit `HistoryPrev` / `HistoryNext`?**
//! Yes, on a single-line buffer, always: `k` at the first line is history
//! previous and `j` at the last line is history next, and a one-line buffer is
//! on both. The `history` cargo feature is off and the plan puts history out of
//! scope, so both are refused here — which means the caret in a message's
//! **normal** mode moves horizontally only, and moves between lines from
//! insert mode, where `Up` and `Down` are motions. That is the same answer
//! readline gives, and it is why the line's normal-mode hint names no `j`/`k`.
//!
//! Two more facts the code depends on, found while answering the above:
//!
//! - `EditResult::edits` must be applied **in reverse**. The field is
//!   documented as "in order" and its own crate's example applies them
//!   backwards; the second is what the crate needs, because `r` followed by a
//!   character emits `Insert` and then `Delete` over the same span, and only
//!   the reverse order leaves the character in the buffer.
//! - `reset()` keeps the editor's internal yank buffer, so a line's own `p`
//!   survives a submit. The conversation's register is a different thing and
//!   does not survive a conversation change; see `crate::app::Register`.

#[cfg(test)]
mod tests {
    use vim_line::{Action, Key, KeyCode as VKey, LineEditor as _, VimLineEditor};

    /// The editor, as the wrapper will hold it.
    fn editor() -> VimLineEditor {
        VimLineEditor::new()
    }

    // ---- 1. Esc in insert mode -------------------------------------------

    #[test]
    fn escape_in_insert_leaves_insert_and_asks_for_nothing() {
        let mut editor = editor();
        let text = "hi";

        let out = editor.handle_key(
            Key {
                code: VKey::Escape,
                ctrl: false,
                alt: false,
                shift: false,
            },
            text,
        );

        assert_eq!(editor.status(), "NORMAL", "and it is insert it left");
        assert_eq!(out.action, None, "no Cancel, and no Submit either");
        assert!(out.edits.is_empty(), "and no edit to the text");
    }

    #[test]
    fn escape_in_normal_asks_for_nothing_either() {
        let mut editor = editor();
        let out = editor.handle_key(Key::code(VKey::Escape), "hi");

        assert_eq!(out.action, None);
        assert!(out.edits.is_empty());
    }

    // ---- 2. Enter --------------------------------------------------------

    #[test]
    fn enter_in_insert_is_a_newline_and_never_a_submit() {
        let mut editor = editor();
        editor.handle_key(Key::char('i'), "");
        let text = "a\nb";

        let out = editor.handle_key(Key::code(VKey::Enter), text);

        assert_eq!(out.action, None, "insert mode's Enter is a newline");
        // The caret is where `i` left it on an empty buffer, so the newline
        // goes in front of the text rather than into the middle of it.
        assert_eq!(
            out.edits,
            vec![vim_line::TextEdit::Insert {
                at: 0,
                text: "\n".to_string()
            }]
        );
    }

    #[test]
    fn enter_in_normal_is_a_submit() {
        let mut editor = editor();

        let out = editor.handle_key(Key::code(VKey::Enter), "hi");

        assert_eq!(out.action, Some(Action::Submit));
        assert!(out.edits.is_empty());
    }

    // ---- 3. status -------------------------------------------------------

    #[test]
    fn every_status_is_a_string_the_bar_can_draw() {
        let mut editor = editor();
        let mut seen = vec![editor.status().to_owned()];

        editor.handle_key(Key::char('i'), "hi");
        seen.push(editor.status().to_owned());
        editor.handle_key(Key::code(VKey::Escape), "hi");

        editor.handle_key(Key::char('v'), "hi");
        seen.push(editor.status().to_owned());
        editor.handle_key(Key::code(VKey::Escape), "hi");

        editor.handle_key(Key::char('d'), "hi");
        seen.push(editor.status().to_owned());
        editor.handle_key(Key::code(VKey::Escape), "hi");

        assert_eq!(seen, ["NORMAL", "INSERT", "VISUAL", "d..."]);
    }

    // ---- 4. history ------------------------------------------------------

    #[test]
    fn j_and_k_on_a_one_line_buffer_are_history_not_motion() {
        let mut editor = editor();

        assert_eq!(
            editor.handle_key(Key::char('k'), "one line").action,
            Some(Action::HistoryPrev),
            "`k` is on the first line of a buffer that has only a first line"
        );
        assert_eq!(
            editor.handle_key(Key::char('j'), "one line").action,
            Some(Action::HistoryNext),
            "and `j` is on the last one"
        );
    }

    /// The other half of the same fact, and the reason the caret can still be
    /// moved between the lines of a multi-line message: in insert mode `Up` and
    /// `Down` are motions, not history.
    #[test]
    fn the_arrow_keys_move_the_caret_in_insert_mode() {
        let mut editor = editor();
        let text = "one\ntwo\nthree";
        editor.handle_key(Key::char('a'), text);
        editor.set_cursor(0, text);

        editor.handle_key(Key::code(VKey::Down), text);
        assert_eq!(editor.cursor(), 4, "down to the second line");
        editor.handle_key(Key::code(VKey::Up), text);
        assert_eq!(editor.cursor(), 0, "and back to the first");
    }

    // ---- the order edits are applied in ----------------------------------

    /// `r` followed by a character emits `Insert` and then `Delete` over the same
    /// span. Applied in the order the field is documented in, the deletion
    /// removes the character that was just inserted and the replace is a no-op;
    /// applied in reverse it is the replacement the reader asked for.
    #[test]
    fn a_replace_only_replaces_when_the_edits_are_applied_in_reverse() {
        let mut editor = editor();
        let text = "abc";

        editor.handle_key(Key::char('r'), text);
        let out = editor.handle_key(Key::char('x'), text);

        let mut forwards = text.to_owned();
        for edit in &out.edits {
            edit.apply(&mut forwards);
        }
        let mut reversed = text.to_owned();
        for edit in out.edits.iter().rev() {
            edit.apply(&mut reversed);
        }

        assert_eq!(forwards, "abc", "in the order the field is documented in");
        assert_eq!(
            reversed, "xbc",
            "and in the order the crate's own example uses"
        );
    }
}
