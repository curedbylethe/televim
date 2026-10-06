//! The live input line and the emoji popup over it.

use crate::emoji;
use crate::line::{LineEditor, LineVerdict};

/// The live input line and the emoji popup over it.
pub struct InputState {
    /// What the reader is composing, and the editor working on it.
    ///
    /// Was a `String`, and was enough of a design not to notice it was wrong:
    /// append-only, no caret, and cleared by the key every reader presses
    /// reflexively. A line is a buffer with a caret in it, and it is the
    /// wrapper's whole job.
    ///
    /// This is the *open* conversation's draft; the drafts of the rest are
    /// parked in [`DraftStore`](super::drafts::DraftStore), and every
    /// conversation switch moves one out of here and the next one in.
    pub line: LineEditor,

    /// The `:query` being completed, if there is one.
    ///
    /// `None` is the whole of "the popup is closed", and it is reached from five
    /// places: a query that stopped being one, a query nobody matches, the focus
    /// leaving the line, a submit, and an acceptance. There is no flag to fall
    /// out of step with the state it describes.
    pub(crate) emoji: Option<emoji::Trigger>,
}

impl InputState {
    /// An empty line and a closed popup.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            line: LineEditor::new(),
            emoji: None,
        }
    }

    /// Puts `line` on the bar, replacing what was there.
    pub(crate) fn set_line(&mut self, line: LineEditor) {
        self.line = line;
    }

    /// Puts the completion popup away.
    ///
    /// The single clear point for every way out of the line, so the completion
    /// does not need a case in each of them.
    pub(crate) fn dismiss_completion(&mut self) {
        self.emoji = None;
    }

    /// Moves the selected candidate one place, wrapping.
    pub(crate) fn move_completion(&mut self, forward: bool) {
        if let Some(trigger) = &mut self.emoji {
            trigger.move_selection(forward);
        }
    }

    /// Commits the chosen emoji over the `:query` that named it.
    ///
    /// The inserted text is exactly what was accepted: no trailing space,
    /// because a character the reader did not ask for is one they would have to
    /// delete. The range comes from the trigger, so what is replaced is what
    /// the popup was describing.
    ///
    /// Answers with the replace's own verdict. `TooLong` leaves the popup up
    /// and is the host's to say out loud; anything else puts it away.
    pub(crate) fn accept_completion(&mut self) -> LineVerdict {
        let Some(trigger) = self.emoji.as_ref() else {
            return LineVerdict::Ignored;
        };
        let Some(chosen) = trigger.chosen() else {
            return LineVerdict::Ignored;
        };
        let range = trigger.range.clone();
        let text = chosen.as_str();

        match self.line.replace(range, text) {
            LineVerdict::TooLong => LineVerdict::TooLong,
            verdict => {
                self.emoji = None;
                verdict
            }
        }
    }

    /// Re-derives the completion from the draft and the caret.
    ///
    /// The row the reader was on is carried across, clamped into the new list,
    /// so typing one more character does not move them off a candidate that is
    /// still there.
    pub(crate) fn redetect_completion(&mut self, selected: usize) {
        self.emoji = emoji::detect(self.line.text(), self.line.caret());
        if let Some(trigger) = &mut self.emoji {
            trigger.reselect(selected);
        }
    }
}
