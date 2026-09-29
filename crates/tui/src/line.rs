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
//! **5. What is in the motion table?** `h l j k 0 ^ $ w b e W B E %`, and
//! `Left`/`Right`/`Home`/`End` beside them. Everything else the normal mode
//! answers is a mode switch, an operator, or a direct deletion — so **`g` and
//! `G` are not motions this crate has**, and an unhandled key is dropped
//! rather than refused, so they arrive here as silence. They are the
//! wrapper's, in [`LineEditor::goto`].
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
//!
//! Four defects. The first three were found by feeding the wrapper multi-byte
//! text the way a French reader would, and the fourth by deleting an emoji.
//! The first is **the crate's word motions index bytes, not characters.** `w`
//! from the start of `"héllo wörld"` lands on byte 1, which is inside the `é`,
//! and a visual selection is `cursor + 1`, so `v w d` asks to slice
//! `text[0..2]` — half a character. With `panic = "abort"` that is the end of
//! the process, and `hello`, `Esc`, `v`, `d` reached the same place on
//! ASCII-free text before the wrapper clamped. `Up`/`Down` share the defect by
//! a different road: the column they carry between lines is counted in bytes.
//!
//! The second is the same fault reached another way: **a visual selection is
//! `cursor + 1`**, so a selection on the last character of a buffer asks to
//! delete one past the end of it. `hello`, `Esc`, `v`, `d` reaches that every
//! time.
//!
//! The third is fatal on its own, and is the one a reader with an emoji in a
//! draft can reach with a single key: **`p` computes its insert position as
//! one *byte* past the caret.** `p` means "paste after the character the caret
//! is on", and on a four-byte emoji that index is inside the character, so
//! `yy` then `p` on `"😀😀"` aborts the process.
//!
//! The fourth is a delete, and it does not abort anything: **the library
//! removes one code point.** It walks to a character boundary and stops, so
//! backspace on `👨‍👩‍👧` leaves the joiner with its last character gone, and
//! backspace on `👍🏽` leaves a `👍` that is one column where the sequence was
//! two. The caret is counted in columns, so it is then sitting on the wrong
//! character. [`LineEditor::apply`] widens the range to the cluster it touches
//! — the same normalising step, one unit further on — and the caret stays a
//! code point. `h` and `l` still step one, a caret may rest inside a cluster,
//! and nothing is deleted across it until a key asks to delete. `x` is that
//! key: it removes the cluster the caret is in.
//!
//! The wrapper therefore never trusts a position it did not snap itself: the
//! cursor is snapped to a character boundary before and after every key, every
//! index an edit carries is snapped on its way into the string
//! ([`LineEditor::apply`]), the caret and the selection the rest of the program
//! reads are snapped views, and visual operators are applied here rather than
//! handed over. What is *refused* is the one place snapping cannot help: a
//! word or vertical motion **behind an operator**, where the motion and the
//! slice to apply it happen inside a single key and there is no moment in
//! between. Everywhere else the word motions run, because there the only thing
//! they can do is move a cursor the next key re-snaps. See [`LineEditor::feed`].
//!
//! ## The wrapper
//!
//! [`LineEditor`] rather than a bare `VimLineEditor` for four reasons, each of
//! which cost something to learn the hard way otherwise: the host owns the text
//! and the library does not, so somebody has to apply the edits; `Enter` belongs
//! to the host and must never reach the editor; not every prompt wants a
//! multi-mode buffer; and the reader needs a real caret, which the library has
//! no opinion about.
//!
//! What it does with the answers above:
//!
//! - `Enter` never reaches the library at all. See [`LineEditor::feed`].
//! - `Esc` is decided here, from `status()` *before* dispatch: in the line's
//!   Normal mode the library is not asked, and the host is told to leave. So
//!   answer 1 cannot reach the rest of the program however 7.8 changes it.
//! - Answer 3 is what `status()` comparisons are made against: Normal is the
//!   one that means "not editing", and everything else — insert, visual, and
//!   the three dotted operator states — is handed to the library.
//! - Answer 4 is refused rather than acted on, because history is out of scope.
//!   The caret's vertical movement is insert mode's arrows instead.
//! - Answer 5 is `gg` and `G` in [`LineEditor::goto`].

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vim_line::{Key, KeyCode as VKey, LineEditor as _, TextEdit, VimLineEditor};

use crate::app::PromptKind;
use crate::grapheme;
use crate::wrap::{columns, wrap_keeping_whitespace};

/// The most characters a composed message may hold.
///
/// Telegram's own limit, over the whole text including its newlines — which is
/// what the framework's `validate_text` checks, since it counts
/// `text.chars().count()` over the same string. The two agree by construction,
/// which is the reason the cap lives here rather than being left to a send that
/// would be refused: a key held down must not be able to grow a buffer past
/// what can be sent.
///
/// Enforced on the *result* of a key rather than on the key, because one key is
/// one keystroke and can insert a whole pasted message.
pub const MESSAGE_LIMIT: usize = 4096;

/// What a key asked of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineVerdict {
    /// Nothing the host needs to do.
    Ignored,

    /// The text changed; a redraw is owed.
    Edited,

    /// The key was refused because it would grow the text past what can be
    /// sent.
    ///
    /// Its own verdict rather than [`LineVerdict::Ignored`] because the reader
    /// pressed a key that did nothing and is owed the reason, which the
    /// conversation's own cap already said before this existed.
    TooLong,

    /// The key was refused because running it would split a character.
    ///
    /// Its own verdict rather than [`LineVerdict::Ignored`] for the same reason
    /// [`LineVerdict::TooLong`] exists: a key that did nothing and says nothing
    /// reads as a hang. The reader pressed a motion the editor cannot run on
    /// this text, and is owed the sentence.
    Refused,

    /// Submit the line for its purpose.
    Submit,

    /// The line's own normal mode was left; the text is kept.
    LeftEditing,
}

/// The text being composed, and the editor working on it.
///
/// Owns the text, because `vim-line` does not: the library calculates edits and
/// the host applies them, and the host is this. Everything the crate cannot be
/// asked about — what `Enter` means, when `Esc` leaves, whether a prompt is a
/// buffer at all — is decided here, which is what keeps the answers in this
/// module's docs from reaching the rest of the program.
pub struct LineEditor {
    text: String,
    editor: VimLineEditor,
    purpose: PromptKind,

    /// The text as it was before the key now being applied.
    ///
    /// A cap refusal has to put the key back entirely, and the edits are
    /// already applied by the time the result is known: a paste the cap
    /// refuses is one edit, but a change of several is not. Keeping the
    /// previous text is what makes the refusal whole rather than partial.
    before: String,

    /// The caret as it was before the key now being applied.
    caret_before: usize,

    /// A `g` waiting for the `g` that completes it, spent by whatever key
    /// comes next.
    ///
    /// The only multi-key state in the wrapper, and it exists because the
    /// library has no `gg` — see [`LineEditor::goto`].
    pending_g: bool,

    /// The text the line's last yank produced, waiting to be taken.
    yanked: Option<String>,
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl LineEditor {
    /// A line with nothing in it, in the state it is left in after a submit.
    #[must_use]
    pub fn new() -> Self {
        Self {
            text: String::new(),
            editor: VimLineEditor::new(),
            purpose: PromptKind::Message,
            before: String::new(),
            caret_before: 0,
            pending_g: false,
            yanked: None,
        }
    }

    /// Opens the line for `purpose`, keeping the draft where the rule says to.
    ///
    /// The rule is one sentence: **changing purpose clears the text unless both
    /// the old purpose and the new one are somewhere to write text.** `Message`
    /// and `Reply` are; `Edit` is not, because the buffer means "an edit of
    /// message 42" and keeping a draft would submit it as that message's new
    /// contents — so `i` after an `e` clears, or the reader who pressed `i` to
    /// start a fresh message gets an edit buffer. A command and a search are a
    /// question being answered, not text being written, so `/` and `:` clear.
    pub fn open(&mut self, purpose: PromptKind) {
        let carried = self.purpose.is_buffer() && purpose.is_buffer() && self.carries(purpose);

        if !carried {
            self.text.clear();
        }

        self.purpose = purpose;
        self.start_inserting();
    }

    /// Opens the line for `purpose` with `text` in it.
    ///
    /// For the one prompt that has an initial content of its own: an edit is
    /// opened with the message being edited, because there is only one right
    /// thing for that buffer to contain.
    pub fn open_with(&mut self, purpose: PromptKind, text: String) {
        self.text = text;
        self.purpose = purpose;
        self.start_inserting();
    }

    /// Whether a draft written for `self.purpose` can become a draft for
    /// `purpose`.
    ///
    /// The half of [`LineEditor::open`]'s rule that is about the pair rather
    /// than about either one: a reply can carry a message forward and a message
    /// can carry a reply forward, but neither can be an edit.
    fn carries(&self, purpose: PromptKind) -> bool {
        !matches!(self.purpose, PromptKind::Edit) && !matches!(purpose, PromptKind::Edit)
    }

    /// Empties the line and forgets what it was for.
    ///
    /// After a submit, and when a prompt is opened that starts a new question.
    /// The editor is reset rather than left where it was, which is what keeps
    /// the next `i` from starting somewhere unexpected.
    pub fn clear(&mut self) {
        self.text.clear();
        self.purpose = PromptKind::Message;
        self.editor.reset();
    }

    /// What has been typed.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether there is a draft at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Takes the text out, leaving an empty line.
    ///
    /// What a submit does with the message: the text leaves here, and the
    /// editor is reset with it, so a send that is refused before it is queued
    /// can put it back with [`LineEditor::restore`].
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.clear();
        text
    }

    /// Puts `text` back into an empty line and starts editing it.
    ///
    /// The other half of [`LineEditor::take`], and it exists for exactly one
    /// reason: a send that was refused has to leave the words in the bar. A
    /// send that was queued has nothing to restore.
    pub fn restore(&mut self, text: String) {
        self.text = text;
        self.start_inserting();
    }

    /// Inserts `text` at the caret, under the same cap as a typed character.
    ///
    /// What `p` in the conversation is: a yank there and a paste here are the
    /// same two halves, and the register is the conversation's, not the line's.
    pub fn insert(&mut self, text: &str) -> LineVerdict {
        if !self.fits(text.chars().count()) {
            return LineVerdict::TooLong;
        }

        self.splice(text);
        LineVerdict::Edited
    }

