//! Dispatch mode, focus, and the chrome that is not conversation or input.

use std::cell::Cell;
use std::time::Instant;

use crate::app::{FLASH_FOR, Focus, Mode, Pane};
use crate::bidi::BidiMode;
use crate::state::connection::ConnectionState;
use crate::theme::Theme;

/// How many message rows the conversation panel is assumed to have before it
/// has been drawn once.
///
/// Only the panel knows the real number, and only during a frame. This is what
/// the key handling falls back on in between, and it is deliberately a normal
/// size rather than a small one: a page that overshoots is clamped.
const ASSUMED_ROWS: usize = 20;

/// How many columns the conversation panel's messages are assumed to have
/// before it has been drawn once.
///
/// The same fallback as [`ASSUMED_ROWS`] and for the same reason: the layout
/// has to be answerable before the first frame.
const ASSUMED_BODY_WIDTH: u16 = 80;

/// What the status line shows before anything has happened.
pub(crate) const IDLE_STATUS: &str = "televim";

/// Whether a decoded sticker paints its picture or its token.
///
/// Inline is the default: a sticker message with bytes draws the bounded
/// block, and one without draws `[sticker]` while its bytes are asked for.
/// Token draws `[sticker]` for every sticker message and never asks — no
/// decode, no fetch traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StickerMode {
    /// Paint the picture (or the token while its bytes are missing).
    #[default]
    Inline,
    /// Always the token, never the picture.
    Token,
}

/// Rows, body width, and the clock, as of the last frame.
///
/// [`Cell`] because a frame is drawn from a shared reference. The panel records
/// what the terminal made of the rectangle, and the key path reads it back.
pub struct FrameMetrics {
    /// How many message rows the conversation panel had room for as of the last
    /// frame.
    ///
    /// A cell rather than a plain field because a frame is drawn from a shared
    /// reference, and the panel is the only place that knows how tall the
    /// terminal made it. It is a measurement rather than state anything decides,
    /// so recording it late is the same as recording it at all.
    pub rows: Cell<usize>,

    /// How many columns the conversation panel's messages had room for as of
    /// the last frame, which is the width the rows are laid out at.
    ///
    /// Recorded beside [`Self::rows`] and for the same reason: only the panel
    /// knows, and the layout cannot be worked out without it. What is given up
    /// for the scrollbar is given up before this, so no message is ever laid
    /// out — or drawn — under the bar.
    pub body_width: Cell<u16>,

    /// The unix second the reader's clock last read, as of the last frame.
    ///
    /// A measurement rather than state anything decides, for the same reason as
    /// [`Self::rows`] and [`Self::body_width`]: only the host owns a clock, and this
    /// crate reads none ([`crate::date`] is pure). Zero means no clock has been
    /// recorded, which the day labels read as "say the date rather than `Today`"
    /// rather than as 1970.
    pub now: Cell<i64>,
}

/// Dispatch mode, focus, and the chrome around the conversation.
pub struct UiState {
    pub mode: Mode,
    pub focus: Focus,
    pub theme: Theme,

    /// What the right-hand pane is showing.
    ///
    /// A field rather than a variant of [`Focus`], because the two answer
    /// different questions: this says what the right-hand column holds, and
    /// `Focus` says where a keystroke lands. See [`Pane`].
    pub pane: Pane,

    pub status: String,
    pub should_quit: bool,

    /// What the network has last told the screen about the connection.
    ///
    /// A plain field beside [`Self::status`] rather than derived from it,
    /// because the sentence and the state answer different questions: the
    /// sentence says what happened, and this says which of the four holds. Set
    /// from `app`, through [`Self::set_connection`], beside the sentence for
    /// the same event — nowhere else.
    pub connection: ConnectionState,

    /// When a transient status stops applying, if it is transient.
    pub(crate) status_until: Option<Instant>,

