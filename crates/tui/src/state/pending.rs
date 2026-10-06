//! Half-typed keys and the requests a keystroke defers.

use crate::app::{ChatChoice, Find, Jump};

/// Half-typed keys and the requests a keystroke defers.
///
/// The `pending_` prefix stays: call sites are `self.pending.pending_g`.
#[allow(clippy::struct_field_names)]
pub struct Pending {
    /// The jump the reader has asked for and no page has answered yet.
    ///
    /// Set when `gg` cannot be answered from what is loaded, and cleared when the
    /// page arrives — or fails, or comes back empty, because a jump that went
    /// wrong must not wedge the key. It is what makes the key idempotent: a
    /// second `gg` produces the same intent, which the caller recognises as one
    /// already on its way.
    pub(crate) pending_jump: Option<Jump>,

    /// The conversation the highlight has moved onto but has not been taken to.
    ///
    /// The same hand-over as [`App::pending_jump`] — recorded here because
    /// `tui` cannot reach the network — and for the same reason it carries a
    /// time: a reader holding `j` would otherwise fetch every conversation they
    /// scrolled past, and one page per chat as fast as a key repeats is how a
    /// scroll through the list becomes a flood wait.
    pub(crate) pending_chat: Option<ChatChoice>,

    /// Whether a `g` was just pressed in the chat list and a second one would
    /// take the reader to the top of it.
    ///
    /// The same latch as `dd` and for the same reason: `gg` is two presses in
    /// Vim, and a key held down is not two of them.
    pub(crate) pending_g: bool,

    /// A `f`, `t`, `F` or `T` waiting for the character to look for.
    ///
    /// Two keys rather than one, as in Vim, and a latch for the same reason `dd`
    /// has one: the key after `f` is the character, not a motion. The character
    /// itself is not recorded, because it has not been typed yet.
    pub(crate) pending_find: Option<Find>,

    /// The `--chat` id to open once the first chat list arrives.
    ///
    /// Set at launch from the CLI and taken when the list lands, so it applies
    /// once: a later list refresh keeps the reader where they are. Unknown ids
    /// are reported by the caller, not here.
    pub(crate) pending_initial_chat: Option<i64>,
}

impl Pending {
    /// No key half-typed and no request waiting.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            pending_jump: None,
            pending_chat: None,
            pending_g: false,
            pending_find: None,
            pending_initial_chat: None,
        }
    }
}