    /// The rows the text occupies at `width`, and the caret's place among them.
    ///
    /// The same rows the conversation panel lays its messages out with, from the
    /// same function, because the input bar and the conversation wrapping
    /// differently is a bug that only shows up on a long message — with one
    /// exception, [`wrap_keeping_whitespace`], which is where a space the reader
    /// typed keeps the cell it has to be seen in. The rows and their count are
    /// the same either way; a run of spaces is only ever recorded while the row
    /// still has room after it, so nothing is wider for it.
    #[must_use]
    pub fn laid_out(&self, width: u16) -> LaidOut {
        let rows = wrap_keeping_whitespace(&self.text, width);
        let caret = self.caret();

        // A caret on the boundary between two rows is the *start* of the second
        // one, which is where the reader will see it. The exception is the end
        // of the text, which can only be the end of the last row.
        let (row, range) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| caret < row.end)
            .or_else(|| rows.last().map(|row| (rows.len().saturating_sub(1), row)))
            .unwrap_or((0, &(0..0)));

        // A caret is clamped into the row it is reported on rather than slicing
        // a range backwards. With the whitespace kept on the row it ends with,
        // every character of the text is on one row or the other, so there is
        // nothing left to clamp in practice — and a caret in a run of spaces is
        // a cell of its own, which is where it is drawn.
        let at = caret.clamp(range.start, range.end);
        // Columns, not characters: the terminal gives an emoji two cells, and a
        // caret counted in characters is drawn inside the glyph it is on. The
        // same `columns` the row was measured with, over the slice in front of
        // the caret.
        let column = columns(&self.text[range.start..at]);