    /// When the peer stops being shown as typing, and in which conversation.
    ///
    /// One conversation rather than one per chat, because the note is drawn on
    /// the open conversation's title and nowhere else: an event for a chat the
    /// reader is not in is dropped, and a note for a chat left behind belongs to
    /// the chat that was left.
    ///
    /// The same deadline shape as [`Self::status_until`] — an instant compared
    /// only where the loop supplies one — because a frame is drawn from a shared
    /// reference and cannot expire anything itself.
    pub(crate) typing_until: Option<(i64, Instant)>,

    /// Who permutes a right-to-left row: this program, or the terminal.
    ///
    /// **Fixed at construction**, and written only by [`Self::with_bidi`]:
    /// the layout is a pure function of the window, the panel's width and the clock
    /// ([`crate::rows`], invariant 4), and a mode read out of mutable state while
    /// a frame is being drawn would make the same conversation two different
    /// heights depending on when it was asked. A caller that has read the
    /// configuration says so once and every later frame draws the same rows.
    ///
    /// [`BidiMode::Terminal`] — the default — emits rows as they are stored and
    /// lets the terminal rearrange them, which is what a shaping terminal needs.
    pub(crate) bidi: BidiMode,

    /// Whether a decoded sticker paints its picture or its token.
    ///
    /// **Fixed at construction**, and written only by [`Self::with_stickers`]:
    /// the same purity argument as [`Self::bidi`] — the layout counts block
    /// rows for a picture and token rows for a token, so a mode that could
    /// change while the window is open would make the same conversation two
    /// different heights depending on when it was asked. A caller that has
    /// read the configuration says so once and every later frame draws the
    /// same rows.
    ///
    /// [`StickerMode::Inline`] — the default — paints the block where bytes
    /// are cached. `tui` names no configuration type: the spelling in the file
    /// is `app`'s to read, and this is what it becomes.
    pub(crate) stickers: StickerMode,

    /// Rows, body width, and the clock, as of the last frame.
    pub(crate) metrics: FrameMetrics,
}

