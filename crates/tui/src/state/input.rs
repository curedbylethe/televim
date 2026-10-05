//! The live input line and the emoji popup over it.

use crate::emoji;
use crate::line::LineEditor;

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
    /// parked in [`App::drafts`], and every conversation switch moves one out of
    /// here and the next one in.
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
}