        LaidOut { rows, row, column }
    }

    /// The caret, as a byte offset into [`LineEditor::text`].
    ///
    /// Always at a character boundary and never past the end. The library does
    /// not promise this — its word motions land inside multi-byte characters —
    /// so the wrapper snaps rather than reports: a caret a caller cannot index
    /// with is not a caret.
    #[must_use]
    pub fn caret(&self) -> usize {
        snap_down(&self.text, self.editor.cursor().min(self.text.len()))
    }

    /// The characters the line's visual mode has selected, as a byte range.
    ///
    /// The same `Option<Range<usize>>` shape the conversation's selection has,
    /// and for the same reason: the bar draws it and the operations act on it,
    /// and one answer is what stops those two disagreeing. The library computes
    /// a selection as `cursor + 1`, which is one byte past a multi-byte
    /// character's first byte, so both ends move to a cluster edge: a highlight
    /// that stopped inside a family would be a cut inside one. The caret stays
    /// a code point; this is the selection, and it is what a delete acts on.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.editor.selection().map(|range| {
            let len = self.text.len();
            let start = grapheme::cluster_start(&self.text, range.start.min(len));
            let end = grapheme::cluster_end(&self.text, range.end.min(len));

            start..end.max(start)
        })
    }

    /// The line's own mode, for the input bar to name.
    ///
    /// The library's own word for it. The bar used to carry its own `Mode` and
    /// that was the wrong shape: a line has a mode of its own, and it is not the
    /// conversation's.
    #[must_use]
    pub fn status(&self) -> &str {
        self.editor.status()
    }

    /// What the line is being asked for.
    #[must_use]
    pub const fn purpose(&self) -> PromptKind {
        self.purpose
    }

    /// Keeps the text and forgets what it was for.
    ///
    /// What closing a conversation does to the draft. The words are the
    /// reader's and belong to no conversation; the subject they were written
    /// against does not survive one being closed, and a draft that kept its
    /// purpose would submit as a reply to a message in a chat that is no longer
    /// open.
    pub fn forget_purpose(&mut self) {
        self.purpose = PromptKind::Message;
    }

    /// Takes the text the line's last yank produced, if there was one.
    ///
    /// The same seam as the conversation's yank: `App` holds one clipboard slot
    /// and both producers fill it, so there is one place a caller drains.
    pub fn take_yanked(&mut self) -> Option<String> {
        self.yanked.take()
    }

    /// Applies a key, and says what the host has to do about it.
    ///
    /// `Enter` is decided here and never handed over. It is the host's key —
    /// this program sends on `Enter` and inserts a newline on `Ctrl+J` or
    /// `Shift+Enter` — and the library's own answer differs by mode, so relying
    /// on it would mean a send that worked in one mode and did nothing in the
    /// other. Not giving it the key at all is also the only way to get
    /// multi-line behaviour, because the library has no notion of "`Enter` is
    /// mine".
    ///
    /// `Esc` is decided here too, from `status()` **before** dispatch. In
    /// insert or visual — or with an operator waiting for its motion — it goes
    /// to the library and the reader is still in the line, so
    /// [`LineVerdict::LeftEditing`] is not returned. In the line's normal mode
    /// the library is not asked at all: the host is told to leave, with the
    /// text kept.
    ///
    /// `Ctrl+J` is the primary newline key, and it is not a workaround: it is
    /// Vim's own line feed, it works in every terminal, and it needs no
    /// keyboard-enhancement protocol. `Shift+Enter` is the same key where the
    /// terminal volunteers the distinction.
    ///
    /// **The limitation, which is data loss wearing a disguise.** A terminal
    /// that does not support the kitty disambiguation protocol — `xterm`
    /// among them — sends `Shift+Enter` as a bare `Enter`, and under the rule
    /// above that *sends the message*. There is no way to detect this from
    /// inside `crossterm`: the terminal says nothing about whether it honoured
    /// the flag. So `Ctrl+J` is advertised first, `Shift+Enter` is advertised
    /// as working "where supported", and the alternative — a bare `Enter`
    /// inserting a newline and something else sending — trades a rare surprise
    /// for a constant one. Do not "fix" it that way.
    ///
    /// **The other limitation is a crash wearing a motion's clothes.** The
    /// library's word motions index bytes rather than characters, so on
    /// multi-byte text they land inside a character — and anything that then
    /// slices there takes the process down with it, because this binary sets
    /// `panic = "abort"`. The wrapper therefore snaps the cursor to a boundary
    /// before and after every key, snaps every index an edit carries before it
    /// reaches the string, applies visual operators itself over a snapped range,
    /// and refuses the one case snapping cannot help: a word or vertical motion
    /// **behind an operator**, where the motion and the slice happen inside a
    /// single key. Elsewhere the word motions run as they do on ASCII. A refused
    /// key says so ([`LineVerdict::Refused`]) and changes nothing, not even the
    /// pending operator: the reader can follow it with a motion that is safe.
    pub fn feed(&mut self, key: KeyEvent) -> LineVerdict {
        // Spent here rather than where it is set, so that `Esc` and `Enter` —
        // which return before any key is dispatched — spend it too. A `g` that
        // survived a second `Esc` would be waiting behind a mode the reader had
        // already left.
        let pending_g = std::mem::take(&mut self.pending_g);

        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // The line feed, under either name a terminal can be relied on to
        // deliver: `Ctrl+J` arrives as a control character and, under the
        // disambiguation protocol, `Shift+Enter` arrives as `Enter` with shift
        // set.
        let newline = matches!(key.code, KeyCode::Enter | KeyCode::Char('j')) && (control || shift);
        if newline {
            return self.newline();
        }

        match key.code {
            KeyCode::Enter => return LineVerdict::Submit,
            // A prompt with no normal mode of its own has nothing to leave, so
            // `Esc` leaves it in one press. A buffer's `Esc` leaves its own
            // normal mode in one and the line in two.
            KeyCode::Esc if !self.purpose.is_buffer() || self.is_normal() => {
                return LineVerdict::LeftEditing;
            }
            _ => {}
        }

        let Some(key) = translate(key) else {
            return LineVerdict::Ignored;
        };

        // The library's motion table is `h l j k 0 ^ $ w b e W B E %` and has no
        // `g` or `G` in it, so the two are answered here rather than handed over
        // and dropped. Normal mode only, because in insert mode they are letters.
        if self.is_normal()
            && let Some(verdict) = self.goto(key, pending_g)
        {
            return verdict;
        }

        // The library reads `text[..cursor]` itself on some motions, so a cursor
        // left off a boundary by the previous key would panic it before this one
        // is even interpreted. In practice the snap after every key keeps the
        // invariant; this keeps it when the text was replaced under the editor.
        self.snap_cursor();

        // Byte-indexed motions on multi-byte text, before they can run.
        if !self.text.is_ascii()
            && let Some(verdict) = self.guard_multibyte(key)
        {
            return verdict;
        }

        self.caret_before = self.caret();
        self.before.clone_from(&self.text);

        let result = self.editor.handle_key(key, &self.text);

        // Backwards, because the crate's own example does and only that makes
        // `rx` replace rather than no-op — see this module's docs. What a
        // delete removed is the cluster, not the code point the library
        // counted, and that is what a yank has to hold. A key that yanked
        // nothing — backspace — still yanks nothing.
        let removed = self.apply_edits(&result.edits);
        let reported = result.yanked;
        self.yanked = if reported.is_some() {
            removed.or(reported)
        } else {
            None
        }
        .or(self.yanked.take());

        // Against the text as it is now, not as it was before the key: the
        // library moves its cursor for the edit before the host applies it, so
        // snapping first would clamp a caret that is already right against text
        // that does not hold it yet. The library promises nothing about where
        // its cursor lands, so the snap is unconditional rather than only after
        // motions: an off-boundary cursor left here is a panic in the *next*
        // key, or a caret the bar cannot draw.
        self.snap_cursor();

        // The cap is on the result of the key rather than on the key, because
        // one key can be a whole pasted message. A refusal puts the text and
        // the caret back whole: the editor has already moved, and a caret left
        // ahead of the text it points into is worse than a key that did
        // nothing.
        if self.text.chars().count() > MESSAGE_LIMIT {
            self.text.clone_from(&self.before);
            self.editor.set_cursor(self.caret_before, &self.text);
            return LineVerdict::TooLong;
        }

        match result.action {
            Some(vim_line::Action::Submit) => LineVerdict::Submit,
            Some(vim_line::Action::HistoryPrev | vim_line::Action::HistoryNext) => {
                LineVerdict::Ignored
            }
            _ if result.edits.is_empty() => LineVerdict::Ignored,
            _ => LineVerdict::Edited,
        }
    }

    /// Whether the line is in its own normal mode.
    ///
    /// Anything else — insert, visual, and the three operator states a status
    /// of `d...`/`c...`/`y...` names — is the reader still editing.
    fn is_normal(&self) -> bool {
        self.editor.status() == "NORMAL"
    }

    /// `gg` and `G` in the line's normal mode, which the library has no key for.
    ///
    /// `vim-line` 7.7's [`dispatch_motion`] is `h l j k 0 ^ $ w b e W B E %`,
    /// and everything else in its normal mode is a mode switch, an operator or
    /// a direct deletion — so `g` and `G` fall out of the end of it and are
    /// silently dropped. They are two motions over the whole draft, which is
    /// what the library does not have: its `0` and `$` are per *row*, and
    /// nothing addresses the buffer. `0` and `$` are left as the library has
    /// them, because a reader who has just pressed `gg` and then `0` means "and
    /// now this row", not "back where I was".
    ///
    /// Only the line's own normal mode. In insert mode `g` and `G` are
    /// letters, and a command or search line never reaches normal mode at all,
    /// so neither needs a case.
    ///
    /// `G` lands **on** the last character rather than past it, which is the
    /// library's own normal-mode invariant (`clamp_to_last_char`, applied to
    /// every motion it dispatches) and the same answer `move_line_end` gives
    /// for `$`. A caret one past the end is a caret the next `h`, `x` or `dd`
    /// would act on wrongly. An empty draft has no last character and goes to
    /// zero.
    ///
    /// `Ignored` because a motion changes no text — every other motion in the
    /// editor returns it, and the bar redraws on the key either way.
    ///
    /// [`dispatch_motion`]: https://docs.rs/vim-line/7.7/vim_line/struct.VimLineEditor.html
    fn goto(&mut self, key: Key, pending_g: bool) -> Option<LineVerdict> {
        if key.ctrl || key.alt {
            return None;
        }
        let VKey::Char(motion) = key.code else {
            return None;
        };

        match (motion, pending_g) {
            ('g', true) => {
                self.editor.set_cursor(0, &self.text);
                Some(LineVerdict::Ignored)
            }
            ('g', false) => {
                self.pending_g = true;
                Some(LineVerdict::Ignored)
            }
            ('G', _) => {
                let last = snap_down(&self.text, self.text.len().saturating_sub(1));
                self.editor.set_cursor(last, &self.text);
                Some(LineVerdict::Ignored)
            }
            _ => None,
        }
    }

    /// Puts the library's cursor back on a character boundary.
    ///
    /// The motions that need this are the word and vertical ones, which count
    /// in bytes: on multi-byte text they stop inside a character, and whatever
    /// runs next — a slice in the library, or the bar drawing the caret —
    /// cannot index there. Snapping back rather than forward, so the cursor
    /// never runs past the end of the text it points into.
    fn snap_cursor(&mut self) {
        let at = self.editor.cursor().min(self.text.len());
        self.editor
            .set_cursor(snap_down(&self.text, at), &self.text);
    }

    /// Refuses the keys the library cannot run on this text, or applies them
    /// here instead.
    ///
    /// Only called for non-ASCII text, which is the only text the byte-indexed
    /// motions can split. `None` means the key is safe to hand over.
    ///
    /// Two cases, and only one of them is a refusal:
    ///
    /// - Behind an operator (`d...` and its siblings) a motion and its slice
    ///   happen inside one key, so nothing can be snapped in between. The
    ///   provably safe answers — leaving, the whole-line doubling, and the
    ///   motions that walk boundaries (`h`, `l`, `0`, `$`, `^`) — go through;
    ///   a word or vertical motion is refused. Anything else cancels the
    ///   operator inside the library without touching the text, so it goes
    ///   through too.
    /// - In visual the operators are applied here, over the snapped range the
    ///   rest of the program already reads, because the library would slice its
    ///   own `cursor + 1`. A word motion is *not* refused: it only moves the
    ///   cursor, and the end of the selection it carries is snapped into the
    ///   range the next operator will read.
    ///
    /// Everywhere else — the line's normal mode above all — nothing is refused,
    /// because a motion that only moves a cursor cannot slice anything: the
    /// cursor is snapped after every key, and the one key that could act on the
    /// position it lands on is an operator, which goes through the first case
    /// above. One emoji in a draft therefore costs the reader nothing but the
    /// emoji.
    ///
    /// A refusal changes nothing, not even a pending operator — the reader can
    /// follow it with a motion that is safe, and does not have to retype the
    /// operator to do so.
    fn guard_multibyte(&mut self, key: Key) -> Option<LineVerdict> {
        if key.ctrl || key.alt {
            return None;
        }
        let VKey::Char(motion) = key.code else {
            // Leaving, and every key that is not a character, never slices.
            return None;
        };

        match self.editor.status() {
            "d..." | "c..." | "y..." => {
                let pending = self.editor.status().chars().next();
                let safe = Some(motion) == pending || matches!(motion, 'h' | 'l' | '0' | '$' | '^');

                if safe || !is_byte_motion(motion) {
                    None
                } else {
                    Some(LineVerdict::Refused)
                }
            }
            "VISUAL" => match motion {
                'd' | 'x' | 'y' | 'c' => Some(self.visual_op(motion)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Applies a visual operator to the snapped selection, without handing the
    /// key to the library.
    ///
    /// The library would slice its own `cursor + 1` range, which on multi-byte
    /// text ends inside a character. The range here is the same one the bar
    /// draws, so what is cut is what was shown — whole clusters only.
    /// Visual is then left the way the `Esc` key would leave it, and `c` enters
    /// insert afterwards, because a change is a deletion the reader keeps
    /// typing after.
    fn visual_op(&mut self, op: char) -> LineVerdict {
        let range = self.selection().unwrap_or_default();
        let cut = self.text[range.clone()].to_owned();

        if op != 'y' && !range.is_empty() {
            self.text.replace_range(range.clone(), "");
            self.yanked = Some(cut);
        } else if op == 'y' && !range.is_empty() {
            self.yanked = Some(cut);
        }

        let _ = self.editor.handle_key(Key::code(VKey::Escape), &self.text);
        self.editor
            .set_cursor(range.start.min(self.text.len()), &self.text);
        if op == 'c' {
            let _ = self.editor.handle_key(Key::char('i'), &self.text);
        }
        self.snap_cursor();

        LineVerdict::Edited
    }

    /// Starts the library in Insert with the caret at the end of the text.
    ///
    /// The only way into Insert is the key that would get a reader there, so
    /// the wrapper presses it. At the end, because a draft the reader is
    /// continuing is written from the end, and an empty buffer has nowhere
    /// else for the caret to be.
    fn start_inserting(&mut self) {
        self.editor.reset();
        self.editor.set_cursor(self.text.len(), &self.text);
        let _ = self.editor.handle_key(Key::char('i'), &self.text);
    }

    /// Inserts a newline at the caret, if this prompt is a buffer.
    ///
    /// A command line and a search line are one row of text each, and a
    /// newline in either is a submission that never got asked for.
    fn newline(&mut self) -> LineVerdict {
        if !self.purpose.is_buffer() {
            return LineVerdict::Ignored;
        }
        if !self.fits(1) {
            return LineVerdict::TooLong;
        }

        self.splice("\n");
        LineVerdict::Edited
    }

    /// Replaces `range` with `text` and leaves the caret after it.
    ///
    /// The host's own edit, and the third of them: `newline` is the second and
    /// `splice` under it is the first. It exists for one caller — the
    /// completion accepting an emoji over the `:query` the reader typed — and
    /// it is on this type rather than in `app.rs` because the text and the
    /// caret are private here, and a `String` handed out to be edited in place
    /// is a `String` that will be. It is not `splice`: `splice` inserts and
    /// this replaces.
    ///
    /// The same three disciplines the library's own edits go through, in
    /// [`LineEditor::apply`] and [`LineEditor::feed`]:
    ///
    /// - the range is widened to the grapheme clusters it touches, because a
    ///   range that ended inside one would cut a family in half;
    /// - the cap is checked on the **result**, and a refusal changes nothing —
    ///   an emoji is one character, so a 4096-character draft cannot absorb one;
    /// - the caret is placed by hand, at `start + text.len()` **in bytes**,
    ///   because the library never saw the key and its cursor is its own.
    ///
    /// [`LineVerdict::Edited`] on success, [`LineVerdict::TooLong`] on a
    /// refusal. Nothing else: there is no third outcome.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> LineVerdict {
        let len = self.text.len();
        let start = grapheme::cluster_start(&self.text, range.start.min(len));
        let end = grapheme::cluster_end(&self.text, range.end.min(len)).max(start);

        let removed = self.text[start..end].chars().count();
        let total = self.text.chars().count() - removed + text.chars().count();
        if total > MESSAGE_LIMIT {
            return LineVerdict::TooLong;
        }

        self.text.replace_range(start..end, text);
        self.editor.set_cursor(start + text.len(), &self.text);
        LineVerdict::Edited
    }

    /// Whether `more` characters can go in on top of what is there.
    fn fits(&self, more: usize) -> bool {
        self.text.chars().count() + more <= MESSAGE_LIMIT
    }

    /// Puts `text` at the caret and leaves the caret after it.
    ///
    /// The wrapper's own insert, used for the two newlines it decides itself.
    /// Its callers have already asked the cap.
    fn splice(&mut self, text: &str) {
        let at = self.caret().min(self.text.len());
        self.text.insert_str(at, text);
        self.editor.set_cursor(at + text.len(), &self.text);
    }

    /// Applies the library's edits, back to front, and returns the text a delete
    /// removed.
    ///
    /// [`LineEditor::apply`] is where an index reaches the string. The return
    /// is `None` when the key deleted nothing — a yank with no edit, or an
    /// insert — so the caller can tell "removed the cluster" from "yanked, and
    /// the library's text is the one to keep".
    fn apply_edits(&mut self, edits: &[TextEdit]) -> Option<String> {
        let mut removed = None;
        // Where a widened delete began, in the text from before it. The caret
        // belongs there: the library left it on the code point it deleted,
        // which after the widening is one code point into a cluster that is
        // gone. An ASCII delete does not widen, and the library's caret stands.
        let mut caret_at = None;
        // The widened range, in those same coordinates. `r` emits an insert at
        // the code point and a delete of it; applied back to front, the insert
        // still speaks in the coordinates from before the delete, and a delete
        // that started earlier than the library asked has moved them.
        let mut hole: Option<(usize, usize)> = None;

        for edit in edits.iter().rev() {
            if let Some(cut) = self.apply(edit, &mut hole) {
                if cut.widened {
                    caret_at = Some(cut.at);
                }
                removed = Some(cut.text);
            }
        }

        if let Some(at) = caret_at {
            self.editor.set_cursor(at.min(self.text.len()), &self.text);
        }

        removed
    }

    /// Applies one of the library's edits to the text the host owns.
    ///
    /// The one place an index the library computes reaches a string. A delete's
    /// range is widened to the clusters it touches, so a key removes the glyph
    /// the reader was on and not one code point of it. An index already on a
    /// cluster edge stays there, which is why an ASCII delete is unchanged and
    /// why a newline — its own cluster — is never widened across. An insert
    /// snaps up to a character boundary, because "after the character the caret
    /// is on" means after the whole of it; `hole` then slides that index back
    /// when the delete just applied took bytes from in front of it.
    ///
    /// Returns the text a delete removed, and where it began.
    fn apply(&mut self, edit: &TextEdit, hole: &mut Option<(usize, usize)>) -> Option<Cut> {
        match edit {
            TextEdit::Delete { start, end } => {
                let len = self.text.len();
                let raw_start = (*start).min(len);
                let raw_end = (*end).min(len);
                if raw_start >= raw_end {
                    return None;
                }

                let start = grapheme::cluster_start(&self.text, raw_start);
                let end = grapheme::cluster_end(&self.text, raw_end);
                if start >= end {
                    return None;
                }

                let text = self.text[start..end].to_owned();
                let widened = start < raw_start || end > raw_end;
                self.text.replace_range(start..end, "");
                *hole = Some((start, end));
                Some(Cut {
                    at: start,
                    text,
                    widened,
                })
            }
            TextEdit::Insert { at, text } => {
                let at = shift_for_hole(*at, *hole);
                self.text.insert_str(snap_up(&self.text, at), text);
                None
            }
        }
    }
}

/// Text a delete removed, once its range covered whole clusters.
struct Cut {
    /// Where the removed text began, in the string from before the delete.
    at: usize,
    text: String,
    /// The range grew past the one the library asked for. The caret it left
    /// is then inside the cluster, and belongs at [`Cut::at`] instead.
    widened: bool,
}

/// The text laid out at a width, and where the caret fell in it.
///
/// The three things the input bar has to know about the geometry of a draft,
/// answered together because they are one measurement: a bar that asked for
/// them separately could be given two widths and disagree with itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaidOut {
    /// The rows the text occupies, as byte ranges. Never empty.
    pub rows: Vec<Range<usize>>,

    /// Which of them the caret is on, counted from the first.
    pub row: usize,

    /// How many columns into that row the caret is.
    pub column: usize,
}

impl LaidOut {
    /// The first row to draw so that the caret is on screen in `height` rows.
    ///
    /// The conversation's own clamping, and the reason it is written here
    /// rather than asked of `crate::rows`: the panel *centres* a cursor and
    /// this keeps one in view, which are different rules that happen to share a
    /// `min`. Sharing a function across them would be a unification of two
    /// things that only look alike, and the panel's behaviour would then be at
    /// the mercy of a line editor's.
    #[must_use]
    pub fn first_row(&self, height: usize) -> usize {
        let height = height.max(1);
        let total = self.rows.len();
        let last = total.saturating_sub(height);

        (self.row + 1).saturating_sub(height).clamp(0, last)
    }
}

/// Where an insert lands once a delete in the same key has widened.
///
/// `at` is in the coordinates from before the delete. An index inside the
/// removed cluster belongs at the cluster's start — that is `r` on a code
/// point of a family, and the replacement has to take the family's place, not
/// a byte that the widening already removed. An index past the cluster shifts
/// back by what was removed. No hole, and the index is unchanged.
fn shift_for_hole(at: usize, hole: Option<(usize, usize)>) -> usize {
    let Some((start, end)) = hole else {
        return at;
    };
    if at >= end {
        at - (end - start)
    } else if at > start {
        start
    } else {
        at
    }
}

/// Walks `at` back to the character it falls in the middle of, if it does.
///
/// The one place two units meet in this module: the library counts positions in
/// bytes but steps some of its motions as though they were characters, so a
/// position it reports is not always one a string can be indexed with. Snapping
/// back rather than forward, so the result never runs past the end of the text.
fn snap_down(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Walks `at` forward to the end of the character it falls in the middle of.
///
/// The sibling of [`snap_down`] and the other half of the same rule. A position
/// one byte into a character has to become a real position, and the only two
/// positions that are unambiguously right are the two ends of the character it
/// is inside. A *cursor* takes the near end ([`snap_down`]); an *insert* takes
/// the far one, because "after the character the caret is on" means after the
/// whole of it. A delete does not come here: it widens to a cluster, in
/// [`LineEditor::apply`]. The end of the text is a boundary of its own.
fn snap_up(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// Whether `c` behind an operator ends in a slice of where the motion landed.
///
/// The word motions — `w`, `b`, `e` and their `WORD` siblings — walk
/// `text.as_bytes()` and classify each byte, so on multi-byte text they stop
/// inside a character; and the vertical ones, because `j` and `k` carry a column
/// counted in bytes between lines and land the slice the same place. Any other
/// character cancels the operator inside the library without touching the text,
/// and is safe for exactly that reason.
///
/// Only behind an operator. Elsewhere these motions merely move a cursor, which
/// is snapped after every key and slices nothing.
fn is_byte_motion(c: char) -> bool {
    matches!(c, 'w' | 'b' | 'e' | 'W' | 'B' | 'E' | 'j' | 'k')
}

/// A key as `vim-line` names it, or `None` for one it has no code for.
///
/// A key the library cannot name is a key it must not be asked about, rather
/// than a key it should be told is something else: mapping `F1` onto `Escape`
/// would give a reader a way out of the line with a key that means nothing on
/// any terminal this program has been run on.
fn translate(key: KeyEvent) -> Option<Key> {
    let code = match key.code {
        KeyCode::Char(c) => VKey::Char(c),
        KeyCode::Esc => VKey::Escape,
        KeyCode::Backspace => VKey::Backspace,
        KeyCode::Delete => VKey::Delete,
        KeyCode::Left => VKey::Left,
        KeyCode::Right => VKey::Right,
        KeyCode::Up => VKey::Up,
        KeyCode::Down => VKey::Down,
        KeyCode::Home => VKey::Home,
        KeyCode::End => VKey::End,
        KeyCode::Enter => VKey::Enter,
        _ => return None,
    };

    Some(Key {
        code,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key, as the terminal delivers it.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A key with modifiers, as the terminal delivers it.
    fn combo(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// A line open for composing, as `i` opens it.
    fn composing() -> LineEditor {
        let mut line = LineEditor::new();
        line.open(PromptKind::Message);
        line
    }

    /// Types `text` one character at a time, the way a reader does.
    fn type_text(line: &mut LineEditor, text: &str) {
        for c in text.chars() {
            line.feed(press(KeyCode::Char(c)));
        }
    }

    /// Feeds every key of `keys`, and answers what the host would.
    ///
    /// The two-stage `Esc` is the thing worth reading here: the first leaves
    /// insert for the line's own normal mode, the second leaves the line. A
    /// helper that stopped at the first would make the headline behaviour
    /// untestable, which is what it is for.
    fn answer(line: &mut LineEditor, keys: &[KeyEvent]) -> Vec<LineVerdict> {
        keys.iter().map(|key| line.feed(*key)).collect()
    }

    // ---- typing ---------------------------------------------------------

    #[test]
    fn typing_appends_and_the_caret_follows_it() {
        let mut line = composing();

        type_text(&mut line, "hello");

        assert_eq!(line.text(), "hello");
        assert_eq!(line.caret(), 5, "a caret is a position, not a decoration");
    }

    #[test]
    fn backspace_removes_exactly_one_character() {
        let mut line = composing();
        type_text(&mut line, "abc");

        line.feed(press(KeyCode::Backspace));

        assert_eq!(line.text(), "ab");
        assert_eq!(line.caret(), 2);
    }

    #[test]
    fn backspace_at_the_start_removes_nothing() {
        let mut line = composing();
        type_text(&mut line, "ab");
        line.feed(press(KeyCode::Backspace));
        line.feed(press(KeyCode::Backspace));

        line.feed(press(KeyCode::Backspace));

        assert_eq!(
            line.text(),
            "",
            "a key that removes nothing is not an error"
        );
    }

    /// The regression that exists today: every keystroke applied exactly once.
    #[test]
    fn every_key_typed_is_applied_exactly_once() {
        let mut line = composing();

        type_text(&mut line, "sq");

        assert_eq!(line.text(), "sq");
    }

    // ---- the caret ------------------------------------------------------

    /// The caret is left on the *last character* by `Esc`, as Vim leaves it, so
    /// a motion from there walks back over the words behind it.
    #[test]
    fn the_caret_moves_within_a_message_and_its_motions_work() {
        let mut line = composing();
        type_text(&mut line, "alpha beta gamma");
        line.feed(press(KeyCode::Esc)); // into the line's own normal mode
        assert_eq!(line.caret(), 15, "on the `a` of gamma, not past it");

        type_text(&mut line, "bb");

        assert_eq!(line.caret(), 6, "two words back is the `b` of beta");
    }

    #[test]
    fn x_removes_the_character_the_caret_is_on() {
        let mut line = composing();
        type_text(&mut line, "abcd");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('b')));
        assert_eq!(line.caret(), 0, "`b` from `d` is the start of the word");

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "bcd");
    }

    #[test]
    fn a_multibyte_character_is_one_character_to_the_editor() {
        let mut line = composing();

        type_text(&mut line, "héllo");

        assert_eq!(line.text(), "héllo");
        assert_eq!(
            line.caret(),
            6,
            "bytes, because that is what indexes a string"
        );
    }

    // ---- Esc, the headline ---------------------------------------------

    #[test]
    fn escape_keeps_the_text_and_enters_the_lines_normal_mode() {
        let mut line = composing();
        type_text(&mut line, "half a th");

        let verdict = line.feed(press(KeyCode::Esc));

        assert_eq!(line.text(), "half a th", "the work is not thrown away");
        assert_eq!(
            line.status(),
            "NORMAL",
            "and the line has a mode of its own"
        );
        assert_ne!(
            verdict,
            LineVerdict::LeftEditing,
            "the reader is still in the line"
        );
    }

    #[test]
    fn a_second_escape_returns_to_the_conversation_with_the_text_kept() {
        let mut line = composing();
        type_text(&mut line, "half a th");

        let verdicts = answer(&mut line, &[press(KeyCode::Esc), press(KeyCode::Esc)]);

        assert_eq!(verdicts[1], LineVerdict::LeftEditing);
        assert_eq!(
            line.text(),
            "half a th",
            "and nothing is lost on the way out"
        );
    }

    #[test]
    fn escape_from_visual_mode_keeps_the_text_and_the_selection_goes() {
        let mut line = composing();
        type_text(&mut line, "abcd");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('l')));
        assert!(line.selection().is_some(), "a selection is being made");

        line.feed(press(KeyCode::Esc));

        assert_eq!(
            line.text(),
            "abcd",
            "leaving visual is not leaving the line"
        );
        assert_eq!(line.selection(), None);
    }

    /// A `:` line has no normal mode to leave, so one `Esc` is enough. Giving
    /// it two would make the reader press a key that does nothing.
    #[test]
    fn a_prompt_line_is_left_by_one_escape() {
        let mut line = LineEditor::new();
        line.open(PromptKind::Command);
        type_text(&mut line, "chat 1");

        let verdict = line.feed(press(KeyCode::Esc));

        assert_eq!(verdict, LineVerdict::LeftEditing);
        assert_eq!(line.text(), "chat 1", "with the query kept");
    }

    // ---- Enter, and the newlines it is not ------------------------------

    #[test]
    fn enter_submits_from_the_lines_normal_mode() {
        let mut line = composing();
        type_text(&mut line, "sent");
        line.feed(press(KeyCode::Esc));

        assert_eq!(line.feed(press(KeyCode::Enter)), LineVerdict::Submit);
    }

    /// The documented deviation from Vim, and the test that says the editor
    /// never sees it: in Normal mode the library's own `Enter` is a submit, so
    /// handing it over would work by accident in one mode and not the other.
    #[test]
    fn enter_never_reaches_the_editor() {
        let mut line = composing();
        type_text(&mut line, "hi");
        line.feed(press(KeyCode::Esc));
        let before = line.caret();

        let verdict = line.feed(press(KeyCode::Enter));

        assert_eq!(verdict, LineVerdict::Submit);
        assert_eq!(line.caret(), before, "the editor's cursor is unmoved by it");
        assert_eq!(line.text(), "hi", "and no newline was inserted");
    }

    #[test]
    fn ctrl_j_inserts_a_newline_and_does_not_submit() {
        let mut line = composing();
        type_text(&mut line, "one");

        let verdict = line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));

        assert_eq!(verdict, LineVerdict::Edited);
        assert_eq!(line.text(), "one\n");
    }

    #[test]
    fn shift_enter_inserts_a_newline() {
        let mut line = composing();
        type_text(&mut line, "one");

        let verdict = line.feed(combo(KeyCode::Enter, KeyModifiers::SHIFT));

        assert_eq!(verdict, LineVerdict::Edited);
        assert_eq!(line.text(), "one\n");
    }

    /// A terminal that does not volunteer the distinction sends `Shift+Enter` as
    /// a bare `Enter`, and under the rule above that *sends the message*. The
    /// only defence is that `Ctrl+J` works everywhere, so it is the key the
    /// bar names first.
    #[test]
    fn ctrl_j_works_where_shift_enter_cannot_be_told_apart() {
        let mut line = composing();
        type_text(&mut line, "one");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "two");

        assert_eq!(
            line.text(),
            "one\ntwo",
            "and no terminal protocol is involved"
        );
    }

    #[test]
    fn a_newline_is_refused_in_a_command_and_in_a_search() {
        for purpose in [PromptKind::Command, PromptKind::Search] {
            let mut line = LineEditor::new();
            line.open(purpose);
            type_text(&mut line, "chat 1");

            let verdict = line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));

            assert_eq!(verdict, LineVerdict::Ignored);
            assert_eq!(line.text(), "chat 1", "{purpose:?} is one row of text");
        }
    }

    #[test]
    fn a_multi_line_message_carries_its_newlines() {
        let mut line = composing();

        type_text(&mut line, "one");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "two");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "three");

        assert_eq!(line.text(), "one\ntwo\nthree");
    }

    // ---- the ambiguity this exists to settle ---------------------------

    /// `v` in a message body is a letter. A reader writing "very" must not be
    /// answered with a mode change halfway through the word — so `v` inserts
    /// in insert mode, and the visual selection is a thing the reader asks for
    /// from the line's own normal mode, one `Esc` away.
    #[test]
    fn v_in_a_message_body_is_the_letter_v() {
        let mut line = composing();

        type_text(&mut line, "very");

        assert_eq!(line.text(), "very");
        assert_eq!(line.status(), "INSERT", "still typing, not selecting");
    }

    #[test]
    fn v_from_the_lines_normal_mode_starts_a_visual_selection() {
        let mut line = composing();
        type_text(&mut line, "abcd");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        assert_eq!(line.caret(), 0, "from the first character");

        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('l')));

        assert_eq!(line.status(), "VISUAL");
        assert_eq!(line.selection(), Some(0..2));
    }

    /// The library computes a visual selection as `cursor + 1`, so a selection
    /// that reaches the last character of the buffer asks to delete one byte
    /// past the end. With `panic = "abort"` that is the end of the process, and
    /// `v` then `d` at the end of a message reaches it every time — so the
    /// wrapper clamps, and this is the test that says so.
    #[test]
    fn a_visual_selection_on_the_last_character_does_not_run_off_the_end() {
        let mut line = composing();
        type_text(&mut line, "hello");
        line.feed(press(KeyCode::Esc));

        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('d')));

        assert_eq!(
            line.text(),
            "hell",
            "the last character, and not one past it"
        );
    }

    #[test]
    fn line_visual_delete_and_yank_work() {
        let mut line = composing();
        type_text(&mut line, "hello");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('l')));

        line.feed(press(KeyCode::Char('d')));

        assert_eq!(line.text(), "llo", "the selection is cut");
        assert_eq!(line.take_yanked().as_deref(), Some("he"));
    }

    #[test]
    fn line_visual_yank_takes_the_text_and_leaves_it() {
        let mut line = composing();
        type_text(&mut line, "hello");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('l')));

        line.feed(press(KeyCode::Char('y')));

        assert_eq!(line.text(), "hello");
        assert_eq!(line.take_yanked().as_deref(), Some("he"));
        assert_eq!(line.take_yanked(), None, "and it is drained once");
    }

    // ---- pasting ---------------------------------------------------------

    /// The regression this whole arrangement exists for. The library computes
    /// `p`'s insert position as one **byte** past the caret, which on a four-byte
    /// emoji is inside the character; the index then reached
    /// `String::insert_str`, which asserts a character boundary, and with
    /// `panic = "abort"` there is no catch. `yy` then `p` on `"😀😀"` took the
    /// process down.
    #[test]
    fn a_paste_with_the_caret_on_an_emoji_does_not_split_it() {
        let mut line = composing();
        type_text(&mut line, "😀😀");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('y')));
        line.feed(press(KeyCode::Char('y')));

        let verdict = line.feed(press(KeyCode::Char('p')));

        assert_ne!(verdict, LineVerdict::Refused);
        assert_eq!(
            line.text(),
            "😀😀😀😀",
            "the whole line pasted after the whole emoji it was on — `yy` yanks a line"
        );
    }

    /// "After the character the caret is on" means after the whole of it, so the
    /// position snaps *up* to the end of the character rather than down to its
    /// start — which is what a bare clamp would give.
    #[test]
    fn a_paste_after_a_character_lands_after_the_whole_character() {
        let mut line = composing();
        type_text(&mut line, "a😀b");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('l')));
        line.feed(press(KeyCode::Char('x')));
        line.take_yanked();

        line.feed(press(KeyCode::Char('p')));

        assert_eq!(line.text(), "ab😀");
        assert_eq!(line.caret(), 2, "the emoji starts at byte 2");
    }

    /// The `P` counterpart, which was never wrong: it uses the caret, which the
    /// wrapper keeps on a boundary. Pinned because it is the pair of a rule, not
    /// because it needs one.
    #[test]
    fn a_paste_before_a_character_lands_before_it() {
        let mut line = composing();
        type_text(&mut line, "a😀b");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('l')));
        line.feed(press(KeyCode::Char('x')));
        line.take_yanked();

        line.feed(press(KeyCode::Char('P')));

        assert_eq!(line.text(), "a😀b", "and the emoji is back where it was");
        assert_eq!(line.caret(), 5, "on the `b` after it");
    }

    /// Vim's rule for where a paste leaves the caret, on a character where "the
    /// last thing pasted" is two columns wide rather than one.
    #[test]
    fn a_paste_leaves_the_caret_on_the_last_thing_pasted() {
        let mut line = composing();
        type_text(&mut line, "😀");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('y')));
        line.feed(press(KeyCode::Char('y')));
        line.take_yanked();

        line.feed(press(KeyCode::Char('p')));

        let at = line.caret();
        assert!(
            line.text().is_char_boundary(at),
            "and on a character, not inside one: {at}"
        );
        assert_eq!(
            &line.text()[at..],
            "😀",
            "which is the last thing pasted, not the first"
        );
    }

    /// The other reproduction of the same crash, through the visual mode's own
    /// `cursor + 1` selection rather than through `p`.
    #[test]
    fn a_visual_paste_of_a_zwj_sequence_does_not_split_it() {
        let mut line = composing();
        type_text(&mut line, "👨‍👩‍👧");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('y')));
        line.feed(press(KeyCode::Char('y')));
        line.take_yanked();

        line.feed(press(KeyCode::Char('p')));

        assert!(!line.text().is_empty());
        assert!(
            line.text().is_char_boundary(line.caret()),
            "the family was pasted whole: {:?}",
            line.text()
        );
    }

    // ---- the cap --------------------------------------------------------

    #[test]
    fn a_character_past_the_cap_is_refused_and_says_so() {
        let mut line = composing();
        type_text(&mut line, &"x".repeat(MESSAGE_LIMIT - 1));

        let verdict = line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "x".repeat(MESSAGE_LIMIT));
        assert_eq!(verdict, LineVerdict::Edited);

        let verdict = line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text().chars().count(), MESSAGE_LIMIT);
        assert_eq!(
            verdict,
            LineVerdict::TooLong,
            "a key that did nothing says why"
        );
    }

    /// The cap is over the whole text, newlines included, because that is what
    /// the framework's own `validate_text` counts — and the two agreeing by
    /// construction is the reason the cap lives where the keystroke does.
    #[test]
    fn the_cap_counts_newlines() {
        let mut line = composing();
        type_text(&mut line, &"x".repeat(MESSAGE_LIMIT));

        let verdict = line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));

        assert_eq!(verdict, LineVerdict::TooLong);
        assert_eq!(line.text(), "x".repeat(MESSAGE_LIMIT));
    }

    /// A paste is one keystroke and can be the whole limit at once, so the cap
    /// has to be on the result of the key. Enforcing it per keystroke would let
    /// a paste through and refuse it at the send, which is the one place a
    /// reader cannot fix it.
    #[test]
    fn a_paste_past_the_cap_is_refused_whole() {
        let mut line = composing();
        type_text(&mut line, &"x".repeat(MESSAGE_LIMIT - 10));

        let verdict = line.insert(&"y".repeat(11));

        assert_eq!(verdict, LineVerdict::TooLong);
        assert_eq!(line.text().chars().count(), MESSAGE_LIMIT - 10, "all of it");
    }

    #[test]
    fn a_paste_goes_in_at_the_caret_and_leaves_it_after_the_paste() {
        let mut line = composing();
        type_text(&mut line, "ac");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('i')));

        line.insert("b");

        assert_eq!(line.text(), "bac", "in at the caret, not at the end");
        assert_eq!(line.caret(), 1, "and the caret is after the paste");
    }

    // ---- what each opener does to the text ------------------------------

    #[test]
    fn a_command_and_a_search_start_from_nothing() {
        let mut line = composing();
        type_text(&mut line, "half a th");

        line.open(PromptKind::Search);
        assert_eq!(line.text(), "", "a new query is a new question");

        line.open(PromptKind::Command);
        assert_eq!(line.text(), "");
    }

    #[test]
    fn a_reply_carries_a_message_forward() {
        let mut line = composing();
        type_text(&mut line, "half a th");

        line.open(PromptKind::Reply);

        assert_eq!(
            line.text(),
            "half a th",
            "a half-written message is not garbage"
        );
    }

    /// The §3.1 rule, both directions. `e` replaces because the buffer's
    /// *meaning* changes from "a message" to "an edit of message 42", so
    /// keeping the old contents would submit them as that message's text; and
    /// `i` after an edit must clear, or the reader who pressed `i` to start a
    /// fresh message gets an edit buffer.
    #[test]
    fn an_edit_cannot_carry_a_draft_and_a_draft_cannot_become_an_edit() {
        let mut line = composing();
        type_text(&mut line, "half a th");

        line.open(PromptKind::Edit);
        assert_eq!(
            line.text(),
            "",
            "an edit names a target, so it starts empty"
        );

        line.open_with(PromptKind::Edit, "what the server has".to_owned());
        line.open(PromptKind::Message);
        assert_eq!(line.text(), "", "and `i` after an edit starts fresh");
    }

    #[test]
    fn an_edit_is_opened_with_the_message_it_is_editing() {
        let mut line = composing();

        line.open_with(PromptKind::Edit, "typo heer".to_owned());

        assert_eq!(line.text(), "typo heer");
        assert_eq!(line.caret(), 9, "with the caret at the end, as Vim's `A`");
    }

    // ---- taking the text out --------------------------------------------

    #[test]
    fn taking_the_text_leaves_an_empty_line_and_a_fresh_editor() {
        let mut line = composing();
        type_text(&mut line, "sent");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('v')));

        let text = line.take();

        assert_eq!(text, "sent");
        assert!(line.is_empty());
        assert_eq!(
            line.status(),
            "NORMAL",
            "the editor was reset, not left behind"
        );
    }

    /// A send refused before it is queued has to leave the words in the bar: the
    /// alternative is that pressing `Enter` on a full conversation loses a
    /// message the reader has already written.
    #[test]
    fn text_taken_and_put_back_is_the_text() {
        let mut line = composing();
        type_text(&mut line, "unsent");

        let text = line.take();
        line.restore(text);

        assert_eq!(line.text(), "unsent");
        assert_eq!(line.status(), "INSERT", "and the caret is back in the text");
    }

    // ---- the layout -----------------------------------------------------

    #[test]
    fn a_draft_lays_out_into_the_rows_it_needs() {
        let mut line = composing();
        type_text(&mut line, "alpha beta");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "gamma");

        let laid_out = line.laid_out(20);

        assert_eq!(
            laid_out.rows.len(),
            2,
            "one row a line, at a width that fits both"
        );
        assert_eq!(laid_out.row, 1, "and the caret is on the second");
        assert_eq!(laid_out.column, 5, "five characters in");
    }

    #[test]
    fn a_draft_wider_than_the_bar_wraps_and_the_caret_is_where_it_lands() {
        let mut line = composing();
        type_text(&mut line, "alpha beta gamma delta");

        let laid_out = line.laid_out(11);

        assert_eq!(laid_out.rows.len(), 2);
        assert_eq!(
            laid_out.row, 1,
            "the caret is on the wrapped row, not the first"
        );
        assert_eq!(laid_out.column, 11, "at the end of `gamma delta`");
    }

    /// A column is a cell. An emoji is two of them, so a caret behind one is
    /// two columns in rather than one — a caret counted in characters is drawn
    /// *inside* the glyph it is on.
    #[test]
    fn the_caret_column_accounts_for_a_wide_character() {
        let mut line = composing();
        type_text(&mut line, "a😀");

        let laid_out = line.laid_out(20);

        assert_eq!(laid_out.column, 3, "the cell to the right of the emoji");
    }

    /// A family is two columns, the same two the row is. The caret did not
    /// start counting the scalars the row used to.
    #[test]
    fn the_caret_column_is_the_same_before_and_after_this_change() {
        let mut line = composing();
        type_text(&mut line, "👨‍👩‍👧");

        let laid_out = line.laid_out(20);
        let row = &line.text()[laid_out.rows[laid_out.row].clone()];

        assert_eq!(laid_out.column, 2);
        assert_eq!(columns(row), laid_out.column);
        assert_eq!(row, "👨‍👩‍👧");
    }

    /// The caret and its row are both [`columns`] of a slice, so a caret on a
    /// cluster boundary — here, the end of the text — is on the row it was
    /// assigned to. A family is two columns, the same two the terminal draws.
    #[test]
    fn a_caret_is_never_past_the_row_it_is_on() {
        let mut line = composing();
        type_text(&mut line, &"👨‍👩‍👧".repeat(4));

        for width in [4_u16, 6, 10, 20] {
            let laid_out = line.laid_out(width);
            let row = &laid_out.rows[laid_out.row];
            assert!(
                laid_out.column <= columns(&line.text()[row.clone()]),
                "{laid_out:?} puts the caret past its own row at width {width}"
            );
        }
    }

    #[test]
    fn an_empty_draft_is_still_one_row_with_the_caret_in_it() {
        let laid_out = LineEditor::new().laid_out(20);

        assert_eq!(laid_out.rows, vec![0..0]);
        assert_eq!((laid_out.row, laid_out.column), (0, 0));
    }

    #[test]
    fn the_viewport_scrolls_only_enough_to_keep_the_caret_on_screen() {
        let mut line = composing();
        for _ in 0..10 {
            type_text(&mut line, "a line of text");
            line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        }
        type_text(&mut line, "last");

        let laid_out = line.laid_out(20);
        let height = 3;

        assert_eq!(laid_out.first_row(height), laid_out.row + 1 - height);
        assert!(laid_out.row >= laid_out.first_row(height));
        assert!(laid_out.row < laid_out.first_row(height) + height);
    }

    #[test]
    fn a_draft_that_fits_is_not_scrolled_at_all() {
        let mut line = composing();
        type_text(&mut line, "one\ntwo");

        assert_eq!(line.laid_out(40).first_row(6), 0);
    }

    #[test]
    fn a_viewport_taller_than_the_draft_starts_at_the_first_row() {
        let mut line = composing();
        type_text(&mut line, "one");

        assert_eq!(line.laid_out(40).first_row(6), 0);
    }

    // ---- keys the library has no name for -------------------------------

    #[test]
    fn a_key_with_no_code_is_not_handed_over() {
        let mut line = composing();
        type_text(&mut line, "hi");

        let verdict = line.feed(press(KeyCode::F(1)));

        assert_eq!(verdict, LineVerdict::Ignored);
        assert_eq!(line.text(), "hi", "and it is not mistaken for another key");
    }

    #[test]
    fn a_tab_is_not_a_character_of_the_message() {
        let mut line = composing();

        let verdict = line.feed(press(KeyCode::Tab));

        assert_eq!(verdict, LineVerdict::Ignored);
        assert_eq!(line.text(), "", "Tab moves between panes, and never has");
    }

    // ---- gg and G, which the library has no key for ---------------------

    /// The line in its own normal mode, with `text` in it and the caret at the
    /// end of it — where a reader who has just finished typing stands.
    fn normal_with(text: &str) -> LineEditor {
        let mut line = composing();
        type_text(&mut line, text);
        line.feed(press(KeyCode::Esc));
        line
    }

    /// Feeds the keys of `keys` as presses, for the tests that want to spell a
    /// motion out key by key.
    fn keys(line: &mut LineEditor, keys: &str) -> Vec<LineVerdict> {
        answer(
            line,
            &keys
                .chars()
                .map(|c| press(KeyCode::Char(c)))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn gg_puts_the_caret_at_the_start_of_the_draft() {
        let mut line = normal_with("hello");
        // `Esc` leaves the caret on the last character and the library holds it
        // there, so `0` is how a normal-mode caret travels right in a draft this
        // short — which is the reason the two motions are worth telling apart.
        keys(&mut line, "0ll");
        assert_eq!(line.caret(), 2, "moved off the character it started on");

        keys(&mut line, "gg");

        assert_eq!(line.caret(), 0);
    }

    /// `G` lands **on** the last character, because the library's normal mode
    /// holds the caret on a character and never past the last one — the same
    /// invariant `$` obeys. A caret one past the end is one the next `h`, `x` or
    /// `dd` would act on wrongly.
    ///
    /// The `0` first is load-bearing: `Esc` leaves the caret on the last
    /// character, so a `G` from there and no `G` at all are the same caret, and
    /// a test written without it would pass against a build with no `G` in it.
    #[test]
    fn upper_g_puts_the_caret_on_the_last_character() {
        let mut line = normal_with("hello");
        keys(&mut line, "0");
        assert_eq!(line.caret(), 0, "which is not where G should leave it");

        keys(&mut line, "G");

        assert_eq!(line.caret(), line.text().len() - 1, "not past the end");
    }

    /// The four keys that address a position, and the two answers they must not
    /// blur into each other: `0` and `$` are the library's and are scoped to the
    /// caret's **row**; `gg` and `G` are the wrapper's and address the **whole
    /// draft**. A reader who has just pressed `gg` and then `0` means "and now
    /// this row", not "back where I was".
    #[test]
    fn zero_and_dollar_stay_on_the_caret_row_and_gg_and_upper_g_do_not() {
        let mut line = composing();
        type_text(&mut line, "abc");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "def");
        line.feed(press(KeyCode::Esc));

        keys(&mut line, "0");
        assert_eq!(line.caret(), 4, "the first character of the second row");

        keys(&mut line, "$");
        assert_eq!(line.caret(), 6, "the end of this row, which is the end");

        keys(&mut line, "0");
        assert_eq!(
            line.caret(),
            4,
            "and 0 is the start of it, not of the draft"
        );

        keys(&mut line, "gg");
        assert_eq!(line.caret(), 0, "gg is the start of the draft");

        keys(&mut line, "G");
        assert_eq!(line.caret(), 6, "and G is the end of it");
    }

    /// A `g` is a prefix, and a prefix the reader does not complete must cost
    /// the next key nothing — `g` then `x` is `x`, and the `g` after it starts
    /// over rather than completing a sequence three keys old.
    #[test]
    fn a_lone_g_costs_the_next_key_nothing() {
        let mut line = normal_with("abc");

        keys(&mut line, "gx");
        assert_eq!(line.text(), "ab", "x deleted, as if no g came first");

        keys(&mut line, "gg");
        assert_eq!(line.caret(), 0, "and the g after it was a fresh prefix");
    }

    /// `Esc` spends a pending `g` like any other key. A prefix that survived it
    /// would be waiting behind a mode the reader had already left, and the `g`
    /// of the next `gg` would be spent completing nothing.
    /// `Esc` spends a pending `g` like any other key.
    ///
    /// `Esc` from the line's own normal mode returns before any key is
    /// dispatched, so it is the one key that can leave the prefix standing —
    /// and a `g` that outlived it would complete a sequence the reader had
    /// already abandoned, taking the caret somewhere they did not ask to go.
    ///
    /// The keys are fed to the editor directly, because the host has stopped
    /// addressing the line by the time `Esc` has been answered: leaving the line
    /// is `App`'s business, and what is under test is the wrapper's own promise
    /// about its state.
    #[test]
    fn a_g_left_pending_is_spent_by_esc() {
        let mut line = normal_with("abc");
        assert_eq!(line.caret(), 2, "which is where the reader was");

        keys(&mut line, "gg");
        assert_eq!(line.caret(), 0, "a pair of them moves the caret");

        keys(&mut line, "l");
        line.feed(press(KeyCode::Char('g')));
        line.feed(press(KeyCode::Esc));
        keys(&mut line, "g");

        assert_eq!(
            line.caret(),
            1,
            "this g is a prefix, so the g before the Esc was spent"
        );
    }

    /// In insert mode they are letters, which is the whole reason `goto` is
    /// reached only from the line's own normal mode.
    #[test]
    fn g_and_upper_g_are_ordinary_letters_while_inserting() {
        let mut line = composing();

        type_text(&mut line, "gG");

        assert_eq!(line.text(), "gG");
    }

    /// The last character of a draft that has none: `len - 1` underflows, and
    /// `saturating_sub` is what keeps the caret at zero rather than at a
    /// position no string can be indexed with. It passes with or without the
    /// feature — it guards the arithmetic, not the motion.
    #[test]
    fn upper_g_on_an_empty_draft_does_not_panic() {
        let mut line = normal_with("");

        keys(&mut line, "G");
        assert_eq!(line.caret(), 0);

        keys(&mut line, "gg");
        assert_eq!(line.caret(), 0);
    }

    /// The last byte of a draft is inside its last character on any text with an
    /// emoji in it, and a caret there is one the bar cannot draw or an operator
    /// cannot slice. `G` snaps to the character, as every other position the
    /// wrapper reports does.
    #[test]
    fn upper_g_lands_on_a_character_boundary_of_non_ascii_text() {
        let mut line = normal_with("héllo");
        keys(&mut line, "0");

        keys(&mut line, "G");

        assert!(
            line.text().is_char_boundary(line.caret()),
            "the caret is somewhere a string can be indexed: {}",
            line.caret()
        );
        assert_eq!(
            line.text().get(line.caret()..),
            Some("o"),
            "the last character"
        );

        line.feed(press(KeyCode::Char('x')));
        assert_eq!(line.text(), "héll", "and x removes it whole");
    }

    // ---- multi-byte text, and the motions that cannot run on it ----------
    //
    // They all can, except behind an operator: see the last three here.

    /// The library's word motions index bytes, so on `"héllo"` a `w` lands
    /// inside the `é` — and where it lands inside a character, the snap after
    /// the key walks it back to that character's start, which is the character
    /// the motion was reaching for. On its own a word motion only moves the
    /// cursor, so it runs.
    #[test]
    fn a_word_motion_runs_on_text_holding_an_emoji() {
        let mut line = composing();
        type_text(&mut line, "héllo wörld");
        line.feed(press(KeyCode::Esc));

        let verdict = line.feed(press(KeyCode::Char('w')));

        assert_ne!(verdict, LineVerdict::Refused);
        assert_eq!(line.text(), "héllo wörld", "a motion is not an edit");
        assert_eq!(line.status(), "NORMAL");
        assert!(
            line.text().is_char_boundary(line.caret()),
            "and the caret is somewhere a string can be indexed: {}",
            line.caret()
        );
    }

    /// Emoji are not the interesting case on their own: a character of any
    /// width takes the same road, and the one the reader is most likely to type
    /// is an accented letter.
    #[test]
    fn a_word_motion_lands_on_the_word_after_an_emoji() {
        let mut line = composing();
        type_text(&mut line, "😀 word");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));

        line.feed(press(KeyCode::Char('w')));

        assert_eq!(line.caret(), 5, "the `w` of `word`, past the emoji");
        assert_eq!(line.text(), "😀 word");
    }

    /// One emoji is not a reason to take the motions away from every other
    /// sentence in the draft, for the rest of its life.
    #[test]
    fn one_emoji_does_not_refuse_the_motions_of_the_rest_of_the_draft() {
        let mut line = composing();
        type_text(&mut line, "hello 😀 world");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        let start = line.caret();

        let first = line.feed(press(KeyCode::Char('w')));
        let one = line.caret();
        let second = line.feed(press(KeyCode::Char('w')));
        let two = line.caret();

        assert_ne!(
            (first, second),
            (LineVerdict::Refused, LineVerdict::Refused)
        );
        assert!(
            one > start && two > one,
            "two real moves: {start} -> {one} -> {two}"
        );
    }

    /// A word motion in visual moves the cursor *and* the end of the selection,
    /// so what is yanked afterwards is the drawn selection — over whole
    /// characters, which is the range the wrapper already snaps.
    #[test]
    fn a_visual_word_motion_extends_the_selection_by_whole_characters() {
        let mut line = composing();
        type_text(&mut line, "héllo wörld");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('w')));
        let small = line.selection().expect("a selection is up");

        line.feed(press(KeyCode::Char('w')));
        let wide = line.selection().expect("a selection is still up");
        let grew = wide.end > small.end;
        let text = line.text().to_owned();
        let drawn = &text[wide];
        line.feed(press(KeyCode::Char('y')));

        assert!(grew, "a word motion moves the end too");
        assert_eq!(
            drawn.to_owned(),
            line.take_yanked().expect("`y` yanks the selection"),
            "what is cut is what was drawn, over whole characters"
        );
    }

    /// Behind an operator the motion and its slice happen inside one key, so a
    /// word motion there is not merely misplaced but a panic. It is refused —
    /// and the operator is kept, so the reader can follow it with a motion that
    /// is safe rather than retyping the operator.
    #[test]
    fn a_word_motion_behind_an_operator_is_refused_and_the_operator_is_kept() {
        let mut line = composing();
        type_text(&mut line, "héllo wörld");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('d')));
        assert_eq!(line.status(), "d...");

        let verdict = line.feed(press(KeyCode::Char('w')));

        assert_eq!(verdict, LineVerdict::Refused);
        assert_eq!(line.status(), "d...", "the operator is still waiting");
        assert_eq!(line.text(), "héllo wörld");

        line.feed(press(KeyCode::Char('0')));

        assert_eq!(
            line.text(),
            "d",
            "`d0` walks a boundary and is safe to run — and it ran, so the operator survived the refusal"
        );
    }

    /// The guard that survived the relaxation of the other two cases, said the
    /// other way round: nothing else in this module may have started refusing,
    /// or a half-applied edit may have started leaking. A refused key is a key
    /// that changed nothing at all.
    #[test]
    fn a_refused_motion_behind_an_operator_leaves_the_text_alone() {
        let mut line = composing();
        type_text(&mut line, "héllo wörld 😀");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('d')));
        let before = line.text().to_owned();
        let caret = line.caret();

        for key in ['w', 'e', 'b', 'W', 'j', 'k'] {
            let verdict = line.feed(press(KeyCode::Char(key)));

            assert_eq!(verdict, LineVerdict::Refused, "`d{key}`");
            assert_eq!(line.text(), before, "no partial edit");
            assert_eq!(line.caret(), caret, "and the caret did not move");
            assert_eq!(line.status(), "d...", "the operator is still pending");
        }
    }

    /// The whole-line doubling never leaves a line, so it never meets the byte
    /// motions — and refusing it would take `dd` away from every reader who
    /// writes in French.
    #[test]
    fn a_doubled_operator_runs_on_non_ascii_text() {
        let mut line = composing();
        type_text(&mut line, "héllo");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('d')));

        line.feed(press(KeyCode::Char('d')));

        assert_eq!(line.text(), "");
        assert_eq!(line.take_yanked().as_deref(), Some("héllo"));
    }

    /// A vertical motion behind an operator carries a column counted in bytes,
    /// so it is refused for the same reason a word motion is.
    #[test]
    fn a_vertical_motion_behind_an_operator_is_refused() {
        let mut line = composing();
        type_text(&mut line, "héllo\nwörld");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('d')));

        let verdict = line.feed(press(KeyCode::Char('j')));

        assert_eq!(verdict, LineVerdict::Refused);
        assert_eq!(line.text(), "héllo\nwörld");
    }

    /// Between lines on its own a vertical motion only moves the cursor, so it
    /// runs — and the snap puts the cursor back on a boundary, because the
    /// column it carried is in bytes.
    #[test]
    fn moving_between_lines_of_multibyte_text_keeps_a_usable_caret() {
        let mut line = composing();
        type_text(&mut line, "héllo");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "wörld");
        line.feed(press(KeyCode::Up));

        assert!(line.text().is_char_boundary(line.caret()));
        assert_eq!(line.text(), "héllo\nwörld", "and moving never edits");

        line.feed(press(KeyCode::Char('x')));

        assert!(
            line.text().is_char_boundary(line.caret()),
            "and whatever follows it does not split a character: {:?}",
            line.text()
        );
    }

    /// The library slices its own `cursor + 1` for a visual operator, which on
    /// multi-byte text ends inside a character. The wrapper applies the
    /// operator itself over the snapped range it draws, so what is cut is what
    /// was shown.
    #[test]
    fn a_visual_delete_on_multibyte_text_cuts_whole_characters() {
        let mut line = composing();
        type_text(&mut line, "héllo");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('l')));
        line.feed(press(KeyCode::Char('l')));

        line.feed(press(KeyCode::Char('d')));

        assert_eq!(line.text(), "lo");
        assert_eq!(line.take_yanked().as_deref(), Some("hél"));
        assert_eq!(line.status(), "NORMAL", "and visual is left");
    }

    /// A visual change is a deletion the reader keeps typing after.
    #[test]
    fn a_visual_change_on_multibyte_text_enters_insert() {
        let mut line = composing();
        type_text(&mut line, "héllo");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('l')));
        line.feed(press(KeyCode::Char('l')));

        line.feed(press(KeyCode::Char('c')));

        assert_eq!(line.text(), "lo");
        assert_eq!(line.status(), "INSERT");
    }

    // ---- a delete removes a cluster -------------------------------------

    /// Two backspaces over a family and the character after it. One code point
    /// at a time would leave the joiner with its last character gone.
    #[test]
    fn a_backspace_removes_a_whole_zwj_family() {
        let mut line = composing();
        type_text(&mut line, "👨‍👩‍👧x");

        line.feed(press(KeyCode::Backspace));
        line.feed(press(KeyCode::Backspace));

        assert_eq!(line.text(), "");
        assert_eq!(line.caret(), 0);
        assert_eq!(line.take_yanked(), None, "backspace does not yank");
    }

    /// The same text, from the start, with `x`. The character after the family
    /// is not part of the cluster and stays.
    #[test]
    fn x_removes_a_whole_zwj_family() {
        let mut line = composing();
        type_text(&mut line, "👨‍👩‍👧x");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "x");
        assert_eq!(line.caret(), 0, "where the family was");
    }

    /// A bare thumb is one column; the sequence is two. Removing the modifier
    /// on its own would move every column after it.
    #[test]
    fn a_backspace_removes_the_modifier_with_its_base() {
        let mut line = composing();
        type_text(&mut line, "👍🏽x");
        line.feed(press(KeyCode::Backspace));
        assert_eq!(columns(line.text()), 2, "the sequence, once `x` is gone");

        line.feed(press(KeyCode::Backspace));

        assert_eq!(line.text(), "");
        assert_eq!(
            columns(line.text()),
            0,
            "a bare thumb would still be a column"
        );
    }

    /// A lone regional indicator is a letter. Empty is the only result that
    /// says the flag went as one thing.
    #[test]
    fn a_backspace_removes_a_flag_as_one_thing() {
        let mut line = composing();
        type_text(&mut line, "🇬🇧x");

        line.feed(press(KeyCode::Backspace));
        line.feed(press(KeyCode::Backspace));

        assert_eq!(line.text(), "");
    }

    /// Text presentation of the heart is one column; the VS16 sequence is two.
    #[test]
    fn a_backspace_removes_a_vs16_sequence_as_one_thing() {
        let mut line = composing();
        type_text(&mut line, "❤️x");

        line.feed(press(KeyCode::Backspace));
        line.feed(press(KeyCode::Backspace));

        assert_eq!(line.text(), "");
    }

    /// The caret is on the modifier, and the character after the sequence is
    /// not part of the cluster.
    #[test]
    fn a_delete_widens_to_the_cluster_and_no_further() {
        let mut line = composing();
        type_text(&mut line, "👍🏽z");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Left));
        assert_eq!(line.caret(), "👍".len(), "on the modifier, not the base");

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "z");
        assert_eq!(line.caret(), 0);
    }

    /// `dd` on the last line deletes back over the preceding newline. Widening
    /// that newline would eat the cluster the line before ends with, and
    /// widening the other way would eat the cluster the next line starts with.
    #[test]
    fn dd_still_deletes_a_whole_line_and_not_the_line_before_it() {
        let family = "👨‍👩‍👧";

        let mut line = composing();
        type_text(&mut line, family);
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, "abc");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('d')));
        line.feed(press(KeyCode::Char('d')));

        assert_eq!(line.text(), family, "the cluster before the newline stays");

        let mut line = composing();
        type_text(&mut line, "abc");
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, family);
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('k')));
        line.feed(press(KeyCode::Char('d')));
        line.feed(press(KeyCode::Char('d')));

        assert_eq!(line.text(), family, "and the cluster after it");
    }

    /// `x` on the newline between two clusters removes the line break and
    /// neither cluster.
    #[test]
    fn a_delete_never_crosses_a_newline() {
        let family = "👨‍👩‍👧";
        let thumb = "👍🏽";
        let mut line = composing();
        type_text(&mut line, family);
        line.feed(combo(KeyCode::Char('j'), KeyModifiers::CONTROL));
        type_text(&mut line, thumb);
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('h')));
        assert_eq!(line.caret(), family.len(), "the caret is on the newline");

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), format!("{family}{thumb}"));
    }

    /// The register holds the cluster the key removed. The library's own yank
    /// is the last code point of it, and a reader who pastes that pastes a
    /// stranger.
    #[test]
    fn the_yank_is_what_was_actually_removed() {
        let family = "👨‍👩‍👧";
        let mut line = composing();
        type_text(&mut line, family);
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(
            line.take_yanked().as_deref(),
            Some(family),
            "the family, not its last code point"
        );
    }

    /// After the cluster is gone the caret is where it began, not one code
    /// point into the hole it left.
    #[test]
    fn the_caret_after_a_cluster_delete_is_where_the_cluster_was() {
        let mut line = composing();
        type_text(&mut line, "a👨‍👩‍👧b");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Left));
        assert!(
            line.caret() > 1,
            "inside the family, on its last code point"
        );

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "ab");
        assert_eq!(line.caret(), 1);
    }

    /// `w` lands on the family and the selection's end is one byte into it.
    /// The end drawn, and the end `d` cuts, is the family's own end.
    #[test]
    fn a_visual_selection_ends_on_a_cluster_edge() {
        let family = "👨‍👩‍👧";
        let mut line = composing();
        type_text(&mut line, &format!("a{family}b"));
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('v')));
        line.feed(press(KeyCode::Char('w')));
        let selection = line.selection().expect("a selection is up");
        assert_eq!(selection, 0..1 + family.len());
        let drawn = line.text()[selection].to_owned();

        line.feed(press(KeyCode::Char('d')));

        assert_eq!(line.text(), "b");
        assert_eq!(line.take_yanked().as_deref(), Some(drawn.as_str()));
    }

    /// `h` and `l` stay on code points. A caret that jumped the family would be
    /// a different key.
    #[test]
    fn the_caret_steps_one_code_point_through_a_cluster() {
        let family = "👨‍👩‍👧";
        let mut line = composing();
        type_text(&mut line, family);
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));

        line.feed(press(KeyCode::Char('l')));

        let step = '👨'.len_utf8();
        assert_eq!(line.caret(), step);
        assert!(step < family.len(), "still inside the family");
    }

    /// `r` on ASCII replaces one character. The insert that follows a delete
    /// in the same key still lands where that character was.
    #[test]
    fn a_replace_on_ascii_replaces_one_character() {
        let mut line = composing();
        type_text(&mut line, "abc");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Char('0')));
        line.feed(press(KeyCode::Char('r')));

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "xbc");
        assert_eq!(line.caret(), 0);
    }

    /// `r` on the modifier replaces the whole sequence, and the characters on
    /// either side stay where they are.
    #[test]
    fn a_replace_inside_a_cluster_replaces_the_whole_cluster() {
        let mut line = composing();
        type_text(&mut line, "a👍🏽b");
        line.feed(press(KeyCode::Esc));
        line.feed(press(KeyCode::Left));
        line.feed(press(KeyCode::Char('r')));

        line.feed(press(KeyCode::Char('x')));

        assert_eq!(line.text(), "axb");
        assert_eq!(line.caret(), 1);
    }

    // ---- accepting a completion -----------------------------------------
    //
    // The one caller of `replace`: a `:query` goes in, the glyph it named comes
    // out, and the caret lands after it.

    #[test]
    fn a_replacement_puts_the_caret_after_what_it_wrote() {
        let mut line = composing();
        type_text(&mut line, ":cry");

        let verdict = line.replace(0..4, "😢");

        assert_eq!(verdict, LineVerdict::Edited);
        assert_eq!(line.text(), "😢");
        assert_eq!(line.caret(), 4, "after the glyph, not over it");
    }

    /// 😢 is four bytes, one character and two columns, and only the first of
    /// those three is what a caret into a `String` means. The other two are
    /// wrong answers that happen to look plausible.
    #[test]
    fn the_caret_after_a_replacement_is_a_byte_offset() {
        let mut line = composing();
        type_text(&mut line, ":cry");

        line.replace(0..4, "😢");

        assert_eq!(line.caret(), 4, "four bytes");
        assert_ne!(line.caret(), 1, "not one character");
        assert_ne!(line.caret(), 2, "and not the two columns it draws");
    }

    #[test]
    fn a_replacement_widens_to_the_clusters_it_touches() {
        let family = "👨‍👩‍👧";
        let mut line = composing();
        type_text(&mut line, &format!("a{family}b"));

        line.replace(1..2, "x");

        assert_eq!(
            line.text(),
            "axb",
            "the whole family, not one code point of it"
        );
        assert_eq!(line.caret(), 2);
    }

    #[test]
    fn a_replacement_past_the_message_limit_changes_nothing() {
        let mut line = composing();
        type_text(&mut line, &"x".repeat(MESSAGE_LIMIT));
        let caret = line.caret();

        let verdict = line.replace(MESSAGE_LIMIT..MESSAGE_LIMIT, "😢");

        assert_eq!(verdict, LineVerdict::TooLong);
        assert_eq!(line.text(), "x".repeat(MESSAGE_LIMIT));
        assert_eq!(line.caret(), caret, "the caret did not move");
    }

    #[test]
    fn a_replacement_leaves_the_library_able_to_read_the_text() {
        let mut line = composing();
        type_text(&mut line, ":cry");

        line.replace(0..4, "😢");
        let verdict = line.feed(press(KeyCode::Backspace));

        assert_eq!(verdict, LineVerdict::Edited);
        assert_eq!(line.text(), "", "the editor still knows where the text is");
        assert_eq!(line.caret(), 0);
    }

    // ---- the prompt rule ------------------------------------------------

    #[test]
    fn a_command_and_a_search_are_not_buffers() {
        assert!(PromptKind::Message.is_buffer());
        assert!(PromptKind::Reply.is_buffer());
        assert!(PromptKind::Edit.is_buffer());
        assert!(!PromptKind::Command.is_buffer());
        assert!(!PromptKind::Search.is_buffer());
    }
}

/// The spike's own tests. These ask the crate rather than the wrapper, and are
/// kept because `vim-line = "7.7"` is a caret range: a 7.8 could answer any of
/// them differently, and only a test would say so.
#[cfg(test)]
mod spike {
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

    /// The defect the wrapper guards, stated as a test so that a `vim-line`
    /// that fixes it fails loudly here rather than silently: the word motions
    /// walk `text.as_bytes()` and classify each byte, so on multi-byte text
    /// they stop where no word ends — and `e` stops inside a character. The
    /// wrapper never hands such a key over (see `LineEditor::feed`); if this
    /// test ever fails, the guard — and the refusal, and the flash — can go
    /// with it.
    #[test]
    fn word_motions_index_bytes_not_characters() {
        let mut editor = editor();
        let text = "héllo wörld";

        editor.handle_key(Key::char('0'), text);
        editor.handle_key(Key::char('e'), text);

        assert_eq!(editor.cursor(), 2, "two bytes in");
        assert!(
            !text.is_char_boundary(editor.cursor()),
            "which is inside the `é` (bytes 1..3) — and no string can be indexed there"
        );
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