impl UiState {
    /// Normal mode, the conversation focused, and no frame measured yet.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            mode: Mode::Normal,
            focus: Focus::Conversation,
            theme: Theme::default(),
            pane: Pane::Conversation,
            status: IDLE_STATUS.to_string(),
            should_quit: false,
            connection: ConnectionState::Connecting,
            status_until: None,
            typing_until: None,
            bidi: BidiMode::Terminal,
            stickers: StickerMode::Inline,
            metrics: FrameMetrics {
                rows: Cell::new(ASSUMED_ROWS),
                body_width: Cell::new(ASSUMED_BODY_WIDTH),
                now: Cell::new(0),
            },
        }
    }

    /// The same application, drawing right-to-left rows itself.
    ///
    /// By value and at construction rather than a setter, because the mode is an
    /// input to the layout rather than a thing that changes while the window is
    /// open: every row is then the same height whichever mode was asked for, and
    /// [`App::row_layout`](crate::app::App::row_layout) stays a pure function of
    /// the window and the width. A caller that has read the configuration calls
    /// this once, where it builds the application; nothing else needs to say
    /// anything.
    #[must_use]
    pub(crate) fn with_bidi(mut self, bidi: BidiMode) -> Self {
        self.bidi = bidi;
        self
    }

    /// The same application, drawing `[sticker]` where a picture would go.
    ///
    /// By value and at construction rather than a setter, for the same reason
    /// as [`Self::with_bidi`]: the mode is an input to the layout, and
    /// [`App::row_layout`](crate::app::App::row_layout) stays a pure function
    /// of the window and the width. A caller that has read the configuration
    /// calls this once, where it builds the application; nothing else needs to
    /// say anything.
    #[must_use]
    pub(crate) fn with_stickers(mut self, stickers: StickerMode) -> Self {
        self.stickers = stickers;
        self
    }

    /// Shows `text` on the status line for a while, then reverts.
    ///
    /// For things that pass on their own: a send that failed, a refusal. State
    /// the reader must not lose is written straight to [`Self::status`], which
    /// never carries a deadline.
    pub(crate) fn flash(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_until = Some(Instant::now() + FLASH_FOR);
    }

    /// Puts the status line back to its resting sentence.
    ///
    /// The other half of [`Self::flash`], for the answers that are not transient:
    /// a flow that has said its sentence and moved on must not keep showing it
    /// over the next thing the reader does.
    pub(crate) fn clear_status(&mut self) {
        IDLE_STATUS.clone_into(&mut self.status);
        self.status_until = None;
    }

    /// Writes `text` to the status line with no deadline.
    ///
    /// For state the reader must not lose — a bring-up, an unknown command —
    /// which [`Self::flash`] must not carry: a deadline would take it away
    /// before its answer arrives.
    pub(crate) fn show_persistent(&mut self, text: &str) {
        text.clone_into(&mut self.status);
        self.status_until = None;
    }

    /// Reverts a transient status once its time is up.
    ///
    /// Reports whether a redraw is owed. Called from the loop, which already
    /// runs on a timer: a status cannot expire during a frame, because a frame
    /// is drawn from a shared reference.
    pub(crate) fn expire_status(&mut self, now: Instant) -> bool {
        if self.status_until.is_none_or(|at| now < at) {
            return false;
        }

        self.status_until = None;
        IDLE_STATUS.clone_into(&mut self.status);
        true
    }

    /// Records the peer's typing note, or clears it.
    ///
    /// `None` is the note going away with the view it belonged to, which is
    /// what a conversation switch and an arrived message both do.
    pub(crate) fn set_typing(&mut self, typing: Option<(i64, Instant)>) {
        self.typing_until = typing;
    }

    /// Stops showing the peer as typing once its deadline has passed.
    ///
    /// Reports whether a redraw is owed. Called from the loop beside
    /// [`Self::expire_status`], which already runs on a timer: nothing repaints on
    /// a schedule for this, so a peer who stops without a final event is gone by
    /// the tick after the deadline rather than by a frame of its own.
    pub(crate) fn expire_typing(&mut self, now: Instant) -> bool {
        let Some((_, at)) = self.typing_until else {
            return false;
        };
        if now < at {
            return false;
        }

        self.typing_until = None;
        true
    }

    /// Puts the dispatch `mode` somewhere.
    pub(crate) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Puts the keystroke landing `focus` somewhere.
    pub(crate) fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
    }

    /// Shows `pane` in the right-hand column.
    pub(crate) fn set_pane(&mut self, pane: Pane) {
        self.pane = pane;
    }

    /// Records that the reader answered yes to quitting.
    pub(crate) fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Writes `status` straight to the line, with no deadline.
    ///
    /// The answering half of [`Self::show_persistent`] for callers that already
    /// hold the owned sentence rather than a borrowed one.
    pub(crate) fn set_status(&mut self, status: String) {
        self.status = status;
    }

    /// Records what the network last told the screen about the connection.
    ///
    /// The one writer of [`Self::connection`], and `pub` because the writer is
    /// `app`'s `net::apply` rather than anything in this crate: the transition
    /// belongs beside the sentence for the same event, and that sentence is
    /// written on the other side of the boundary.
    pub fn set_connection(&mut self, connection: ConnectionState) {
        self.connection = connection;
    }

    /// Records how many message rows the conversation panel has room for.
    ///
    /// Called from the panel, which is the only place the terminal's height has
    /// been turned into a rectangle. Zero is not a measurement anything can act
    /// on, so it is stored as one row: a page that moves nowhere is worse than a
    /// page that moves too little.
    ///
    /// This is the panel's height, and it is rows rather than messages: a
    /// message is as tall as its text is, and how tall that is depends on the
    /// width the panel gave it.
    pub(crate) fn record_rows(&self, rows: usize) {
        self.metrics.rows.set(rows.max(1));
    }

    /// Records how many columns the conversation panel's messages have room for.
    pub(crate) fn record_body(&self, width: u16) {
        self.metrics.body_width.set(width);
    }

    /// Records what the reader's clock says, in unix seconds.
    ///
    /// Called by the host once a frame, alongside the other measurements it
    /// records: what a day is called depends on when it is being read, and
    /// nothing here can know that.
    pub(crate) fn record_now(&self, now: i64) {
        self.metrics.now.set(now);
    }
}
