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
//!
//! And one defect, found by feeding the wrapper multi-byte text the way a
//! French reader would: **the crate's word motions index bytes, not
//! characters.** `w` from the start of `"héllo wörld"` lands on byte 1, which
//! is inside the `é`, and a visual selection is `cursor + 1`, so `v w d` asks
//! to slice `text[0..2]` — half a character. With `panic = "abort"` that is the
//! end of the process, and `hello`, `Esc`, `v`, `d` reached the same place on
//! ASCII-free text before the wrapper clamped. `Up`/`Down` share the defect by
//! a different road: the column they carry between lines is counted in bytes.
//!
//! The wrapper therefore never trusts a position it did not snap itself: the
//! cursor is snapped to a character boundary before and after every key, the
//! caret and the selection the rest of the program reads are snapped views,
//! visual operators are applied here rather than handed over, and a word motion
//! — or a vertical one behind an operator — on non-ASCII text is refused
//! rather than run. See [`LineEditor::feed`].
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

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vim_line::{Key, KeyCode as VKey, LineEditor as _, TextEdit, VimLineEditor};

use crate::app::PromptKind;
use crate::wrap::wrap_keeping_whitespace;

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
        let column = self.text[range.start..at].chars().count();

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
    /// and one answer is what stops those two disagreeing. Snapped for the same
    /// reason the caret is — the library computes a selection as `cursor + 1`,
    /// which is one byte past a multi-byte character's first byte — and shrunk
    /// rather than grown, so that operating on it can only ever touch whole
    /// characters.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.editor.selection().map(|range| {
            let start = snap_down(&self.text, range.start.min(self.text.len()));
            let end = snap_down(&self.text, range.end.min(self.text.len()));

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
    /// before and after every key, applies visual operators itself over a
    /// snapped range, and refuses a word motion — or a vertical one behind an
    /// operator — on non-ASCII text outright. A refused key says so
    /// ([`LineVerdict::Refused`]) and changes nothing, not even the pending
    /// operator: the reader can follow it with a motion that is safe.
    pub fn feed(&mut self, key: KeyEvent) -> LineVerdict {
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
        self.yanked = result.yanked.or(self.yanked.take());

        // Backwards, because the crate's own example does and only that makes
        // `rx` replace rather than no-op — see this module's docs.
        for edit in result.edits.iter().rev() {
            self.apply(edit);
        }

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
    /// Three cases, by what the line is in the middle of:
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
    ///   own `cursor + 1`. A word motion that would only extend the selection
    ///   into a split character is refused.
    /// - In normal a word motion would only land the cursor somewhere the next
    ///   key cannot use, so it is refused. Motions between lines are left to
    ///   run and snapped afterwards: they move, just not by words.
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
                _ if is_word_motion(motion) => Some(LineVerdict::Refused),
                _ => None,
            },
            "NORMAL" => {
                if is_word_motion(motion) {
                    Some(LineVerdict::Refused)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Applies a visual operator to the snapped selection, without handing the
    /// key to the library.
    ///
    /// The library would slice its own `cursor + 1` range, which on multi-byte
    /// text ends inside a character. The range here is the same snapped one the
    /// bar draws, so what is cut is what was shown — whole characters only.
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

    /// Applies one of the library's edits to the text the host owns.
    ///
    /// Clamped to the text, because the library computes a visual selection as
    /// `cursor + 1` and a selection on the **last** character therefore asks to
    /// delete one past the end — which, with `panic = "abort"`, takes the
    /// process down. `hello`, `Esc`, `v`, `d` reaches it every time. The
    /// library is not wrong about the selection; it is wrong about a range on
    /// the last character of a buffer, and clamping is cheaper than a
    /// `panic = "abort"` the reader cannot catch.
    fn apply(&mut self, edit: &TextEdit) {
        let len = self.text.len();

        match edit {
            TextEdit::Delete { start, end } => {
                let (start, end) = (start.min(&len), end.min(&len));
                if start < end {
                    self.text.replace_range(start..end, "");
                }
            }
            TextEdit::Insert { at, text } => {
                self.text.insert_str((*at).min(len), text);
            }
        }
    }
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

/// Whether `c` is a motion the library counts in bytes.
///
/// `w`, `b` and `e` and their `WORD` siblings walk `text.as_bytes()` and
/// classify each byte, so on multi-byte text they stop inside a character. The
/// rest of the motions — steps, line ends, brackets, the arrows — either walk
/// boundaries or are snapped afterwards without anything slicing in between.
fn is_word_motion(c: char) -> bool {
    matches!(c, 'w' | 'b' | 'e' | 'W' | 'B' | 'E')
}

/// Whether `c` behind an operator ends in a slice of where the motion landed.
///
/// The word motions, and the vertical ones: `j` and `k` carry a column counted
/// in bytes between lines, so behind `d` they land the slice the same place a
/// word motion would. Any other character cancels the operator inside the
/// library without touching the text, and is safe for exactly that reason.
fn is_byte_motion(c: char) -> bool {
    is_word_motion(c) || matches!(c, 'j' | 'k')
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

    // ---- multi-byte text, and the motions that cannot run on it ----------

    /// The library's word motions index bytes, so on `"héllo"` a `w` lands
    /// inside the `é`. Running it would only put the cursor somewhere the next
    /// key cannot use, so it is refused — and the refusal changes nothing, not
    /// even the mode.
    #[test]
    fn a_word_motion_on_non_ascii_text_is_refused() {
        let mut line = composing();
        type_text(&mut line, "héllo wörld");
        line.feed(press(KeyCode::Esc));

        let verdict = line.feed(press(KeyCode::Char('w')));

        assert_eq!(verdict, LineVerdict::Refused);
        assert_eq!(line.text(), "héllo wörld");
        assert_eq!(line.status(), "NORMAL");
        assert_eq!(line.caret(), 12, "still at the end of the text");
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
