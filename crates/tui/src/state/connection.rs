//! What the network has last told the screen about the connection.
//!
//! A rendering-neutral state machine: four variants and no wording, no colour,
//! no rank. What each variant *says* on the status line stays where it has
//! always been said — the sentences `net::apply` writes — and what it *looks
//! like* is STAGE-02's. This module only records which one holds, so the
//! indicator that draws it cannot disagree with the events that set it.
//!
//! Owned by `tui` and set from `app`, which is the one crate that sees both
//! halves: every transition goes through `net::apply`, beside the sentence for
//! the same event. Nothing here reads `net::State` — `bringing_up` and
//! `auto_reconnect_used` stay the driver's business.
//!
//! The default is [`ConnectionState::Connecting`], which is what a launch is:
//! the runtime writes `"connecting…"` before the first bring-up answers, and a
//! state built before any event must agree with that screen.

/// What the connection is, as far as the screen is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// Nothing has answered yet: the launch bring-up is on its way.
    ///
    /// The default, because a state built before any event is a launch, and a
    /// launch is waiting for its `Ready`.
    #[default]
    Connecting,

    /// The client is up and the feed is delivering: `Ready` said so, or an
    /// update has arrived since.
    Connected,

    /// The feed dropped and a rebuild or a waited-out retry is under way: the
    /// sentence for the event says the same thing in words.
    Reconnecting,

    /// The bring-up or the reconnect budget is spent: the `offline:` screen.
    Offline,
}
