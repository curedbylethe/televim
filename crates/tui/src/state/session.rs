//! The session lifecycle and the sign-in surface.

use crate::app::{AccountState, SessionStore, SignIn};

/// The account's session and the sign-in surface.
pub struct SessionState {
    /// What the profile panel shows, and why it might show nothing.
    pub account: AccountState,

    /// Where the session is kept, as the panel says it.
    pub session_store: SessionStore,

    /// The sign-in surface, when it is up.
    ///
    /// `None` for every reader who is already signed in, which is most of the
    /// life of the program. `Some` is an overlay and not a pane: see [`SignIn`].
    pub signin: Option<SignIn>,

    /// The phone number configuration carried, if there is one.
    ///
    /// Held rather than read from a prompt because `:signin` needs it twice —
    /// to fill the field in, and to draw the row that says where the code went —
    /// and because it is the one thing about a sign-in a reader does not have to
    /// type. It is rewritten by the network with the number the request actually
    /// went out with, so it is "where the code went" rather than "what the
    /// configuration suggested".
    pub phone: String,

    /// What the configuration carried for the login code, if any.
    ///
    /// A **pre-fill**, and the only place a code may come from that is not
    /// Telegram: Telegram sends the code, and the reader types it. This saves
    /// that on a machine where the code is already written down, and it is read
    /// once — when the step opens — because a wrong code has to be retyped, not
    /// restored.
    pub code_prefill: String,

    /// What the configuration carried for the two-factor password, if any.
    ///
    /// A pre-fill on the same terms as [`Self::code_prefill`]: read when the step
    /// opens, never restored after a refusal.
    pub password_prefill: String,

    /// Whether this machine carries application credentials at all.
    ///
    /// **The gate on the flow.** Without an `api_id` and `api_hash` there is no
    /// client to sign in *to*, so a phone field would be asking the reader for
    /// something the program still could not do with — and the sentence that
    /// says so is a better screen than a form that cannot be finished.
    pub credentials_configured: bool,

    /// Whether a client is there to carry a request.
    ///
    /// **The gate on the sign-in's `waiting` flag.** A request no client will
    /// take is not a request on its way, so the flow must not be told one is:
    /// the panel would say "Checking…" for an answer that is never coming, and
    /// nothing else on the screen can put it right. `false` until the caller
    /// says otherwise — this side of the boundary cannot see a client.
    pub(crate) client_available: bool,

    /// Whether the reader has asked for the client to be brought up again.
    ///
    /// Recorded here because `tui` cannot reach the network, and taken by the
    /// one caller that can. **Not an [`crate::app::Action`]**, and the reason is
    /// the state a retry is needed in: actions are drained only while there is a
    /// client, so a queued one would be invisible at exactly the moment it
    /// matters — a launch that came up `offline:`. One slot rather than a queue,
    /// because a second retry while one is on its way is the same retry; the
    /// caller says so with a transient status rather than asking twice.
    pub(crate) retry_requested: bool,
}

impl SessionState {
    /// Nothing read, nothing configured, and no client to carry a request.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            account: AccountState::Unfetched,
            session_store: SessionStore::default(),
            signin: None,
            phone: String::new(),
            code_prefill: String::new(),
            password_prefill: String::new(),
            // A program that is handed nothing assumes nothing: the caller that
            // has read the configuration says so, and a launch without one gets
            // the sentence rather than a form.
            credentials_configured: false,
            // A program that is handed nothing has no client either, and a
            // sign-in it accepts now would be a request nobody carries.
            client_available: false,
            retry_requested: false,
        }
    }
}
