//! The network half of the application.
//!
//! `app` is the composition root: the one crate that may hold a framework
//! `Client` and the `proto` wrapper around it at the same time. This module is
//! where those two meet the event loop, and it is deliberately the only place
//! that knows both halves exist.
//!
//! # What it does
//!
//! Three things, in order of how much they decide:
//!
//! * **Brings the client up** — build it from the configuration, fetch the chat
//!   list and take the update feed **when the stored session is signed in**, and
//!   run the sign-in flow when it is not. Every failure is reported to the screen
//!   rather than raised, because a client that cannot connect is a state the
//!   reader is in, not a crash.
//! * **Asks for pages** — after every event and every tick, the conversation on
//!   show is asked what it needs next, and a fetch is spawned for it. The fetch
//!   runs as its own task so that a round trip does not stop the reader's
//!   keystrokes from being read.
//! * **Folds in what arrives** — a page, an update, or the chat list.
//!
//! # What is decided here rather than performed
//!
//! Everything with a rule in it is a function over the state, so it can be
//! tested without a client, a runtime or a datacenter: [`wanted`] decides which
//! page to ask for, [`open_first_chat`] decides what a fetched list means for
//! the screen, and [`backoff`] decides how long to leave a failure alone. The
//! rest of this module is what performs those decisions.
//!
//! # Where the cursor lives
//!
//! A [`HistoryCursor`] belongs to `proto`, and `tui` may not name `proto` — so
//! the cursor lives here, beside the loop, and the window is told only what it
//! needs to know: that a direction has run out. That is the one fact the trigger
//! cannot work out for itself, because a short page and the last full one look
//! the same once they are in the window.
//!
//! The same is true of the sign-in tokens — [`LoginCode`] and
//! [`PasswordChallenge`], which are `proto` types `tui` may not name — and of
//! the store the session went into. [`State`] holds them, so the one module that
//! may see both halves sees them in one place.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use domain::chat::Chat;
use domain::history::ConversationView;
use domain::message::{MediaKind, Message};
use domain::search::SEARCH_MATCHES;
use domain::updates::UpdateEvent;
use domain::user::UserCandidate;
use proto::{
    HistoryCursor, LoginCode, PasswordChallenge, ProtoClient, ProtoError, SearchResults, SignIn,
    UpdateStream,
};
use telegram_framework::{
    ClientBuilder, FileStore, FrameworkError, KeyProvider, KeyringKeyProvider, KeyringStore,
    MEDIA_LIMIT, PASSWORD_ATTEMPTS, PassphraseProvider, Refusal, RequestError, SessionError,
    SessionStore,
};
use tokio::sync::mpsc::UnboundedSender;
use tui::app::{Action, App, ConnectionState, FetchDirection, Jump, LoginField};

use crate::config::Config;
use crate::draft_store::DraftFile;
use crate::history_store::{HistoryCache, HistoryFile, PageKind};
use crate::media_cache::{Key, MediaCache};
use crate::runtime::AppEvent;

/// How many messages one page holds.
///
/// Telegram will not return more than this in one answer, so it is both the size
/// asked for and the size a short page is measured against — the framework
/// clamps to the same number, and `proto` reads the clamped value back to decide
/// whether a direction has run out.
const PAGE: usize = 100;

/// How long to wait before asking for a page again after a failure.
///
/// Only used when Telegram did not say how long to wait: its own answer is the
/// useful one, and a fixed backoff is either too short to satisfy it or too long
/// for a failure that will pass on its own.
const RETRY: Duration = Duration::from_secs(5);

/// How many times the launch chat-list fetch is asked for before the bring-up is
/// called a failure.
///
/// A bound rather than an open loop because every attempt is another request
/// Telegram may refuse: an unbounded retry against a session it has decided
/// against is a client that never says it is offline. Three is the same bound
/// the two-factor password gets, and it buys two waits — a transient refusal
/// and a flood wait that fits inside one of them.
const CHAT_LIST_ATTEMPTS: u8 = 3;

/// Something that happened away from the keyboard.
pub enum Event {
    /// The machine carries no application credentials, so there is nothing to
    /// connect as.
    ///
    /// Its own event rather than an [`Event::Offline`], because nothing failed:
    /// there is no client to build and no session to fetch, and the screen's
    /// answer is a sentence about the configuration rather than a failure.
    NoCredentials,

    /// The client is up, the chat list has been fetched, and the account's own
    /// profile has been read.
    Ready {
        /// The wrapper the fetches go through.
        client: Arc<ProtoClient>,

        /// The conversations, newest first.
        chats: Vec<Chat>,

        /// The account's own profile, or why there is not one.
        ///
        /// Its own `Result` rather than a second event, because a profile that
        /// could not be read is not a failure of the client: the conversations
        /// are there, and the screen is usable. It travels with the rest of what
        /// "the client is up" means, so the two cannot be applied apart.
        ///
        /// **Three answers, and the empty one is not a failure.**
        /// `Ok(account)` is signed in. `Err(String::new())` is *no session at
        /// all* — the signed-out card, which is a state and not a reason, so it
        /// carries no reason. `Err(reason)` is signed in and the profile read
        /// failed, and the reason is what the card draws under it.
        account: Result<domain::account::Account, String>,

        /// Where the session is kept, as the panel says it.
        session_store: tui::SessionStore,
    },

    /// Telegram accepted a number and sent its code, or refused the request.
    ///
    /// The number travels back because the panel draws where the code went, and
    /// the configuration's copy is a pre-fill rather than the answer: a reader
    /// who changed it must be told about the number that was actually used.
    CodeRequested {
        /// The number the request went out with, as the reader gave it.
        phone: String,

        /// The code to redeem, or why there is not one.
        result: Result<LoginCode, ProtoError>,
    },

    /// A login code was redeemed, or refused.
    ///
    /// A completed step does not merely clear the overlay: the account it named
    /// arrives here, and the chat list and the feed are fetched behind it, so
    /// "you are in" reaches the screen through [`Event::Ready`] either way.
    SignedIn {
        /// The account, the password step to come, or why there is neither.
        result: Result<SignIn, ProtoError>,
    },

    /// A two-factor password was checked, or refused.
    ///
    /// Its own event rather than another [`Event::SignedIn`] because the
    /// question is different and the count is different: only a password answer
    /// carries attempts left, and a panel counting them for the wrong step would
    /// count something Telegram did not count.
    PasswordChecked {
        /// The account, or why it is not signed in.
        result: Result<SignIn, ProtoError>,
    },

    /// A stored session that could not be read has been discarded, and the
    /// startup is carrying on without one.
    ///
    /// Its own event rather than an [`Event::Offline`] because nothing failed:
    /// the bytes were unusable, the reader is being asked to sign in again, and
    /// the [`Event::Ready`] that follows is the signed-out screen doing its
    /// ordinary work. A sentence of its own rather than a reason on the account,
    /// because the empty account already means "no session at all" and a reason
    /// there would turn the sign-in form into an error card.
    SessionDiscarded(String),

    /// The client could not be brought up.
    ///
    /// Carries the reason, and the chain behind it: the screen has one line to
    /// say what went wrong, and the most specific answer is the useful one.
    Offline(anyhow::Error),

    /// The launch chat-list fetch failed and is being asked again.
    ///
    /// Its own event rather than a second [`Event::Offline`] because nothing has
    /// failed yet: the screen has one line to spend, and the reader watching a
    /// launch they expect to end in a chat list is better told that the wait is
    /// a wait with a reason and an end than shown an `offline:` they may not be
    /// able to clear. The [`Event::Offline`] behind it comes only once the
    /// attempts are spent.
    ChatListRetrying(ChatListRetry),

    /// A feed read failed and is being waited out.
    ///
    /// Its own event rather than silence because the reader watching a live
    /// feed is owed the same account a launch gets: the wait is a wait with a
    /// reason and an end. The [`Event::FeedEnded`] behind it comes only once
    /// the errors are past their bound.
    FeedRetrying(FeedRetry),

    /// The update feed has been read to its end.
    ///
    /// The feed ends when the client shuts down, and `None` is that. Its own
    /// event rather than an [`Event::Offline`] because nothing has failed yet:
    /// the app rebuilds the client from the stored session and takes a new feed,
    /// and only when that reconnect cannot be made — or the feed ends again
    /// before any update has arrived — does the reader see an `offline:`.
    FeedEnded,

    /// A page came back, or the fetch that asked for it failed.
    History {
        /// What was asked for.
        direction: FetchDirection,

        /// The id the page was counted from, exclusive: the oldest loaded for
        /// an older page, the newest loaded for a newer one, nothing for the
        /// newest page.
        ///
        /// Taken from the cursor *before* the fetch moved it, because that is
        /// what says whether the page joins the cached history: the cursor
        /// below has already been moved onto the page itself.
        anchor: Option<i64>,

        /// The cursor the fetch ended with.
        ///
        /// Sent back rather than kept here because only the fetch knows whether
        /// the page was short, and that is what settles a direction.
        cursor: HistoryCursor,

        /// The page, oldest first, or why there is not one.
        result: Result<Vec<Message>, ProtoError>,
    },

    /// A page came back around a message the reader jumped to, or the fetch that
    /// asked for it failed.
    ///
    /// Its own event rather than another direction, because what it replaces is
    /// not an end of the window: the page lands somewhere the reader named, and
    /// the only thing that says whether it is still wanted is the target.
    Jumped {
        /// What was asked for.
        jump: Jump,

        /// The cursor for the conversation the page belongs to.
        ///
        /// A page that replaces the window is the caller's to report, so the
        /// cursor travels with the answer rather than being updated where the
        /// fetch ran.
        cursor: HistoryCursor,

        /// The page, oldest first, or why there is not one.
        result: Result<Vec<Message>, ProtoError>,
    },

    /// A contact's profile came back, or the read failed.
    ///
    /// The `peer_id` is the card that asked, and a card the reader has since left
    /// is not the one this answer belongs to — which is checked when it is applied
    /// rather than here, because only the screen knows which card is on show.
    Contact {
        /// Bare identifier of the person who was asked about.
        peer_id: i64,

        /// Their profile, or why there is not one.
        result: Result<domain::account::Account, ProtoError>,
    },

    /// A send came back, or the request failed.
    Sent {
        /// The conversation it was sent to, so a result the reader has left can
        /// be dropped.
        chat_id: i64,

        /// The placeholder the message was rendered as.
        temp_id: i64,

        /// The message the server accepted, or why there is not one.
        result: Result<Message, ProtoError>,
    },

    /// An edit came back, or the request failed.
    Edited {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message that was edited.
        message_id: i64,

        /// Nothing on success: the new text arrives as an update.
        result: Result<(), ProtoError>,
    },

    /// A pin change came back, or the request failed.
    PinToggled {
        /// The chat whose pin was asked for.
        chat_id: i64,

        /// The pin that was asked for, so a success is applied without asking
        /// the list what it was.
        pinned: bool,

        /// Nothing on success: the list moves here, on the answer.
        result: Result<(), ProtoError>,
    },

    /// A forward came back, or the request failed.
    Forwarded {
        /// The conversation the messages were forwarded from.
        chat_id: i64,

        /// The conversation they were forwarded to.
        dest_chat_id: i64,

        /// How many messages were asked for.
        requested: usize,

        /// How many landed on success; the failure otherwise.
        result: Result<usize, ProtoError>,
    },

    /// A deletion came back, or the request failed.
    Deleted {
        /// The conversation the messages belonged to. Used only to decide
        /// whether to surface a failure, never to scope the removal.
        chat_id: i64,

        /// The messages that were asked for.
        message_ids: Vec<i64>,

        /// Nothing on success: the messages leave as an update.
        result: Result<(), ProtoError>,
    },

    /// A sign-out was carried out, or the stored session could not be cleared.
    ///
    /// Its own event because it is the only one that *ends* a client: the answer
    /// is not a value to put on screen but a decision about which client there
    /// is, and the reader who asked for it is signed out afterwards.
    LoggedOut {
        /// Nothing on success: the account is gone and the screen says so.
        ///
        /// A failure means the **local** session is still there — the framework
        /// clears the store after a warn-only revocation, so only
        /// `store.session.clear()` can fail and that is the one thing that
        /// decides whether the reader is signed out at all.
        result: Result<(), String>,
    },

    /// Telegram accepted the reader's read marker for a conversation, so its
    /// unread count is cleared on the chat list.
    ///
    /// Sent only on success: a refused marker leaves the count as the server had
    /// it, and the next open asks again.
    ReadMarked {
        /// The conversation the marker was for.
        chat_id: i64,

        /// The newest message the marker reached, which is what the answer is
        /// recorded against.
        max_id: i64,
    },

    /// An update arrived for a conversation televim displays.
    Update(UpdateEvent),

    /// A search came back, or the request failed.
    ///
    /// Its own event rather than another direction: a search asks a different
    /// question from "which page does this window need", and its answer writes
    /// only the match list, never the window. The two cannot collide.
    Searched {
        /// The conversation it was for, so a result the reader has left can be
        /// dropped.
        chat_id: i64,

        /// The query, echoed back.
        ///
        /// `chat_id` alone is not the whole identity of an answer: `/foo` and
        /// then `/bar` inside one round trip means foo's answer would otherwise
        /// land on bar's list and the label would name the wrong query. The
        /// reader still wanting the answer is checked against this.
        query: String,

        /// The matching identifiers, oldest first, or why there are none.
        result: Result<SearchResults, ProtoError>,
    },

    /// A username resolved to the person who owns it, or found nobody.
    ///
    /// `user: None` is not an answer to show: the query was not a handle anyone
    /// owns, and the caller's fallback — the name search — is what supplies a
    /// list. That intermediate state travels as this event's own absence rather
    /// than as a separate event, so the routing rule stays in `looks_like_username`
    /// and the arm that reads it.
    ///
    /// Not an [`Event::Searched`]: a search asks a different question about a
    /// conversation, and its answer writes only that window's match list.
    UserResolved {
        /// The query it was for, echoed back so a late answer can be refused.
        query: String,

        /// The person, or `None` when nobody owns the handle.
        user: Option<UserCandidate>,
    },

    /// A name search listed the people it matched.
    ///
    /// The query is echoed back for the same reason as [`Event::Searched`]'s: a
    /// result for a query the reader has replaced must not land on the newer
    /// list's label.
    UsersListed {
        /// The query it was for, echoed back.
        query: String,

        /// The people the search offered, best first as the server ranked them.
        users: Vec<UserCandidate>,
    },

    /// A user lookup failed.
    ///
    /// Its own event rather than a `Result` on one of the two above, because the
    /// two success shapes differ and a failure is neither: it belongs to the
    /// query, not to a list or to a person.
    UserLookupFailed {
        /// The query it was for, echoed back.
        query: String,

        /// Why the lookup failed, worded for the status line and the label.
        reason: String,
    },

    /// A sticker download that ran off the loop has finished. Settled here,
    /// on the loop, because the cache is single-threaded: the fetch lives in
    /// a task, the answer comes back as this event.
    StickerSettled {
        /// The chat the message is in, echoed back so a reader who has moved
        /// on is not handed a picture for the chat on show.
        chat_id: i64,

        /// The message the sticker belongs to.
        message_id: i64,

        /// The bytes, or why the fetch failed, worded for the log.
        fetched: Result<Vec<u8>, String>,
    },

    /// A media download that ran off the loop has written its file. Nothing has
    /// opened it yet: the path waits in [`State::media`] for the viewer.
    MediaSaved {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message whose media was saved.
        message_id: i64,

        /// The temp file the media was written to, whole.
        path: PathBuf,
    },

    /// A media download that ran off the loop has not written a file, worded
    /// for the status line.
    MediaFailed {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message whose media was asked for.
        message_id: i64,

        /// Why no file was written, with the fact that none was.
        reason: String,
    },

    /// A media download has collected another chunk. Nothing settles on it: the
    /// reader's view of the transfer is drawn from it, and the transfer goes on.
    MediaProgress {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message whose media is downloading.
        message_id: i64,

        /// The bytes collected so far.
        downloaded: usize,

        /// The size Telegram declared, when it declared one.
        total: Option<usize>,
    },

    /// A media download was stopped on the reader's request. No file was
    /// written, so there is nothing for a viewer to open.
    MediaCancelled {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message whose download was stopped.
        message_id: i64,
    },
}

/// What a chat-list retry in progress has to say about itself.
///
/// One type rather than four fields on the event, because the whole of it is one
/// sentence: none of the reason, the wait, or the count is read apart from the
/// others, and a reader told the client is retrying is owed all three at once.
pub struct ChatListRetry {
    /// Why the attempt that just failed failed.
    reason: ProtoError,

    /// How long until the next attempt, which is [`backoff`]'s answer: Telegram's
    /// own wait when it gave one, the fixed `RETRY` otherwise.
    delay: Duration,

    /// The attempt that failed, of `attempts` in all.
    ///
    /// Counted as attempts *made*, so the last one to be reported is the second
    /// — the third either succeeds, and its [`Event::Ready`] is the answer, or
    /// fails, and its `offline:` is.
    attempt: u8,

    /// The bound those attempts are counted against.
    attempts: u8,
}

/// What a feed retry in progress has to say about itself.
///
/// One type rather than four fields on the event, because the whole of it is one
/// sentence: none of the reason, the wait, or the count is read apart from the
/// others, and a reader watching a live feed is owed all three at once — the
/// same account a launch gets from [`ChatListRetry`].
pub struct FeedRetry {
    /// Why the read that just failed failed.
    reason: ProtoError,

    /// How long until the next read, which is [`backoff`]'s answer: Telegram's
    /// own wait when it gave one, the fixed `RETRY` otherwise.
    delay: Duration,

    /// The failure being waited out, of `attempts` in all.
    ///
    /// Counted as failures *seen*, so the first one to be reported is the
    /// first — every wait inside the bound is slept, and only the error past
    /// it ends the feed.
    attempt: u8,

    /// The bound those failures are counted against.
    attempts: u8,
}

/// What the loop knows about the network between events.
#[derive(Default)]
pub struct State {
    /// The client, once it is up.
    client: Option<Arc<ProtoClient>>,

    /// The cursor for the conversation on show, and how long it has to be left
    /// alone.
    history: History,

    /// The sign-in tokens, and the store the session is in.
    ///
    /// Both here for the same reason [`History`] is: they are `proto` and
    /// configuration types that `tui` may not name, and they outlive the request
    /// that made them.
    login: Login,

    /// Where the session went, once bring-up has said so.
    session_store: Option<tui::SessionStore>,

    /// Where unsent words live on disk, so signing out can remove them.
    ///
    /// Set by the loop after [`State::new`]: the path is derived from the
    /// configuration path, which `apply` cannot be handed — the loop owns it.
    /// `None` wherever no loop set one, which is every test.
    draft_file: Option<DraftFile>,

    /// Where read messages and the chat list live on disk: written behind
    /// every page, feed event or list change that changes [`State::cached`],
    /// and removed on sign-out.
    ///
    /// Set by the loop beside [`State::draft_file`], and `None` in the same
    /// places for the same reason — with no file the cache is kept in memory
    /// and never written.
    history_file: Option<HistoryFile>,

    /// The media cache: downloads are looked up in it and stored into it.
    ///
    /// Set by the loop at launch, and `None` in the same places for the same
    /// reason. Shared with the download tasks, which lock it off the loop.
    media_cache: Option<Arc<Mutex<MediaCache>>>,

    /// The cached messages, and where writing them to that file has got to.
    cached: CachedHistory,

    /// The media files saved for the viewer, oldest first.
    ///
    /// Pushed by [`apply`] when a download lands, and taken by the loop with
    /// [`State::take_media`] before it hands the terminal to a viewer.
    media: tui::state::pending::MediaQueue,

    /// The media downloads started and not yet settled, each with the flag its
    /// cancel sets. Settling a download removes its entry.
    media_cancel: Vec<MediaCancel>,

    /// The configuration, and the channel to answer on — the pair bring-up needs
    /// to be run again.
    ///
    /// A sign-out has to rebuild the client rather than reuse it, because
    /// `UpdateRelay::take` is single-shot: a second `subscribe_updates()` on the
    /// same client fails, so a reader who signed out and back in would have a
    /// live-looking UI and no updates for the rest of the process. Rebuilding
    /// needs a `Config`, and `apply` has no way to be handed one — the loop owns
    /// it — so it is kept here, and with the sender that has to go with it.
    ///
    /// **Two `Option`s that are read together and set together.** Either alone
    /// does nothing: a configuration with no channel cannot report what it found,
    /// and a channel with no configuration has nothing to bring up. `None` in
    /// tests is therefore the ordinary case, not a special one.
    cfg: Option<Config>,

    /// The channel bring-up answers on. Read only beside [`State::cfg`].
    tx: Option<UnboundedSender<AppEvent>>,

    /// Whether a bring-up is in flight, so a second one is not started over it.
    ///
    /// Set where a bring-up is *issued* rather than where it is wanted, because
    /// the window that matters is the whole of one — including the retry loop's
    /// own waits, where a reader's `:retry` would otherwise start a second client
    /// beside the first and let two of them fight over one update relay. Cleared
    /// by the two events that end a bring-up, [`Event::Ready`] and
    /// [`Event::Offline`], because those are the only answers it can come back
    /// with.
    ///
    /// `true` from [`State::new`]: the state that can bring the client up is
    /// built immediately before the launch bring-up is issued, so the launch
    /// needs no separate marking.
    bringing_up: bool,

    /// A reconnect the feed's end asked for, waiting for the driver to carry it
    /// out.
    ///
    /// A one-slot request rather than a call into bring-up from [`apply`], for
    /// the same reason the reader's `:retry` is: the driver owns the
    /// configuration and the channel a bring-up needs, and it is the only place
    /// the single-flight guard can be read. Taken once, like a retry: a request
    /// taken is a request being carried out.
    reconnect_requested: bool,

    /// Whether the one automatic reconnect has been spent.
    ///
    /// **One per working feed.** Issuing the reconnect sets it, and an
    /// [`Event::Update`] clears it, because an update is the feed proving it
    /// works — without that, a reconnect that re-subscribes to a feed that
    /// immediately ends again would rebuild the client for ever. A second feed
    /// end while this is set is the reader-visible [`Event::Offline`] rather
    /// than another rebuild.
    auto_reconnect_used: bool,

    /// The conversation and newest message Telegram last accepted a read marker
    /// for. A conversation opened again at the same ceiling is not sent again.
    read_acked: Option<(i64, i64)>,

    /// The conversation whose read marker is owed: one that a message arrived in
    /// since the driver last ran, or a Latest page that a card refused the marker
    /// for. Its marker is asked for again on a later pass.
    ///
    /// A request rather than a call from [`apply`], for the same reason as the
    /// reconnect request: the client and the channel are the driver's. Taken on
    /// every pass, client or not, so an arrival that lands with no client is not
    /// carried into the next one.
    read_owed: Option<i64>,
}

impl State {
    /// The state the loop starts from, able to bring the client up again.
    ///
    /// Both fields at once rather than a setter each: they are read together and
    /// mean nothing apart, and two setters is two ways to set one of them. A
    /// bring-up that reported on a channel nobody is listening to is a bring-up
    /// that hangs rather than fails.
    #[must_use]
    pub fn new(cfg: Config, tx: UnboundedSender<AppEvent>) -> Self {
        Self {
            cfg: Some(cfg),
            tx: Some(tx),
            // The launch bring-up is issued immediately after this is built, and
            // it has to be counted as in flight before the reader can ask for a
            // second one.
            bringing_up: true,
            ..Self::default()
        }
    }

    /// The reconnect the feed's end asked for, once.
    ///
    /// Forgotten on the way out, the way [`App::take_retry_request`] is: a
    /// request taken is a request being carried out, and a caller that asks
    /// again on the next pass gets `false` rather than a second bring-up.
    fn take_reconnect_request(&mut self) -> bool {
        std::mem::take(&mut self.reconnect_requested)
    }

    /// Points sign-out at the drafts file, so it can remove it.
    ///
    /// A setter rather than a third `new` parameter: unlike `cfg` and `tx`
    /// this one is read apart from them, only on the logout path, and every
    /// existing caller builds a loop-less state that has no file.
    pub fn set_draft_file(&mut self, file: DraftFile) {
        self.draft_file = Some(file);
    }

    /// Points sign-out at the history file, so it can remove it.
    ///
    /// A setter for [`State::set_draft_file`]'s reason.
    pub(crate) fn set_history_file(&mut self, file: HistoryFile) {
        self.history_file = Some(file);
    }

    /// Hands the launch's media cache to downloads.
    pub(crate) fn set_media_cache(&mut self, cache: MediaCache) {
        self.media_cache = Some(Arc::new(Mutex::new(cache)));
    }

    /// Empties the media cache: its files are the account's, and sign-out ends
    /// the account.
    ///
    /// Synchronous, like the history file's clear on the same path. Every
    /// download still in flight is cancelled, not only refused on store: its
    /// flag is set so the transfer stops before its next chunk. A download
    /// that does finish is still refused when it stores, so it leaves no file
    /// behind; the next launch under another account, or none, clears the
    /// directory anyway ([`MediaCache::open`]).
    fn forget_media(&self) {
        for cancel in &self.media_cancel {
            cancel.flag.store(true, Ordering::Relaxed);
        }
        if let Some(cache) = &self.media_cache {
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .clear();
        }
    }

    /// Drops a message's cached media after an edit: the message may now carry
    /// different media, so its cached file is no longer its answer.
    fn forget_media_message(&self, chat_id: i64, message_id: i64) {
        if let Some(cache) = &self.media_cache {
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .remove_message(chat_id, message_id);
        }
    }

    /// Drops the cached media of deleted messages, in any chat.
    fn forget_media_messages(&self, message_ids: &[i64]) {
        if let Some(cache) = &self.media_cache {
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .remove_messages(message_ids);
        }
    }

    /// Hands over what the history file held at launch, already vetted
    /// against the configured account.
    ///
    /// Not marked as changed: it is what the file already says.
    pub(crate) fn restore_history(&mut self, cache: HistoryCache) {
        self.cached.messages = cache;
    }

    /// The messages cached for `peer`, oldest first; empty when there are
    /// none.
    pub(crate) fn cached_history(&self, peer: i64) -> Vec<Message> {
        self.cached.messages.get(peer)
    }

    /// Folds a page the wire answered with into the cache, and marks the
    /// cache for writing if the page changed it.
    fn remember_page(&mut self, peer: i64, page: &[Message], kind: PageKind) {
        if self.cached.messages.merge(peer, page, kind) {
            self.cached.dirty = true;
        }
    }

    /// Folds an event from the feed into the cache, and marks the cache for
    /// writing if the event changed it.
    ///
    /// Whatever conversation is on screen: the cache keeps every peer, and an
    /// edit to one the reader is not looking at is still the newest word on
    /// it.
    fn remember_update(&mut self, event: &UpdateEvent) {
        if self.cached.messages.apply_update(event) {
            self.cached.dirty = true;
        }
        match event {
            UpdateEvent::MessageEdited {
                chat_id,
                message_id,
                ..
            } => self.forget_media_message(*chat_id, *message_id),
            UpdateEvent::MessagesDeleted { message_ids } => self.forget_media_messages(message_ids),
            _ => {}
        }
    }

    /// Folds a send the server has numbered into the cache, the way the same
    /// message arriving over the feed would be.
    ///
    /// Its own path because the answer is the one place a send's real
    /// identifier is sure to reach: the feed may or may not carry the same
    /// message back, and when it does the copy lands on the same row. The
    /// placeholder it replaces was never cached.
    fn remember_sent(&mut self, message: &Message) {
        if self.cached.messages.arrive(message) {
            self.cached.dirty = true;
        }
    }

    /// Writes the history cache to its file, if it changed and no write is
    /// still in flight.
    ///
    /// Called by the loop every pass. The snapshot is serialised here, on the
    /// loop's thread, so it is exactly the cache as it stands; only the disk
    /// work goes to the blocking pool, so a slow disk never holds up a frame.
    /// While the previous write is still going the cache stays marked and is
    /// written on a later pass — never beside it, which is what keeps an older
    /// snapshot from landing over a newer one. Failures are the file's to
    /// warn about; the cache in memory is untouched either way.
    ///
    /// **`chats` is the list on screen**, and it is taken into the cache here
    /// rather than where each thing that moves it happens. A list lands with a
    /// `Ready`, is reordered by a pin, and has its previews and unread counts
    /// moved by the feed, by a send and by the reader reading — and the screen
    /// is the one place all of those have already been folded together. Read
    /// once a pass, compared in place, and only a change marks the cache, so a
    /// quiet pass costs a scan of the list and no write.
    pub(crate) fn persist_history(&mut self, chats: &[Chat]) {
        if self.cached.messages.set_chats(chats) {
            self.cached.dirty = true;
        }
        if !self.cached.dirty {
            return;
        }
        let Some(file) = self.history_file.clone() else {
            return;
        };
        if self
            .cached
            .write
            .as_ref()
            .is_some_and(|write| !write.is_finished())
        {
            return;
        }

        self.cached.dirty = false;
        let account = self.cfg.as_ref().and_then(|cfg| cfg.phone.as_deref());
        let Some(bytes) = HistoryFile::encode(&self.cached.messages, account) else {
            return;
        };
        self.cached.write = Some(tokio::task::spawn_blocking(move || file.write(&bytes)));
    }

    /// Waits out the write in flight and writes what it held back, for the
    /// loop to call once on its way out.
    ///
    /// Without it, a page that landed while a write was going would be marked
    /// and never written: the pass that would have written it never comes.
    pub(crate) async fn finish_history(&mut self, chats: &[Chat]) {
        if let Some(write) = self.cached.write.take() {
            let _ = write.await;
        }
        self.persist_history(chats);
        if let Some(write) = self.cached.write.take() {
            let _ = write.await;
        }
    }

    /// Forgets the cached messages, in memory and on disk.
    ///
    /// The file is removed at once when nothing is writing it; otherwise the
    /// removal is queued behind the write in flight, which would otherwise
    /// land after it and put the file back.
    fn forget_history(&mut self) {
        self.cached.messages = HistoryCache::default();
        self.cached.dirty = false;
        let Some(file) = self.history_file.clone() else {
            return;
        };
        match self.cached.write.take() {
            Some(write) if !write.is_finished() => {
                self.cached.write = Some(tokio::spawn(async move {
                    let _ = write.await;
                    let _ = tokio::task::spawn_blocking(move || file.clear()).await;
                }));
            }
            _ => file.clear(),
        }
    }

    /// Takes every saved media path, oldest first, for the loop to hand to the
    /// viewer. Leaves the queue empty.
    pub fn take_media(&mut self) -> Vec<PathBuf> {
        self.media.take_pending()
    }

    /// Sets the cancel flag of every download of `message_id` in `chat_id` that
    /// is in flight. The transfer checks the flag before each chunk and stops; a
    /// download that has already settled has no entry, so nothing happens.
    fn cancel_media(&self, chat_id: i64, message_id: i64) {
        for cancel in &self.media_cancel {
            if cancel.chat_id == chat_id && cancel.message_id == message_id {
                cancel.flag.store(true, Ordering::Relaxed);
            }
        }
    }

    /// The client, once bring-up has installed one.
    ///
    /// Read by the loop's sticker drain, which downloads the panel's queued
    /// requests in series on the loop thread: a sticker that arrives a tick
    /// later draws `[sticker]` that frame either way, so the round trip needs
    /// no task and no event of its own. `None` while there is no client, and
    /// the drain asks nothing then.
    pub(crate) fn client(&self) -> Option<Arc<ProtoClient>> {
        self.client.clone()
    }
}

/// The newest messages of each private conversation, and the write behind
/// them.
#[derive(Default)]
struct CachedHistory {
    /// The messages as the wire has shown them: what the history file was
    /// loaded into at launch, with every fetched page merged in since and
    /// every edit, deletion and arrival the feed carried folded in.
    ///
    /// Held here rather than re-read from the file because every page and
    /// event has to fold into what is cached, and the file is only ever this,
    /// a write behind.
    messages: HistoryCache,

    /// Whether [`CachedHistory::messages`] has changed since its last
    /// snapshot was taken for the file.
    ///
    /// A flag rather than a write per page: pages can land faster than a
    /// write finishes, and only the newest snapshot is worth writing.
    dirty: bool,

    /// The one write of the history file that may be in flight.
    ///
    /// **One at a time is the ordering.** Two writes handed to the blocking
    /// pool together can finish in either order, and the older snapshot
    /// renaming over the newer would leave the file behind the cache. So a
    /// snapshot is only taken once the write before it has finished, and a
    /// sign-out's removal waits behind it too — or the write would put back
    /// the file the sign-out had just removed.
    write: Option<tokio::task::JoinHandle<()>>,
}

/// The two answers Telegram has given this sign-in, and nothing else.
///
/// Both are spent, never kept: a login code is redeemed once and a two-factor
/// challenge is answered once, so a token that is still here is a token whose
/// answer has not come back yet. Holding both at once is the ordinary middle of
/// the flow rather than a mistake — the code is spent before the challenge
/// arrives.
#[derive(Default)]
struct Login {
    /// The code Telegram sent, until a login code is redeemed with it.
    code: Option<LoginCode>,

    /// The two-factor challenge, until a password is submitted against it.
    challenge: Option<PasswordChallenge>,
}

/// Where the loaded part of the open conversation ends.
#[derive(Debug, Default, Clone, Copy)]
struct History {
    /// The cursor for the conversation the window shows.
    ///
    /// Present from the moment that conversation was opened, which is what
    /// stops its first page being asked for twice — and absent while nothing is
    /// open, or after a first page failed and there is nothing to count from.
    cursor: Option<HistoryCursor>,

    /// The jump a page is on its way for, if one is.
    ///
    /// Kept beside the cursor rather than on the window for the same reason the
    /// cursor is: it is about what has been *asked for*, and the window only
    /// ever sees what came back. Without it the driver would ask again on the
    /// next pass, which is a quarter of a second later.
    jump: Option<Jump>,

    /// The earliest instant a page may be asked for again.
    retry_at: Option<Instant>,
}

/// What the conversation on show needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    /// Nothing is open, or a page is already on its way, or a fetch failed
    /// recently enough that asking again would be a storm.
    Nothing,

    /// The newest page, because the conversation was just opened.
    Latest(i64),

    /// A page from an end of what is loaded.
    Page(FetchDirection, HistoryCursor),

    /// A page around a message the reader asked to be taken to.
    Jump(Jump),
}

/// Starts the client, and reports to the loop once the screen can be filled.
///
/// Nothing is awaited by the caller: the terminal is already up, and the first
/// frame is worth drawing before a round trip has finished. Every failure
/// arrives as [`Event::Offline`] with the reason, so the screen can say what
/// went wrong instead of staying silently empty.
pub fn spawn_bring_up(cfg: Config, tx: UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        // Half a pair of application credentials is the same as having none, and
        // neither can build a client — so nothing is built. The screen's answer
        // is the sentence about the configuration rather than an `offline:` line
        // with nothing to have gone offline *to*.
        if cfg.credentials().is_none() {
            let _ = tx.send(AppEvent::Net(Event::NoCredentials));
            return;
        }

        if let Err(reason) = bring_up(&cfg, &tx).await {
            let _ = tx.send(AppEvent::Net(Event::Offline(reason)));
        }
    });
}

/// Builds the client, and fetches what a signed-in session may fetch.
///
/// **It never signs in.** A stored session Telegram no longer knows is not a
/// failure of bring-up and not something a file can repair: the reader is asked
/// for the number and the code on the screen, by the flow [`request_action`]
/// runs, which is the only way a sign-in happens now. So the check below is kept
/// as a *value*, and everything that needs a session is skipped when it is false
/// — the conversations, the account's own profile and the update feed are all
/// requests an unauthorised client cannot answer.
async fn bring_up(cfg: &Config, tx: &UnboundedSender<AppEvent>) -> Result<()> {
    let (api_id, api_hash) = cfg.credentials().context(
        "no telegram application credentials configured; set TELEVIM_API_ID and TELEVIM_API_HASH",
    )?;

    // Resolved once, probed, and then handed on as the same value: the session
    // is described to the reader out of `session_store(cfg)`, and a store
    // resolved a second time could be a second store — so a reader could be told
    // where their session is while a different file is the one being cleared.
    let Session { store, keys } = session_store(cfg);
    let discarded = discard_corrupt_session(&*store);

    // After the probe above, not before: the probe is what opens an existing file
    // and so what derives its key, and a key already derived is not derived
    // twice. What this adds is the case with no file yet: a reader with no key
    // is told so now, rather than after typing a login code that has nowhere to
    // be kept.
    if let Some(keys) = &keys {
        keys.sealing_key().map_err(|error| key_trouble(&error))?;
    }

    let client = ClientBuilder::new(api_id, api_hash)
        .session_store(store)
        .build()
        .await
        .map_err(|error| match error {
            // A file that did not open is the key's doing, and the sentence says
            // which key. Never a discard: see [`discard_corrupt_session`].
            FrameworkError::Session(error) if keys.is_some() => key_trouble(&error),
            error => anyhow::Error::new(error).context("building the client"),
        })?;

    let authorized = client
        .is_authorized()
        .await
        .context("checking whether the stored session is signed in")?;

    let client = Arc::new(ProtoClient::new(client));
    let session_store = session_description(cfg);

    if !authorized {
        // A drawable state rather than a failure: the signed-out card is what a
        // reader with no session is in, and it says what to do about it. The
        // empty `Err` is the one account meaning that carries no reason.
        let _ = tx.send(AppEvent::Net(Event::Ready {
            client,
            chats: Vec::new(),
            account: Err(String::new()),
            session_store,
        }));
        report_discarded(discarded, tx);
        return Ok(());
    }

    // The one fetch the launch cannot do without, and the one request most
    // likely to be refused on the way in: Telegram rate-limits a client that has
    // been off for a while, and a single refusal would otherwise throw the
    // reader a terminal `offline:` for something that passes on its own. So it
    // is asked again while there is budget left, saying so each time, and only
    // the last refusal is a failure of bring-up. Everything after it — the
    // account, the feed, the `Ready` — still runs exactly once, on the attempt
    // that worked.
    let mut attempts_used: u8 = 0;
    let chats = loop {
        match client.fetch_private_chats().await {
            Ok(chats) => break chats,
            Err(error) => {
                let Some(delay) = chat_list_retry(attempts_used, &error) else {
                    return Err(anyhow::Error::new(error).context("fetching the chat list"));
                };
                attempts_used += 1;
                let _ = tx.send(AppEvent::Net(Event::ChatListRetrying(ChatListRetry {
                    reason: error,
                    delay,
                    attempt: attempts_used,
                    attempts: CHAT_LIST_ATTEMPTS,
                })));
                tokio::time::sleep(delay).await;
            }
        }
    };
    let account = read_account(&client).await;

    // The list is fetched before the feed is taken. Resolving what arrived while
    // the client was offline reads peers back out of the session, and iterating
    // the dialogs is what puts them there.
    let updates = client
        .subscribe_updates()
        .await
        .context("taking the update feed")?;

    let _ = tx.send(AppEvent::Net(Event::Ready {
        client,
        chats,
        account,
        session_store,
    }));
    report_discarded(discarded, tx);
    tokio::spawn(pump(updates, tx.clone()));

    Ok(())
}

/// A stored session that was unreadable is gone, so the reader has to sign in
/// again — and there is a sentence for that on the status line.
///
/// Placed **above** the store rather than in it: `clear` is part of the store's
/// own report-not-reset contract, and a store that silently dropped what it
/// could not parse would be a store that says a session is gone when it is not.
///
/// `None` for everything else, deliberately: a store that could not be reached
/// or read for any other reason is a failure of the machine, not bytes to throw
/// away, and the reader is better served by the `offline:` line that says so
/// than by a sign-in that was never going to work.
fn discard_corrupt_session(store: &dyn SessionStore) -> Option<String> {
    let Err(SessionError::Corrupt(reason)) = store.load() else {
        return None;
    };

    tracing::warn!(%reason, "discarding an unreadable stored session");
    if let Err(error) = store.clear() {
        tracing::warn!(%error, "the discarded stored session could not be removed");
    }

    Some(format!(
        "the stored session could not be read ({reason}) and has been discarded — \
         sign in again"
    ))
}

/// Says, if a stored session was discarded, why the reader is being signed out.
///
/// Behind the [`Event::Ready`] it belongs to rather than in front of it: the
/// screen's own answer to a client that is up is a status line of its own, so a
/// sentence sent first would be overwritten before it was ever drawn.
fn report_discarded(discarded: Option<String>, tx: &UnboundedSender<AppEvent>) {
    if let Some(sentence) = discarded {
        let _ = tx.send(AppEvent::Net(Event::SessionDiscarded(sentence)));
    }
}

/// The account's own profile, or why there is not one.
///
/// Read once, here, and not when the panel is opened: the profile does not
/// change under a reader, and a panel that blanked and refilled every time would
/// be a panel they could not trust. A failure is carried rather than raised,
/// because the conversations are here either way and the screen is usable
/// without a profile — the panel says why it has none.
async fn read_account(client: &ProtoClient) -> Result<domain::account::Account, String> {
    match client.fetch_account().await {
        Ok(account) => Ok(account),
        Err(error) => {
            tracing::warn!(%error, "the account's own profile could not be read");
            Err(format!("{error:#}"))
        }
    }
}

/// Where the session is kept, in the words the profile panel says it in.
///
/// The decision, and the only one: [`session_store`] builds the store out of
/// what this returns, so the panel cannot describe a store the session is not
/// in. Reading the same field twice would leave that to a test, and a test
/// cannot see a field read two ways.
fn session_description(cfg: &Config) -> tui::SessionStore {
    match &cfg.session_path {
        Some(path) => tui::SessionStore::EncryptedFile(path.clone()),
        None => tui::SessionStore::Keyring,
    }
}

/// The store the session goes in, and the key behind it when that is a file.
struct Session {
    store: Box<dyn SessionStore>,

    /// The file's key source. `None` for the OS credential store, which needs no
    /// key from here.
    keys: Option<Arc<dyn KeyProvider>>,
}

/// The store itself: the encrypted file the configuration names, or the
/// machine's own credential store.
///
/// Resolved afresh by every bring-up and never kept: a cached key would be a
/// longer-lived secret, and a passphrase changed in the environment is read on
/// the next `:retry`.
fn session_store(cfg: &Config) -> Session {
    session_store_with(cfg, || Arc::new(KeyringKeyProvider::default()))
}

/// [`session_store`], with the OS-keyring key source passed in.
///
/// A seam for the tests only: it is what lets them resolve a store without
/// reaching the real credential store.
fn session_store_with(cfg: &Config, keyring: impl FnOnce() -> Arc<dyn KeyProvider>) -> Session {
    match session_description(cfg) {
        tui::SessionStore::Keyring => Session {
            store: Box::new(KeyringStore::default()),
            keys: None,
        },
        tui::SessionStore::EncryptedFile(path) => {
            let keys = key_provider(cfg, keyring);
            Session {
                store: Box::new(FileStore::with_key_provider(path, Arc::clone(&keys))),
                keys: Some(keys),
            }
        }
    }
}

/// Where the session file's key comes from: the passphrase if there is one,
/// otherwise the OS credential store.
///
/// The order is the whole policy. Neither being usable is not answered here with
/// a third source — least of all with no key at all — but when the provider is
/// first asked for a key, as [`key_trouble`].
fn key_provider(
    cfg: &Config,
    keyring: impl FnOnce() -> Arc<dyn KeyProvider>,
) -> Arc<dyn KeyProvider> {
    match cfg.passphrase() {
        Some(passphrase) => Arc::new(PassphraseProvider::new(passphrase)),
        None => keyring(),
    }
}

/// The `offline:` sentence for a session file whose key is missing or wrong.
///
/// Names what the reader can set, and says the file was left alone, because the
/// alternative they will fear is that it was thrown away.
fn key_trouble(error: &SessionError) -> anyhow::Error {
    match error {
        SessionError::Unavailable(why) => anyhow::anyhow!(
            "the session file is encrypted and no key is available: set \
             TELEVIM_SESSION_PASSPHRASE or make the OS keyring available ({why}); \
             the file was left as it was"
        ),
        other => anyhow::anyhow!(
            "{other}; check TELEVIM_SESSION_PASSPHRASE or the OS keyring; \
             the file was left as it was"
        ),
    }
}

/// Forwards the feed to the loop for as long as it lasts.
///
/// The feed ends when the client shuts down, and `None` is that: the task stops,
/// and the loop is told so it can rebuild the client and take a new feed. A
/// failure while resolving a gap in the sequence is waited out up to
/// [`CHAT_LIST_ATTEMPTS`] times — [`feed_error_retry`]'s answer, slept here in
/// the pump's task so the driver's 250 ms tick never waits on it — and read
/// past while the feed stays usable. Past the bound the feed is ended instead:
/// the loop below records its position and reports it, which is the feed's end
/// asking for the one rebuild.
async fn pump(mut updates: UpdateStream, tx: UnboundedSender<AppEvent>) {
    let mut errors_seen: u8 = 0;
    while let Some(result) = updates.next().await {
        match result {
            Ok(event) => {
                if tx.send(AppEvent::Net(Event::Update(event))).is_err() {
                    // The loop is gone, so there is nothing left to tell.
                    break;
                }
            }
            Err(error) => {
                if let Some(delay) = feed_error_retry(errors_seen, &error) {
                    errors_seen += 1;
                    tracing::warn!(%error, "the update feed reported a failure it can recover from");
                    let retry = FeedRetry {
                        reason: error,
                        delay,
                        attempt: errors_seen,
                        attempts: CHAT_LIST_ATTEMPTS,
                    };
                    if tx.send(AppEvent::Net(Event::FeedRetrying(retry))).is_err() {
                        // The loop is gone, so there is nothing left to tell.
                        break;
                    }
                    tokio::time::sleep(delay).await;
                } else {
                    tracing::warn!(%error, "the update feed kept failing past its bound; ending it so the client is rebuilt");
                    break;
                }
            }
        }
    }

    // The feed has been read to its end, so the position it reached is final.
    // Recording it is an explicit call rather than something the drop does,
    // because it has to be awaited, and the next launch resolves a gap against
    // the stored position — so a session left pointing further back would replay
    // every update in between.
    if let Err(error) = updates.finish().await {
        tracing::warn!(%error, "the update position could not be recorded");
    }

    // The end is reported **after** the position is recorded: the app rebuilds
    // the client and only drops the old one when the new `Ready` replaces it, so
    // this ordering is what keeps the position syncing against the client whose
    // store it belongs to (G9). If the loop is already gone there is nowhere to
    // report it to.
    let _ = tx.send(AppEvent::Net(Event::FeedEnded));
}

/// Asks for whatever page the screen is about to need.
///
/// Called after every event and every tick, so it has to be cheap and it has to
/// be idempotent: what it decides is [`wanted`]'s, and what it does is start one
/// fetch and record that it did.
pub fn drive(app: &mut App, state: &mut State, tx: &UnboundedSender<AppEvent>) {
    // A transient status is the one thing here that expires on its own, and a
    // frame cannot expire it: `status_text` is read from a shared reference.
    // This runs every pass, so it is the natural clock.
    app.expire_status(Instant::now());
    // The peer's typing note expires the same way and for the same reason: it is
    // state an event set, and the loop's tick is the only thing that can take it
    // back, because a frame is drawn from a shared reference.
    app.expire_typing(Instant::now());

    // Nothing is open, so there is no conversation for a cursor to describe —
    // nor a jump to be waiting on, because closing a conversation forgets one.
    if app.conversation.conversation.window.chat_id == 0 {
        state.history.cursor = None;
        state.history.jump = None;
    }

    // A chat the reader has highlighted and stopped on shows its card. Taken
    // before the client is looked up, so the card's one contact read is asked
    // for on the same pass the highlight settled on.
    app.open_settled_card(Instant::now());

    // Read here, before the client is looked up, and not as an action: the state a
    // retry is asked for in is the state with no client, where the action drain
    // below never runs.
    if app.take_retry_request() {
        if state.bringing_up {
            app.flash("already trying to connect");
        } else if let (Some(cfg), Some(tx)) = (state.cfg.clone(), state.tx.clone()) {
            state.bringing_up = true;
            spawn_bring_up(cfg, tx);
        }
    }

    // The feed ended, so the client is rebuilt and a new feed taken. Read in the
    // same place as the reader's retry and for the same reason — the state a
    // reconnect is needed in has no feed at all. A bring-up already in flight is
    // not stacked on, and the one automatic reconnect is bounded: a second feed
    // end before an update has cleared the flag is the reader-visible failure,
    // exactly as a launch that spent every chat-list attempt is.
    if state.take_reconnect_request() {
        if auto_reconnect(state) {
            state.auto_reconnect_used = true;
            if let (Some(cfg), Some(tx)) = (state.cfg.clone(), state.tx.clone()) {
                state.bringing_up = true;
                spawn_bring_up(cfg, tx);
            }
        } else if !state.bringing_up {
            apply_offline(app, state, &anyhow::anyhow!("the update feed ended again"));
        }
    }

    // An arrival under a card is kept rather than spent: the marker it is owed
    // is refused for as long as the card covers the conversation, and the pass
    // after the reader backs out of it asks again.
    let arrived = !card_covers_conversation(app) && state.read_owed.take().is_some();
    let Some(client) = state.client.clone() else {
        // No client, so nothing is asked for — but a conversation opened
        // while there is none is still owed what the cache holds for it, or
        // an offline reader switching chats finds every one of them empty.
        // The cursor is left alone: the newest page is still unasked for, and
        // the client that comes up asks for it.
        seed_opened(app, state);
        return;
    };

    // The reader's outbound requests are drained before `wanted`, which returns
    // early while a history page backs off and while nothing is open: a send
    // must not be held up by either. An action is left in place while there is
    // no client, because a message typed offline must not be thrown away.
    while let Some(action) = app.take_action() {
        // A media open is resolved here, where the conversation on show is: the
        // download task has no state to ask, and a message that is not on show
        // has no kind to name its file by.
        let media = match &action {
            Action::OpenMedia {
                chat_id,
                message_id,
            } => {
                let kind = open_media_kind(&app.conversation.conversation, *chat_id, *message_id);
                let media_id = open_media_id(&app.conversation.conversation, *chat_id, *message_id);
                kind.zip(state.media_cache.clone()).map(|(kind, cache)| {
                    "downloading media…".clone_into(&mut app.ui.status);
                    app.conversation.downloads.start(*chat_id, *message_id);
                    let cancel = Arc::new(AtomicBool::new(false));
                    state.media_cancel.push(MediaCancel {
                        chat_id: *chat_id,
                        message_id: *message_id,
                        flag: Arc::clone(&cancel),
                    });
                    MediaJob {
                        kind,
                        media_id,
                        cancel,
                        cache,
                    }
                })
            }
            _ => None,
        };
        request_action(&client, state, action, media, tx);
    }

    let next = wanted(app, state.history, Instant::now());
    // An arrival in the open conversation asks for the marker again, through the
    // same target as opening does. A Latest does its own just below, once its
    // window has begun, so it is not asked twice on one pass.
    if arrived
        && !matches!(next, Wanted::Latest(_))
        && let Some((chat_id, max_id)) = read_target(app, state)
    {
        request_read(&client, chat_id, max_id, tx);
    }

    match next {
        Wanted::Nothing => {}

        Wanted::Latest(peer_id) => {
            let cursor = begin_latest(app, state, peer_id);
            if let Some((chat_id, max_id)) = latest_read(app, state) {
                request_read(&client, chat_id, max_id, tx);
            }
            request(&client, FetchDirection::Latest, cursor, tx);
        }

        Wanted::Page(direction, cursor) => {
            app.begin_fetch(direction);
            request(&client, direction, cursor, tx);
        }

        Wanted::Jump(jump) => {
            // `wanted` only names a jump once the conversation has a cursor, so
            // this is the same guard it read: a page that replaces a window has
            // to be reported to the cursor that describes it.
            let Some(cursor) = state.history.cursor else {
                return;
            };

            // Recorded before the request, like the cursor above, and for the
            // same reason: the reader holding the key must not turn into a
            // request per pass.
            state.history.jump = Some(jump);
            request_jump(&client, jump, cursor, tx);
        }
    }
}

/// Starts on a conversation's newest page: puts what the cache holds for it on
/// screen, and records the fetch that will replace it.
///
/// Everything [`drive`]'s `Latest` arm does short of the request itself, which
/// needs a client: these effects can be checked without one, the way
/// [`apply_ready_to_screen`]'s are.
///
/// **Every way of opening a conversation ends here** — the chat list, `:chat`,
/// a search result, the launch landing and `--chat` — because each of them
/// leaves a window the cursor does not name, and that is what [`wanted`] answers
/// with the newest page. It runs on the same pass as the open, so the first
/// frame drawn after it is the cached window, not the `Loading…` row.
///
/// The seed is [`seed_opened`]'s, which a driver with no client runs on its
/// own.
fn begin_latest(app: &mut App, state: &mut State, peer_id: i64) -> HistoryCursor {
    seed_opened(app, state);

    let cursor = HistoryCursor::new(peer_id);
    // Recorded before the request rather than after it, so that the next pass —
    // which is a quarter of a second away — does not ask for the same page again
    // while this one is still on its way.
    state.history.cursor = Some(cursor);
    app.begin_fetch(FetchDirection::Latest);
    cursor
}

/// Puts what the cache holds for the conversation just opened on screen.
///
/// *Just opened* is the cursor's word, the one [`wanted`] reads: a
/// conversation the cursor does not name has not had its newest page begun.
/// Once one has, an empty window is the wire's answer — a conversation with
/// nothing in it — and laying cached rows over it would show the reader
/// messages the server has just said are not there.
///
/// The cache is read only for an empty window. A window with rows in it would
/// refuse the seed — a retry after a newest page that failed, or a client
/// brought back up over the conversation the reader was in — and reading the
/// cache for it would copy a page out only to throw it away.
///
/// Needs no client, so it is the whole of an open when there is none:
/// [`begin_latest`] runs it and then asks; [`drive`] runs it alone while
/// offline.
fn seed_opened(app: &mut App, state: &State) {
    let open = app.conversation.conversation.window.chat_id;
    if open == 0
        || state
            .history
            .cursor
            .is_some_and(|cursor| cursor.peer_id() == open)
        || !app.conversation.conversation.window.is_empty()
    {
        return;
    }
    app.seed_from_cache(open, state.cached_history(open));
}

/// Whether a feed that has just ended may be reconnected, or has spent its one
/// automatic attempt.
///
/// A pure answer over [`State`], so the bound can be checked without a client or
/// a runtime, the way [`chat_list_retry`]'s is. Two refusals, and they mean
/// different things to the caller: `bringing_up` says a bring-up is already on
/// its way and will answer for itself, while [`State::auto_reconnect_used`] says
/// the one attempt is gone and this feed's end is the failure.
fn auto_reconnect(state: &State) -> bool {
    !state.bringing_up && !state.auto_reconnect_used
}

/// Decides which page to ask for, from what the screen and the cursor say.
fn wanted(app: &App, history: History, now: Instant) -> Wanted {
    let open = app.conversation.conversation.window.chat_id;
    if open == 0 {
        return Wanted::Nothing;
    }

    // A fetch that failed holds every direction for a while. Asking again at
    // once is what turns a throttle into a storm, and the page is not going
    // anywhere.
    if history.retry_at.is_some_and(|at| now < at) {
        return Wanted::Nothing;
    }

    // A conversation the cursor does not name has just been opened, and the page
    // it needs is the newest one: there is nothing loaded to page from either
    // end of. A jump waits behind this rather than beside it, because its answer
    // is reported to the same cursor and there is not one yet.
    let Some(cursor) = history.cursor.filter(|cursor| cursor.peer_id() == open) else {
        return Wanted::Latest(open);
    };

    // A jump outranks paging: the window it is going to be answered in has not
    // been fetched, so a page from an end of the one on show is work the
    // replacement would throw away.
    if let Some(jump) = app.pending_jump() {
        return if history.jump == Some(jump) {
            Wanted::Nothing
        } else {
            Wanted::Jump(jump)
        };
    }

    // Older first: a window shorter than the margin is near both of its ends at
    // once, and the reader is more often looking for what came before.
    if app.wants_older() {
        return Wanted::Page(FetchDirection::Older, cursor);
    }
    if app.wants_newer() {
        return Wanted::Page(FetchDirection::Newer, cursor);
    }

    Wanted::Nothing
}

/// The conversation and newest message the read marker should go up to on this
/// open, if one should be sent.
///
/// Only the conversation on show is considered, and only while its list entry
/// still has unread messages. The ceiling is the newest message the list knows
/// of (`last_message_id`), not the window's end. A missing or non-positive id is
/// never sent, because it names no message the server holds. A ceiling already
/// accepted is not sent again.
fn read_target(app: &App, state: &State) -> Option<(i64, i64)> {
    let open = app.conversation.conversation.window.chat_id;
    if open == 0 || card_covers_conversation(app) {
        return None;
    }
    let chat = app.chats().iter().find(|chat| chat.id == open)?;
    if chat.unread_count == 0 {
        return None;
    }
    let max_id = chat.last_message_id.filter(|id| *id > 0)?;
    (state.read_acked != Some((open, max_id))).then_some((open, max_id))
}

/// The read marker a Latest page asks for, or none.
///
/// A card that covers the conversation refuses the marker, so the conversation
/// is owed it: [`State::read_owed`] keeps it for the pass after the reader backs
/// out, which asks again through [`read_target`].
fn latest_read(app: &App, state: &mut State) -> Option<(i64, i64)> {
    let target = read_target(app, state);
    let open = app.conversation.conversation.window.chat_id;
    if target.is_none() && card_covers_conversation(app) && open != 0 {
        state.read_owed = Some(open);
    }
    target
}

/// Whether a profile card is on show in the conversation's place.
///
/// The window is only ever replaced by a real open, so a conversation the
/// reader has not confirmed is never the one on show: the card is what covers
/// it. Browsing a card therefore keeps the marker back, and confirming the card
/// (or backing out of it) is what puts the conversation in front of the reader
/// again.
fn card_covers_conversation(app: &App) -> bool {
    matches!(app.ui.pane, tui::app::Pane::Profile(_))
}

/// Tells Telegram the reader has read a conversation up to `max_id`, and hands
/// the answer back to the loop.
///
/// The same shape as [`request`], and for the same reason: the round trip is its
/// own task. Only an accepted marker is reported, and a refused one is logged and
/// dropped — there is no retry, and the count stays, so the next open asks again.
fn request_read(
    client: &Arc<ProtoClient>,
    chat_id: i64,
    max_id: i64,
    tx: &UnboundedSender<AppEvent>,
) {
    let client = Arc::clone(client);
    let tx = tx.clone();

    tokio::spawn(async move {
        match client.mark_read(chat_id, max_id).await {
            Ok(()) => {
                let _ = tx.send(AppEvent::Net(Event::ReadMarked { chat_id, max_id }));
            }
            Err(error) => {
                tracing::debug!(%error, chat_id, max_id, "the read marker was not accepted; the unread count is kept");
            }
        }
    });
}

/// Asks for one page, and hands the answer back to the loop.
///
/// The fetch runs as its own task rather than being awaited where it is asked
/// for: the event loop is the only thing drawing the screen, and a round trip is
/// long enough that awaiting one would stop the reader's keystrokes from being
/// read at all.
fn request(
    client: &Arc<ProtoClient>,
    direction: FetchDirection,
    cursor: HistoryCursor,
    tx: &UnboundedSender<AppEvent>,
) {
    let client = Arc::clone(client);
    let tx = tx.clone();

    tokio::spawn(async move {
        let mut cursor = cursor;
        let anchor = match direction {
            FetchDirection::Latest => None,
            FetchDirection::Older => cursor.oldest_loaded_id(),
            FetchDirection::Newer => cursor.newest_loaded_id(),
        };

        let result = match direction {
            FetchDirection::Latest => client.fetch_latest(cursor.peer_id(), PAGE).await,
            FetchDirection::Older => client.fetch_older(&mut cursor, PAGE).await,
            FetchDirection::Newer => client.fetch_newer(&mut cursor, PAGE).await,
        };

        // The loop may have gone; there is then nothing to report the page to.
        let _ = tx.send(AppEvent::Net(Event::History {
            direction,
            anchor,
            cursor,
            result,
        }));
    });
}

/// Asks for a page around the message the reader jumped to, and hands the answer
/// back to the loop.
///
/// The same shape as [`request`], and for the same reason: the round trip is its
/// own task so that a jump does not stop the reader's keystrokes from being read.
fn request_jump(
    client: &Arc<ProtoClient>,
    jump: Jump,
    cursor: HistoryCursor,
    tx: &UnboundedSender<AppEvent>,
) {
    let client = Arc::clone(client);
    let tx = tx.clone();

    tokio::spawn(async move {
        let result = client
            .fetch_around(jump.peer_id, jump.target_id, PAGE)
            .await;

        let _ = tx.send(AppEvent::Net(Event::Jumped {
            jump,
            cursor,
            result,
        }));
    });
}

/// Whether a query is shaped like a `@username` rather than a name.
///
/// The rule is the framework's `normalize_username`, repeated here because it is
/// private there and this module — not `tui` — is where the routing decision
/// lives: a handle is what remains after an optional leading `@` and any
/// surrounding whitespace, and a name is any query with whitespace left in it.
/// An empty handle is not a handle, so a bare `@` falls to the name search.
fn looks_like_username(query: &str) -> bool {
    let trimmed = query.trim();
    let name = trimmed.strip_prefix('@').unwrap_or(trimmed);
    !name.is_empty() && !name.chars().any(char::is_whitespace)
}

/// The one person to open directly, when a result set is exactly one.
///
/// A single candidate — a resolved handle, or a name only one contact matches —
/// is the answer the reader asked for rather than a list to choose from (Q4).
/// Zero or many means the list is shown.
fn sole_candidate(users: &[UserCandidate]) -> Option<&UserCandidate> {
    match users {
        [only] => Some(only),
        _ => None,
    }
}

/// What a `ResolveUser` action found, before it becomes an event.
enum Lookup {
    /// The exact username resolved to one person.
    Resolved(UserCandidate),

    /// The name search listed the people it matched.
    Listed(Vec<UserCandidate>),
}

/// Answers a `ResolveUser`: resolve a handle exactly, or search a name.
///
/// The routing is Q5's: a username-shaped query is tried through the exact
/// resolver first, and only a handle nobody owns falls through to the contact
/// search — a stranger is reachable by handle and by nothing else, while a name
/// is never lost to the handle path because that path declines it first. A
/// name-shaped query goes straight to the search.
async fn lookup(client: &ProtoClient, query: &str) -> Result<Lookup, ProtoError> {
    if looks_like_username(query)
        && let Some(user) = client.resolve_user(query).await?
    {
        return Ok(Lookup::Resolved(user));
    }

    Ok(Lookup::Listed(
        client.search_users(query, SEARCH_MATCHES).await?,
    ))
}

/// Performs the operation the reader asked for, and hands the answer back.
///
/// The same shape as the two above, and for the same reason: a round trip in the
/// event loop would stop the reader's keystrokes from being read while it runs.
///
/// It takes the state because the sign-in is the one operation whose *input* is
/// a `proto` token rather than a value the reader typed: the code and the
/// two-factor challenge are `proto` types `tui` may not name, so they are spent
/// here. Both are taken **before** the task is spawned, because a task that
/// borrowed them would outlive this function and neither token can be copied.
fn request_action(
    client: &Arc<ProtoClient>,
    state: &mut State,
    action: Action,
    media: Option<MediaJob>,
    tx: &UnboundedSender<AppEvent>,
) {
    // The sign-out. It needs the client, so it cannot fall through to
    // `request_plain` below with everything else: the session this destroys is
    // reached through it and through nothing else.
    if let Action::Logout = action {
        let client = Arc::clone(client);
        let tx = tx.clone();
        tokio::spawn(async move {
            let result = client.logout().await.map_err(|error| format!("{error:#}"));
            let _ = tx.send(AppEvent::Net(Event::LoggedOut { result }));
        });
        return;
    }

    // The reader's cancel sets a flag the transfer reads, so it is answered from
    // the state the flags live in and never reaches the network.
    if let Action::CancelMediaDownload {
        chat_id,
        message_id,
    } = action
    {
        state.cancel_media(chat_id, message_id);
        return;
    }

    // The reader giving up on the step. Nothing to ask for: `tui` has already put
    // its own state back to the phone, and the tokens that step was reached with
    // are what has to go with it.
    let action = match action {
        Action::LoginCancelled => {
            state.login.code = None;
            state.login.challenge = None;
            return;
        }
        other => other,
    };

    let Action::Login { field, value } = action else {
        request_plain(client, action, media, tx);
        return;
    };

    let client = Arc::clone(client);
    let tx = tx.clone();
    let session_store = state.session_store.clone().unwrap_or_default();

    match field {
        LoginField::Phone => {
            tokio::spawn(async move {
                // Asked for locally, because Telegram's own answer to an empty
                // number is a throttled round trip that says nothing a reader
                // typing `⏎` on a blank field cannot be told here.
                let phone = value.trim();
                let result = if phone.is_empty() {
                    Err(ProtoError::Auth {
                        refusal: Refusal::PhoneInvalid,
                        detail: None,
                    })
                } else {
                    client.request_login_code(phone).await
                };
                let _ = tx.send(AppEvent::Net(Event::CodeRequested {
                    phone: value,
                    result,
                }));
            });
        }

        LoginField::Code => {
            let Some(code) = state.login.code.take() else {
                // Nothing to redeem: the step was left without answering it, and
                // a request with no hash would burn a login attempt on nothing.
                tracing::debug!("a login code was submitted with no code to redeem it");
                return;
            };
            tokio::spawn(async move {
                let result = client.sign_in(code, value.trim()).await;
                report_sign_in(
                    |result| Event::SignedIn { result },
                    client,
                    result,
                    session_store,
                    &tx,
                )
                .await;
            });
        }

        LoginField::Password => {
            let Some(challenge) = state.login.challenge.take() else {
                tracing::debug!("a password was submitted with no challenge to check it against");
                return;
            };
            tokio::spawn(async move {
                let result = client.check_password(challenge, value.trim()).await;
                report_sign_in(
                    |result| Event::PasswordChecked { result },
                    client,
                    result,
                    session_store,
                    &tx,
                )
                .await;
            });
        }
    }
}

/// Reports a finished sign-in step, and does what being signed in means.
///
/// The answer is wrapped in `as_event` rather than hard-coded, because the two
/// steps are two events: a panel counting password attempts left must not count
/// them against a login code. The account answer is sent first, so the overlay
/// comes down while the chat list is still on its way; a step that did not finish
/// — a refusal, or the password step still to come — reports itself and stops,
/// because there is nothing to fetch for an account that is not signed in yet.
async fn report_sign_in(
    as_event: fn(Result<SignIn, ProtoError>) -> Event,
    client: Arc<ProtoClient>,
    result: Result<SignIn, ProtoError>,
    session_store: tui::SessionStore,
    tx: &UnboundedSender<AppEvent>,
) {
    match result {
        Ok(SignIn::Account(account)) => {
            let _ = tx.send(AppEvent::Net(as_event(Ok(SignIn::Account(
                account.clone(),
            )))));

            // The same tail bring-up runs, and deliberately tolerantly: a
            // sign-in that worked and a chat list that did not is a signed-in
            // account with nothing on show, which is drawable. `Offline` would
            // wipe the account the reader just earned.
            let chats = match client.fetch_private_chats().await {
                Ok(chats) => chats,
                Err(error) => {
                    tracing::warn!(%error, "the chat list could not be fetched after signing in");
                    Vec::new()
                }
            };
            // The launch path reads the account's own profile, and this one has
            // to reach the same screen: the login hand-back carries the identity
            // and nothing else, and the card does not re-read on open, so a bio
            // or a birthday would be missing until the next launch. A failed read
            // falls back to the hand-back rather than losing the sign-in.
            let account = match read_account(&client).await {
                Ok(full) => full,
                Err(_) => account,
            };
            match client.subscribe_updates().await {
                Ok(updates) => {
                    tokio::spawn(pump(updates, tx.clone()));
                }
                Err(error) => {
                    tracing::warn!(%error, "the update feed could not be taken after signing in");
                }
            }

            let _ = tx.send(AppEvent::Net(Event::Ready {
                client,
                chats,
                account: Ok(account),
                session_store,
            }));
        }

        other => {
            let _ = tx.send(AppEvent::Net(as_event(other)));
        }
    }
}

/// Everything a sign-in step is *not*: an operation on a conversation, or a
/// question about somebody.
fn request_plain(
    client: &Arc<ProtoClient>,
    action: Action,
    media: Option<MediaJob>,
    tx: &UnboundedSender<AppEvent>,
) {
    let client = Arc::clone(client);
    let tx = tx.clone();

    tokio::spawn(async move {
        match action {
            Action::Send {
                chat_id,
                temp_id,
                text,
                reply_to,
            } => {
                let result = client.send_message(chat_id, &text, reply_to).await;
                let _ = tx.send(AppEvent::Net(Event::Sent {
                    chat_id,
                    temp_id,
                    result,
                }));
            }

            Action::Edit {
                chat_id,
                message_id,
                text,
            } => {
                let result = client.edit_message(chat_id, message_id, &text).await;
                let _ = tx.send(AppEvent::Net(Event::Edited {
                    chat_id,
                    message_id,
                    result,
                }));
            }

            Action::Delete {
                chat_id,
                message_ids,
            } => {
                let result = client.delete_messages(chat_id, &message_ids).await;
                let _ = tx.send(AppEvent::Net(Event::Deleted {
                    chat_id,
                    message_ids,
                    result,
                }));
            }

            Action::Forward {
                chat_id,
                message_ids,
                dest_chat_id,
            } => {
                let event = forward(&client, chat_id, message_ids, dest_chat_id).await;
                let _ = tx.send(AppEvent::Net(event));
            }

            Action::TogglePin { chat_id, pinned } => {
                let result = client.toggle_pin(chat_id, pinned).await;
                let _ = tx.send(AppEvent::Net(Event::PinToggled {
                    chat_id,
                    pinned,
                    result,
                }));
            }

            // A profile is a question about a person rather than an operation on
            // a conversation, for the same reason as the search below, and it shares
            // this task's shape for the same reason: a round trip here would stop
            // the reader's keystrokes being read while it runs.
            Action::FetchContact { peer_id } => {
                let result = client.fetch_user(peer_id).await;
                let _ = tx.send(AppEvent::Net(Event::Contact { peer_id, result }));
            }

            // A search is not an operation on the conversation's messages: it
            // asks a question and returns places. It shares this task's shape
            // for the same reason as the three above — a round trip here would
            // stop the reader's keystrokes being read.
            Action::Search { chat_id, query } => {
                let result = client.search(chat_id, &query, SEARCH_MATCHES).await;
                let _ = tx.send(AppEvent::Net(Event::Searched {
                    chat_id,
                    query,
                    result,
                }));
            }

            // The sign-in actions are handled by `request_action`, which has to
            // reach the state before this function exists. `Logout` is there too:
            // it needs the client, so it is asked for there rather than here.
            // Unreachable rather than wrong: a value is one of the three, never
            // two.
            // The cancel is answered by `request_action`, which holds the flag;
            // nothing is asked of the network for it either.
            Action::Login { .. }
            | Action::LoginCancelled
            | Action::Logout
            | Action::CancelMediaDownload { .. } => {}

            // The download is the network's and the file it leaves is the
            // viewer's. A message with no kind resolved is refused here rather
            // than in `drive`, so the refusal reaches the status line the same
            // way a failed fetch does: as an event.
            Action::OpenMedia {
                chat_id,
                message_id,
            } => {
                let event = save_media(client.as_ref(), &tx, chat_id, message_id, media).await;
                let _ = tx.send(AppEvent::Net(event));
            }

            // A person lookup is a question about a person rather than an
            // operation on a conversation, and it shares this task's shape for
            // the same reason as the search above: a round trip here would stop
            // the reader's keystrokes being read while it runs. The answer is
            // one of three events, chosen by what the lookup found.
            Action::ResolveUser { query } => {
                let event = match lookup(client.as_ref(), &query).await {
                    Ok(Lookup::Resolved(user)) => Event::UserResolved {
                        query,
                        user: Some(user),
                    },
                    Ok(Lookup::Listed(users)) => Event::UsersListed { query, users },
                    Err(error) => Event::UserLookupFailed {
                        query,
                        reason: failure_reason(&error),
                    },
                };
                let _ = tx.send(AppEvent::Net(event));
            }
        }
    });
}

/// Puts a signed-out screen up, or says why the sign-out did not happen.
///
/// **The client goes on success and stays on failure.** The framework's
/// `logout` revokes the key over the network warn-only and fails only when the
/// *local* session could not be cleared, so an error means the reader is still
/// signed in: there is nothing to take down and nothing to replace, and wiping
/// the screen on a failure would leave a signed-in reader looking at a signed-out
/// program.
///
/// **The sign-in field is deliberately not opened here.** It is [`Event::Ready`]
/// that opens it, and opening it twice is not harmless: `set_chats` with an empty
/// list calls `select_chat_none`, which forgets the line's purpose — so a field
/// opened first would lose `PromptKind::Phone` and the reader's next `⏎` would
/// take the message path. This arm installs the screen; the `Ready` that follows
/// installs the screen and the field together.
fn apply_logged_out(app: &mut App, state: &mut State, result: Result<(), String>) {
    match result {
        Ok(()) => {
            // Dropping the `Arc` is the point: the client owns the update relay
            // and aborts its runner when it goes, so the pump feeding this
            // screen ends with it.
            state.client = None;
            app.set_chats(Vec::new());
            // The words go with the account: a peer id can be reused by
            // another one, and the in-memory clear above already flows to the
            // file on the next sync — this removes the file itself, so no
            // empty payload is left behind. Best-effort: `clear` only warns,
            // and a failed delete must not fail the logout.
            if let Some(file) = &state.draft_file {
                file.clear();
            }
            // The cached messages go too, for the same reason and with the
            // same best-effort: another account must never open onto them.
            // From memory as well as from disk, or the next write would put
            // them back.
            state.forget_history();
            state.forget_media();
            // The empty reason is the signed-out *state*, not a missing one: the
            // reader chose this, so a line saying why it could not read a
            // profile would be an excuse nobody asked for.
            app.set_account(Err(String::new()));
            "signed out".clone_into(&mut app.ui.status);
        }

        // A real failure, so the real-failure wording. The deliberate-refusal
        // words are gone with the refusal they belonged to.
        Err(reason) => app.ui.status = format!("could not sign out: {reason}"),
    }
}

/// Folds something that arrived from the network into the screen's state.
// One arm per event by design; the media events are four patterns of it.
#[allow(clippy::too_many_lines)]
pub fn apply(app: &mut App, state: &mut State, event: Event) {
    match event {
        Event::Ready {
            client,
            chats,
            account,
            session_store,
        } => apply_ready(app, state, client, chats, account, session_store),

        // Nothing failed, so nothing is an `Offline`: the screen's answer is a
        // sentence about the configuration, which is what the flow draws.
        Event::NoCredentials => {
            app.begin_no_credentials();
            "televim has no application credentials".clone_into(&mut app.ui.status);
        }

        // A flash would be the wrong lifetime here. The sentence is the only
        // account of what happened to the session a reader was signed in with,
        // and a reader who reaches the sign-in form a minute later is looking at
        // a screen they did not expect and has to be told why. It says nothing
        // about the client: there is one either way, and the `Ready` beside it is
        // what puts the screen up.
        Event::SessionDiscarded(sentence) => {
            app.ui.status = sentence;
        }

        Event::CodeRequested { phone, result } => match result {
            Ok(code) => {
                state.login.code = Some(code);
                // The number the request went out with, not the pre-fill: the
                // row under the code says where Telegram sent it.
                app.session.phone.clone_from(&phone);
                app.login_advanced(domain::session::SessionState::AwaitingCode { phone }, None);
            }
            Err(error) => apply_login_refusal(app, &error, 0),
        },

        Event::SignedIn { result } | Event::PasswordChecked { result } => {
            apply_sign_in(app, state, result);
        }

        // A contact's profile, and nothing else: no status and no cursor. The
        // card is already on show and already says it is waiting, so the answer
        // fills it in — and an answer for a card the reader has left is dropped by
        // `set_contact`, which is the only place that knows which card is on show.
        Event::Contact { peer_id, result } => {
            app.set_contact(peer_id, result.map_err(|error| format!("{error:#}")));
        }

        Event::Offline(reason) => apply_offline(app, state, &reason),

        Event::ChatListRetrying(retry) => apply_chat_list_retrying(app, &retry),

        Event::FeedRetrying(retry) => {
            // The feed reads past the failure, so what it failed to deliver
            // may be gone: no cached run is known current behind it.
            state.cached.messages.feed_interrupted();
            apply_feed_retrying(app, &retry);
        }

        Event::FeedEnded => apply_feed_ended(app, state),

        Event::ReadMarked { chat_id, max_id } => {
            state.read_acked = Some((chat_id, max_id));
            app.mark_chat_read(chat_id);
        }

        Event::LoggedOut { result } => apply_logged_out_and_reconnect(app, state, result),

        Event::Update(event) => {
            // An update is the feed working, so it earns the next automatic
            // reconnect: without this, a reconnect that re-subscribes to a feed
            // that immediately ends again would rebuild the client for ever.
            state.auto_reconnect_used = false;
            // And the connection holds: a working feed is a connected one.
            app.set_connection(ConnectionState::Connected);
            state.remember_update(&event);
            if let UpdateEvent::NewMessage(message) = &event
                && message.chat_id == app.conversation.conversation.window.chat_id
            {
                state.read_owed = Some(message.chat_id);
            }
            // Whether it moved anything is not acted on: the loop redraws on
            // every pass, so the report has no decision to feed here.
            let _ = app.apply_update(&event);
        }

        Event::Searched {
            chat_id,
            query,
            result,
        } => apply_searched(app, chat_id, &query, result),

        Event::UserResolved { query, user } => apply_user_resolved(app, &query, user),
        Event::UsersListed { query, users } => apply_users_listed(app, &query, users),
        Event::UserLookupFailed { query, reason } => apply_user_lookup_failed(app, &query, reason),
        Event::Sent {
            chat_id,
            temp_id,
            result,
        } => {
            // Cached whether or not the reader is still in the conversation:
            // the screen drops an answer it has left, the cache does not.
            if let Ok(message) = &result {
                state.remember_sent(message);
            }
            apply_sent(app, chat_id, temp_id, result);
        }

        Event::Edited {
            chat_id,
            message_id,
            result,
        } => apply_edited(app, chat_id, message_id, result),

        Event::PinToggled {
            chat_id,
            pinned,
            result,
        } => apply_pin_toggled(app, chat_id, pinned, result),

        Event::Deleted {
            chat_id,
            message_ids,
            result,
        } => {
            // Before the answer: a failed batch may have removed some of them.
            state.forget_media_messages(&message_ids);
            apply_deleted(app, chat_id, &message_ids, result);
        }

        Event::Forwarded {
            chat_id,
            dest_chat_id,
            requested,
            result,
        } => apply_forwarded(app, chat_id, dest_chat_id, requested, result),

        Event::History {
            direction,
            anchor,
            cursor,
            result,
        } => apply_history(app, state, direction, anchor, cursor, result),

        Event::Jumped {
            jump,
            cursor,
            result,
        } => apply_jumped(app, state, jump, cursor, result),

        Event::StickerSettled {
            chat_id,
            message_id,
            fetched,
        } => apply_sticker_settled(app, chat_id, message_id, fetched),

        Event::MediaSaved { .. }
        | Event::MediaFailed { .. }
        | Event::MediaCancelled { .. }
        | Event::MediaProgress { .. } => apply_media(app, state, event),
    }
}

fn apply_sticker_settled(
    app: &mut App,
    chat_id: i64,
    message_id: i64,
    fetched: Result<Vec<u8>, String>,
) {
    // A reader who has moved to another chat has had the cache cleared, the
    // in-flight mark with it. The picture belongs to the chat on show, so this
    // one is dropped rather than cached under a message id it does not match.
    if app.current_chat_id() != chat_id {
        return;
    }
    tui::sticker::resolve_fetch(&mut app.conversation.stickers, chat_id, message_id, fetched);
}

/// A media download in flight, as the loop keeps it: the message, and the flag
/// its cancel sets. The flag is the only thing the loop and the transfer share.
struct MediaCancel {
    chat_id: i64,
    message_id: i64,
    flag: Arc<AtomicBool>,
}

/// What one media download needs to run: the kind its file is stored under, the
/// Telegram media id it is also stored under when the message carries one, the
/// flag that stops it, and the cache it is looked up in and stored into.
struct MediaJob {
    kind: MediaKind,
    media_id: Option<i64>,
    cancel: Arc<AtomicBool>,
    cache: Arc<Mutex<MediaCache>>,
}

/// The status sentence for an open whose message is not on the screen, so its
/// kind, and so its file name, is not known.
const MEDIA_NOT_LOADED: &str = "that message's media is not loaded; nothing was downloaded";

/// The kind of the media on `message_id` in `chat_id`, when the conversation on
/// show holds that message with media.
///
/// `None` for a message that is not on show or carries nothing: the file's name
/// needs the kind, and a message the reader can no longer see is refused rather
/// than fetched without one.
fn open_media_kind(view: &ConversationView, chat_id: i64, message_id: i64) -> Option<MediaKind> {
    view.message(message_id)
        .filter(|message| message.chat_id == chat_id)
        .and_then(|message| message.media)
}

/// The Telegram media id of the media on `message_id` in `chat_id`, when the
/// conversation on show holds that message and it carries one.
fn open_media_id(view: &ConversationView, chat_id: i64, message_id: i64) -> Option<i64> {
    view.message(message_id)
        .filter(|message| message.chat_id == chat_id)
        .and_then(|message| message.media_id)
}

/// The status sentence for a download that wrote no file.
///
/// Every failure the download can report gets a sentence, so none is silent.
/// The limit is named for an oversize media, because it is the one thing a
/// reader can act on.
fn media_failure(error: &ProtoError) -> String {
    match error {
        ProtoError::Framework(FrameworkError::MediaTooLarge { .. }) => format!(
            "media is over the {} MiB limit; nothing was saved",
            MEDIA_LIMIT / (1024 * 1024)
        ),
        ProtoError::Framework(FrameworkError::MediaUnavailable { .. }) => {
            "this message carries no media televim can fetch; nothing was saved".to_owned()
        }
        ProtoError::MessageIdOutOfRange { .. } => {
            "that message id is outside telegram's range; nothing was saved".to_owned()
        }
        other => format!("media download failed: {other}; nothing was saved"),
    }
}

/// The progress callback of one download: reports each chunk on the loop's
/// channel, and answers `false` once the reader has cancelled, which stops the
/// transfer before the next chunk.
///
/// The send is unbounded, so the chunk loop never waits on the loop draining
/// events; progress is one event per chunk, which bounds how many there are.
fn media_progress(
    tx: UnboundedSender<AppEvent>,
    chat_id: i64,
    message_id: i64,
    cancel: Arc<AtomicBool>,
) -> impl FnMut(usize, Option<usize>) -> bool {
    move |downloaded, total| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }

        let _ = tx.send(AppEvent::Net(Event::MediaProgress {
            chat_id,
            message_id,
            downloaded,
            total,
        }));

        true
    }
}

/// Downloads the media on a message into the cache, or serves it from there.
///
/// A message already cached is answered from disk with no request on the wire.
/// A miss downloads, then stores the bytes; the store is disk I/O, so it runs
/// off the loop.
async fn save_media(
    client: &ProtoClient,
    tx: &UnboundedSender<AppEvent>,
    chat_id: i64,
    message_id: i64,
    media: Option<MediaJob>,
) -> Event {
    // A message with no kind resolved is refused here, so the refusal reaches the
    // status line the same way a failed fetch does: as an event.
    let Some(job) = media else {
        return Event::MediaFailed {
            chat_id,
            message_id,
            reason: MEDIA_NOT_LOADED.to_owned(),
        };
    };

    if let Some(event) = cached_media(&job.cache, chat_id, message_id, job.media_id) {
        return event;
    }

    let progress = media_progress(tx.clone(), chat_id, message_id, job.cancel);
    let bytes = match client
        .download_media_with_progress(chat_id, message_id, progress)
        .await
    {
        Ok(bytes) => bytes,
        Err(ProtoError::Framework(FrameworkError::DownloadAborted { .. })) => {
            return Event::MediaCancelled {
                chat_id,
                message_id,
            };
        }
        Err(error) => {
            return Event::MediaFailed {
                chat_id,
                message_id,
                reason: media_failure(&error),
            };
        }
    };

    let cache = Arc::clone(&job.cache);
    let kind = job.kind;
    let media_id = job.media_id;
    tokio::task::spawn_blocking(move || {
        keep_download(&cache, chat_id, message_id, media_id, kind, &bytes)
    })
    .await
    .unwrap_or_else(|error| Event::MediaFailed {
        chat_id,
        message_id,
        reason: format!("could not cache the media: {error}; nothing was saved"),
    })
}

/// The answer for a message already in the cache, if it is.
///
/// The media id is asked first, when the message carries one: a copy forwarded
/// from another chat is cached under the id, not under this message. Only then
/// the message's own key, which is all a message without an id has.
fn cached_media(
    cache: &Mutex<MediaCache>,
    chat_id: i64,
    message_id: i64,
    media_id: Option<i64>,
) -> Option<Event> {
    let path = {
        let mut cache = cache.lock().expect("the media cache lock is not poisoned");
        media_id
            .and_then(|id| cache.lookup_media(id))
            .or_else(|| cache.lookup(chat_id, message_id))?
    };
    Some(Event::MediaSaved {
        chat_id,
        message_id,
        path,
    })
}

/// Stores a finished download in the cache, and answers with the file it is in.
///
/// A store that fails is said on the status line: the reader asked to open the
/// media, and a file that was never written cannot be opened.
fn keep_download(
    cache: &Mutex<MediaCache>,
    chat_id: i64,
    message_id: i64,
    media_id: Option<i64>,
    kind: MediaKind,
    bytes: &[u8],
) -> Event {
    // The file is named for the message it was downloaded for, and for its media
    // id when it has one, so a later copy in another chat finds it.
    let mut keys = vec![Key::Message(chat_id, message_id)];
    keys.extend(media_id.map(Key::Media));
    let stored = MediaCache::store_shared(cache, &keys, kind, bytes);
    match stored {
        Some(path) => Event::MediaSaved {
            chat_id,
            message_id,
            path,
        },
        None => Event::MediaFailed {
            chat_id,
            message_id,
            reason: "could not cache the media; nothing was saved".to_owned(),
        },
    }
}

/// Settles a media download: a saved file is queued for the viewer and said on
/// the status line, and a failed one is said there and queues nothing.
fn apply_media(app: &mut App, state: &mut State, event: Event) {
    // Every settled download leaves the in-flight list, so a cancel that arrives
    // after it has finished finds no flag and does nothing.
    if let Event::MediaSaved {
        chat_id,
        message_id,
        ..
    }
    | Event::MediaFailed {
        chat_id,
        message_id,
        ..
    }
    | Event::MediaCancelled {
        chat_id,
        message_id,
    } = &event
    {
        let (chat_id, message_id) = (*chat_id, *message_id);
        state
            .media_cancel
            .retain(|cancel| cancel.chat_id != chat_id || cancel.message_id != message_id);
    }

    match event {
        // Progress is for the conversation view to draw; nothing settles on it.
        Event::MediaProgress {
            chat_id,
            message_id,
            downloaded,
            total,
        } => {
            app.conversation
                .downloads
                .progress(chat_id, message_id, downloaded, total);
            tracing::trace!(chat_id, message_id, downloaded, ?total, "media progress");
        }
        Event::MediaCancelled {
            chat_id,
            message_id,
        } => {
            app.conversation.downloads.forget(chat_id, message_id);
            tracing::debug!(chat_id, message_id, "media download cancelled");
        }
        Event::MediaSaved {
            chat_id,
            message_id,
            path,
        } => {
            app.conversation.downloads.forget(chat_id, message_id);
            tracing::debug!(chat_id, message_id, path = %path.display(), "media saved");
            app.flash(format!("media saved to {}", path.display()));
            state.media.push(path);
        }
        Event::MediaFailed {
            chat_id,
            message_id,
            reason,
        } => {
            tracing::warn!(chat_id, message_id, %reason, "a media download failed");
            app.conversation
                .downloads
                .fail(chat_id, message_id, reason.clone());
            app.flash(reason);
        }
        _ => {}
    }
}

fn apply_ready(
    app: &mut App,
    state: &mut State,
    client: Arc<ProtoClient>,
    chats: Vec<Chat>,
    account: Result<domain::account::Account, String>,
    session_store: tui::SessionStore,
) {
    apply_ready_to_screen(app, state, chats, account, session_store);
    state.client = Some(client);
    state.bringing_up = false;
    // The client is there, so the sign-in flow's `waiting` flag means
    // what it says: a request a client is carrying.
    app.set_client_available(true);
}

fn apply_logged_out_and_reconnect(app: &mut App, state: &mut State, result: Result<(), String>) {
    apply_logged_out(app, state, result);
    // The signed-out client is gone — and the fresh one below is not up
    // yet, so the screen is client-less until its `Ready` says otherwise.
    app.set_client_available(false);

    // A fresh client, because the one that just signed out cannot be
    // reused — see [`State::cfg`]. Its `Ready` carries
    // `Err(String::new())`, and `apply_ready_to_screen` is what opens
    // the phone field off the back of it.
    if let (Some(cfg), Some(tx)) = (state.cfg.clone(), state.tx.clone()) {
        // Counted as in flight from here rather than from the `Ready` it
        // will send, so a `:retry` typed while it is on its way is refused
        // rather than answered with a second client.
        state.bringing_up = true;
        spawn_bring_up(cfg, tx);
    }
}

fn apply_jumped(
    app: &mut App,
    state: &mut State,
    jump: Jump,
    mut cursor: HistoryCursor,
    result: Result<Vec<Message>, ProtoError>,
) {
    // However it ended, the jump is over — unless a later one has taken
    // its place, in which case that one is still on its way and must not
    // be asked for again.
    if state.history.jump == Some(jump) {
        state.history.jump = None;
    }

    // A page was fetched for one conversation, and the reader can open
    // another while it is in flight. The window refuses a page that is
    // not its own; the cursor has to be refused here too, or it would
    // start describing a conversation that is no longer on screen.
    if state.history.cursor.map(|open| open.peer_id()) != Some(cursor.peer_id()) {
        return;
    }

    match result {
        Ok(page) => {
            // Cached whether or not the window takes it: the page is what the
            // wire said about that stretch either way, and the merge decides
            // for itself whether it joins what is cached.
            state.remember_page(cursor.peer_id(), &page, PageKind::Around);
            // Only a page the window took is worth telling the cursor
            // about: a jump the reader abandoned leaves it describing
            // what is still on screen.
            if app.apply_jump(&page, jump.target_id) {
                cursor.reset_to(&page);
                settle(app, cursor);
                state.history.cursor = Some(cursor);
            }
        }

        // The reader is left where they were, with the reason on the
        // status line — and the wait is over, or the key would be wedged
        // by one bad request. No backoff is set, unlike a paging fetch:
        // this one was asked for by a keystroke rather than by the
        // driver, so nothing is going to ask again on its own, and
        // holding every direction would stall the paging the reader did
        // not interrupt.
        Err(error) => {
            app.clear_jump(jump.target_id);
            app.ui.status = format!("history: {error}");
        }
    }
}

/// Puts a fetched page on screen, or holds every direction for a while.
///
/// A page is for the conversation the cursor names, and the reader can open
/// another one while it is in flight: the window refuses a page that is not its
/// own, and the cursor is refused here too, or it would go on describing a
/// conversation that is no longer on screen.
fn apply_history(
    app: &mut App,
    state: &mut State,
    direction: FetchDirection,
    anchor: Option<i64>,
    mut cursor: HistoryCursor,
    result: Result<Vec<Message>, ProtoError>,
) {
    // However it ended, the direction is open again: a fetch that failed
    // must not close a conversation for good.
    app.end_fetch(direction);

    if state.history.cursor.map(|open| open.peer_id()) != Some(cursor.peer_id()) {
        return;
    }

    match result {
        Ok(page) => {
            if let Some(kind) = page_kind(direction, anchor, cursor, page.len()) {
                state.remember_page(cursor.peer_id(), &page, kind);
            }
            if direction == FetchDirection::Latest {
                cursor.reset_to(&page);
            }
            apply_page(app, direction, page);
            settle(app, cursor);
            state.history.cursor = Some(cursor);
        }

        Err(error) => {
            app.ui.status = format!("history: {error}");
            state.history.retry_at = Some(Instant::now() + backoff(&error));

            // A first page that failed leaves nothing to count from, so
            // the conversation is opened again once the backoff has
            // passed rather than staying empty for good.
            if direction == FetchDirection::Latest {
                state.history.cursor = None;
            }
        }
    }
}

/// What a bring-up that failed says, everywhere it has to be said.
///
/// The panel is told as well as the status line. A status is a flash: it is gone
/// within seconds, and a reader who opens the profile a minute later must still
/// be told why it is empty rather than shown a blank panel they cannot tell from
/// a broken one.
///
/// **This also ends the bring-up**, which is why it takes the state: a bring-up
/// that answered is not in flight, and a `:retry` from the `offline:` screen it
/// just wrote is a retry of nothing.
fn apply_offline(app: &mut App, state: &mut State, reason: &anyhow::Error) {
    let reason = format!("{reason:#}");
    app.set_account(Err(reason.clone()));
    // A persistent sentence, so the flash deadline goes with it: `expire_status`
    // is the clock a flash carries, and a failure must not revert to idle
    // while the reader is still looking at it.
    app.set_status(format!("offline: {reason}"));
    // The spent budget reads as the failed state, beside the sentence.
    app.set_connection(ConnectionState::Offline);
    // No client to carry anything, so an in-flight sign-in is not in flight: the
    // flag would only keep the panel saying "Checking…".
    app.set_client_available(false);
    state.bringing_up = false;
}

/// What the feed's end says to the screen, and what it leaves for the driver.
///
/// The request is recorded rather than carried out here: the driver owns the
/// configuration and the channel a bring-up needs, and it is the only place the
/// single-flight guard can be read.
///
/// The status is written straight rather than through `flash`, because a
/// reconnect is not a thing that passes on its own — it ends in an event, and
/// that event brings its own sentence. The wording is the manual `:retry` path's,
/// so the two cannot disagree about what "reconnecting" looks like.
fn apply_feed_ended(app: &mut App, state: &mut State) {
    state.reconnect_requested = true;
    // Whatever arrives before the new feed is up is never seen, so no cached
    // run is known current until its next newest page.
    state.cached.messages.feed_interrupted();
    // Persistent rather than a flash, for the same reason as the retry
    // sentence: a pending flash deadline must not take it down.
    app.set_status("reconnecting");
    // The feed dropped and the driver is rebuilding: reconnecting, in words and
    // in state.
    app.set_connection(ConnectionState::Reconnecting);

    // Nothing has a client until the rebuilt one's `Ready`: the flag says so to
    // every surface that would otherwise claim a request can be carried.
    app.set_client_available(false);
}

/// Puts a client that is up on screen: its conversations, its store, and its
/// account.
///
/// **The sign-in flow comes down here.** A sign-in that has just finished
/// leaves the overlay up, and [`App::login_complete`] is the one call that takes
/// it down — before the account, because the account is what the card draws and a
/// card drawn under an overlay is a card nobody sees. At a launch there is no
/// flow open, and the method only settles the focus and the status line: the
/// line itself is then the open conversation's draft, and it is kept.
///
/// **And the one case that opens the flow instead: credentials, no session.**
/// That is `Err("")` — not an account and not a reason — and a reader who lands
/// on an empty chat list is being asked to notice that they are signed out, which
/// `:signin` in a hint row is a poor way of saying. So the same entry point that
/// command uses is called here, and a non-empty reason never reaches it: a
/// session that authorizes with a profile this build could not read is a signed-in
/// account, and answering that with a sign-in form would be wrong twice.
///
/// Both decisions are read *before* `login_complete`, because that call takes any
/// flow down unconditionally — read after, `signin()` is always `None` and the
/// guard against clobbering a flow the reader already started would be a guard
/// against nothing. It is not reachable today (`Ready` is sent once, by bring-up,
/// before the reader can type anything) but a `Ready` that arrived mid-flow would
/// cost the reader a code they had already sent, which is exactly what the guard is
/// for. A `Ready` carrying an account still takes the overlay down as it always
/// has: that is a sign-in that *worked*.
///
/// Everything a `Ready` says *about the screen*, with the client itself left to
/// the caller: a client cannot be built without a runtime, and these effects can
/// be checked without one.
fn apply_ready_to_screen(
    app: &mut App,
    state: &mut State,
    chats: Vec<Chat>,
    account: Result<domain::account::Account, String>,
    session_store: tui::SessionStore,
) {
    // The client is up: connected, whatever the screen says underneath it.
    app.set_connection(ConnectionState::Connected);
    let no_session = matches!(&account, Err(reason) if reason.is_empty());
    // A conversation already on screen is the reader's place, and a `Ready` that
    // lands on top of one is the client being brought back up — or a launch the
    // cache drew before the wire answered — so the list is refreshed around that
    // place rather than the reader being moved to the top of it. A launch with
    // nothing open keeps the old landing. Either way the stale network anchors
    // go: the cursor, the in-flight jump and the retry gate all described the
    // list and the feed that are gone, and holding them makes `wanted` fall to a
    // paging direction whose page can never arrive (G5), because the preserved
    // window is not empty.
    // The cache's word that a run is current was a word about the old feed,
    // and this client brings a new one: forgotten for every peer, so each is
    // trusted again only once its next newest page lands — which, for the
    // conversation on show, the reset below asks for at once.
    state.cached.messages.feed_interrupted();
    // **No session is no place to keep.** The cache was read for a session that
    // is gone, and the form about to open takes any number — so it goes the way
    // a sign-out sends it, from memory and from disk, and the landing below
    // closes whatever conversation the cache had open rather than leaving its
    // rows behind the form. Whoever signs in next fetches their own.
    if no_session {
        state.forget_history();
    }
    let restored = if app.conversation.conversation.window.chat_id != 0 && !no_session {
        state.history.cursor = None;
        state.history.jump = None;
        state.history.retry_at = None;
        app.refresh_chats(chats)
    } else {
        open_first_chat(app, chats);
        true
    };

    // The `--chat` id, taken once the first list has landed: a known id moves
    // the reader there, before the sign-in flow below is entered, so a
    // signed-out launch lands on the requested chat. An unknown id keeps the
    // launch landing and is named persistently afterwards — unlike `:chat`,
    // which stays silent — because `login_complete` below puts the status
    // line back to its resting sentence and would overwrite it said here.
    let mut unknown_initial_chat: Option<i64> = None;
    if let Some(id) = app.take_initial_chat()
        && !app.select_chat_by_id(id)
    {
        if app.conversation.conversation.window.chat_id == 0 {
            app.select_chat(0);
        }
        unknown_initial_chat = Some(id);
    }

    app.set_session_store(session_store.clone());
    state.session_store = Some(session_store);
    let interrupted = app.signin().is_some();
    if !no_session || !interrupted {
        // `login_complete` empties the line because with a flow up the line is
        // the flow's field, holding a code or a password. With none up and a
        // buffer on it, it is the open conversation's draft — resumed into it
        // at the landing, or typed into a conversation the cache drew before
        // the wire answered — and the reader's words are not the flow's to
        // take. A `:` or `/` prompt mid-answer is still cleared, as it was.
        let draft = (!interrupted && app.input.line.purpose().is_buffer())
            .then(|| std::mem::take(&mut app.input.line));
        app.login_complete();
        if let Some(draft) = draft {
            app.input.line = draft;
        }
    }
    app.set_account(account);
    if no_session && !interrupted {
        app.begin_signin();
    }

    // Said last, because `login_complete` is what puts the status line back to
    // its resting sentence and would otherwise overwrite this.
    if !restored {
        "the open conversation is no longer in the chat list".clone_into(&mut app.ui.status);
    }
    if let Some(id) = unknown_initial_chat {
        app.ui.status = format!("no chat with id {id}");
    }
}

/// Folds a sign-in answer into the flow, whatever step it came from.
///
/// The two events differ only in which sentence they carry, so they are applied
/// by one function: a code and a password are the same shape of answer, and a
/// third copy of this match would be a third place for the two to disagree.
fn apply_sign_in(app: &mut App, state: &mut State, result: Result<SignIn, ProtoError>) {
    match result {
        Ok(SignIn::Account(account)) => {
            // The account first, then the flow down: `login_complete` is what
            // puts the status line back to its resting sentence, and the flash
            // naming the account is read out of the account state by the reader's
            // own next action — so this order is what lets both be true.
            app.set_account(Ok(account.clone()));
            app.login_complete();
        }

        Ok(SignIn::PasswordRequired(challenge)) => {
            // Read once, and owned from here: the hint borrows from the
            // challenge, and the challenge is kept for the request that spends it.
            let hint = challenge.hint().map(str::to_owned);
            state.login.challenge = Some(challenge);
            let phone = app.session.phone.clone();
            app.login_advanced(
                domain::session::SessionState::AwaitingPassword { phone },
                hint,
            );
        }

        Err(error) => {
            let used =
                password_attempts_used(error.refusal()).unwrap_or_else(|| used_attempts(app));
            apply_login_refusal(app, &error, used);
        }
    }
}

/// How many password attempts the reader has spent, if this refusal knows.
///
/// `None` for every refusal that is not about a password, which is most of them:
/// a wrong code says nothing about the two-factor count, and answering it with a
/// count would move a row Telegram did not move.
fn password_attempts_used(refusal: Option<&Refusal>) -> Option<u8> {
    match refusal {
        Some(Refusal::PasswordInvalid { attempts_left }) => {
            Some(PASSWORD_ATTEMPTS.saturating_sub(*attempts_left))
        }
        _ => None,
    }
}

/// The count the flow is already showing, which is the answer for a refusal that
/// says nothing about the count.
fn used_attempts(app: &App) -> u8 {
    app.signin()
        .and_then(tui::app::SignIn::flow)
        .map_or(0, |flow| flow.used)
}

/// Says what Telegram refused, in the reader's own words.
///
/// The sentence is `proto`'s — it is where the refusal vocabulary lives — and
/// only the count is decided here, because only this half can read what a
/// refusal was about.
fn apply_login_refusal(app: &mut App, error: &ProtoError, used: u8) {
    let sentence = match error.refusal() {
        Some(refusal) => proto::refusal_sentence(refusal),
        // Not a refusal at all — a request that never arrived. Its own words are
        // the only ones there are.
        None => error.to_string(),
    };
    app.login_refused(sentence, used);
}

/// Folds a send's answer into the conversation it was for.
///
/// The in-flight gate is freed before the conversation is checked: a result for
/// a conversation the reader has left still has to free the send key, or it
/// would stay wedged for every conversation they open after, with no visible
/// symptom to explain it.
fn apply_sent(app: &mut App, chat_id: i64, temp_id: i64, result: Result<Message, ProtoError>) {
    app.end_send(temp_id);

    if app.conversation.conversation.window.chat_id != chat_id {
        return;
    }

    match result {
        Ok(message) => {
            app.confirm_sent(temp_id, message);
        }
        Err(error) => {
            let reason = failure_reason(&error);
            tracing::debug!(chat_id, temp_id, %error, "a send failed");
            app.fail_send(temp_id, reason.clone());
            app.flash(reason);
        }
    }
}

/// Folds an edit's answer in.
///
/// An edit is gated per message rather than globally, so there is no key to free
/// here. Nothing is applied on success: the new text is the edit's
/// `MessageEdited` update and nothing else, because `grammers` discards the
/// updates the request answers with.
fn apply_edited(app: &mut App, chat_id: i64, message_id: i64, result: Result<(), ProtoError>) {
    if app.conversation.conversation.window.chat_id != chat_id {
        return;
    }

    match result {
        Ok(()) => tracing::debug!(chat_id, message_id, "an edit was accepted"),
        Err(error) => {
            tracing::debug!(chat_id, message_id, %error, "an edit failed");
            app.flash(format!("edit: {}", failure_reason(&error)));
        }
    }
}

/// Folds a pin's answer in. The list moves and the status says so when Telegram
/// agreed; the status says why not when it did not.
fn apply_pin_toggled(app: &mut App, chat_id: i64, pinned: bool, result: Result<(), ProtoError>) {
    match result {
        Ok(()) => {
            app.set_pinned(chat_id, pinned);
            app.flash(if pinned { "pinned" } else { "unpinned" });
        }
        Err(error) => {
            tracing::debug!(chat_id, %error, "a pin change failed");
            app.flash(format!("pin: {}", failure_reason(&error)));
        }
    }
}

/// Folds a deletion's answer in.
///
/// The removal itself is the feed's, and the update that does it names no
/// conversation — so `chat_id` decides only whether the reader is told about a
/// failure. A failure for a conversation they have left is not worth a line they
/// cannot act on.
///
/// A deletion of more than one request can be part-way through when it fails, and
/// "deleted 200 of 250" and "failed" are different events: one leaves the reader
/// with a conversation to finish cleaning up, the other leaves them with the same
/// one they started with. The count is reported because only one of them is
/// actionable.
fn apply_deleted(app: &mut App, chat_id: i64, message_ids: &[i64], result: Result<(), ProtoError>) {
    match result {
        Ok(()) => {
            tracing::debug!(
                chat_id,
                deleted = message_ids.len(),
                "a deletion was accepted"
            );
        }
        Err(error) => {
            let partial = deleted_before_failure(&error);
            tracing::debug!(chat_id, partial, %error, "a deletion failed");

            if app.conversation.conversation.window.chat_id == chat_id {
                app.flash(match partial {
                    Some(deleted) => format!(
                        "delete: {deleted} of {} went through, the rest did not",
                        message_ids.len()
                    ),
                    None => format!("delete: {}", failure_reason(&error)),
                });
            }
        }
    }
}

/// Sends a forward and says what came back.
///
/// The batch is sent as one call, which the proto wrapper splits; a refusal from
/// the wire is what the reply carries, not something checked up front.
async fn forward(
    client: &ProtoClient,
    chat_id: i64,
    message_ids: Vec<i64>,
    dest_chat_id: i64,
) -> Event {
    let requested = message_ids.len();
    let result = client
        .forward_messages(chat_id, dest_chat_id, &message_ids)
        .await;
    Event::Forwarded {
        chat_id,
        dest_chat_id,
        requested,
        result,
    }
}

/// Reports how a forward went on the status line.
///
/// The source and destination are named by title, because the reader thinks of
/// a conversation by its name and not its identifier.
fn apply_forwarded(
    app: &mut App,
    chat_id: i64,
    dest_chat_id: i64,
    requested: usize,
    result: Result<usize, ProtoError>,
) {
    let source = chat_title(app, chat_id);
    let text = match result {
        Ok(forwarded) => {
            let dest = chat_title(app, dest_chat_id);
            format!("Forwarded {forwarded} message(s) to {dest}")
        }
        Err(ProtoError::Framework(FrameworkError::PartialForward { forwarded, .. })) => {
            format!(
                "Forwarded {forwarded} of {requested} message(s) from {source}; the rest failed"
            )
        }
        Err(error) if is_forward_refusal(&error) => {
            format!("{source} does not allow forwarding")
        }
        Err(error) => format!("forward: {}", failure_reason(&error)),
    };
    app.flash(text);
}

/// Whether the wire refused the forward because the source is content-protected.
fn is_forward_refusal(error: &ProtoError) -> bool {
    matches!(
        error,
        ProtoError::Framework(FrameworkError::Request(RequestError::Rpc { name, .. }))
            if name == "CHAT_FORWARDS_RESTRICTED"
    )
}

/// The title of a conversation on the chat list, or its identifier if the list
/// does not hold it.
fn chat_title(app: &App, chat_id: i64) -> String {
    app.chats()
        .iter()
        .find(|chat| chat.id == chat_id)
        .map_or_else(|| format!("chat {chat_id}"), |chat| chat.title.clone())
}

/// How many identifiers a failed deletion had already removed, if it had got
/// that far.
///
/// `None` for a failure before anything landed, which is the ordinary case and
/// which the caller reports as itself rather than as a partial deletion.
fn deleted_before_failure(error: &ProtoError) -> Option<usize> {
    match error {
        ProtoError::Framework(FrameworkError::PartialDelete { deleted, .. }) => Some(*deleted),
        _ => None,
    }
}

/// Folds a search's answer in, or says why there is not one.
///
/// A failure never empties the local list: the reader asked a question and the
/// window answered it, and a failed request says nothing about that answer. The
/// reason travels with the search so the label can say the list is local.
///
/// It deliberately does **not** touch [`History::retry_at`]. That gate holds
/// history *paging*, and a search that was throttled must not stall the
/// conversation the reader is standing in — the wrong wiring here would be
/// invisible until a reader noticed they could not scroll.
fn apply_searched(
    app: &mut App,
    chat_id: i64,
    query: &str,
    result: Result<SearchResults, ProtoError>,
) {
    match result {
        Ok(results) => {
            tracing::debug!(
                chat_id,
                query,
                found = results.ids.len(),
                total = results.total,
                "a search landed"
            );
            // Whether it landed is not acted on: a result for a conversation
            // the reader has left, or for a query they have replaced, is
            // refused by the screen and owes no redraw of its own.
            app.apply_searched(chat_id, query, results.ids, results.total);
        }
        Err(error) => {
            tracing::debug!(chat_id, query, %error, "a search failed");
            app.search_failed(query, failure_reason(&error));
        }
    }
}

/// Opens the person a resolution named, when it is still the answer wanted.
///
/// A `None` is not an answer: the caller's fallback — the name search — will
/// supply the list, so nothing is shown here. A resolution for a query the
/// reader has replaced is dropped, exactly as a stale list would be, because the
/// chat it would open is one nobody is looking for any more.
fn apply_user_resolved(app: &mut App, query: &str, user: Option<UserCandidate>) {
    let Some(user) = user else {
        return;
    };
    if app.user_search().query() != Some(query) {
        return;
    }

    app.open_user(&user);
}

/// Shows the people a name search listed, opening a lone one directly.
///
/// A single candidate is the reader's answer rather than a list (Q4); zero or
/// more than one is a list to choose from, and [`App::apply_users`] refuses it
/// if the query has been replaced.
fn apply_users_listed(app: &mut App, query: &str, users: Vec<UserCandidate>) {
    if app.user_search().query() != Some(query) {
        return;
    }

    // One candidate is the answer; the rest are a list. The borrow of `users`
    // ends before `open_user`, which needs the application mutably.
    if let Some(only) = sole_candidate(&users) {
        app.open_user(only);
    } else {
        app.apply_users(query, users);
    }
}

/// Records a failed lookup on the search it belongs to, and says why.
///
/// A reason for a query the reader has replaced is not theirs to be told: only
/// the search the failure belongs to is flashed, and [`App::fail_users`] is the
/// one that decides that.
fn apply_user_lookup_failed(app: &mut App, query: &str, reason: String) {
    if app.fail_users(query, reason.clone()) {
        app.flash(reason);
    }
}

/// Draws what the history file held, before the client is up: the cached chat
/// list, and the conversation a launch lands in, seeded from the cache.
///
/// The launch landing run early, so the first frame is the reader's
/// conversations rather than an empty screen saying `connecting…`. The
/// sentence stays — nothing has connected — and nothing is asked for: the
/// [`Event::Ready`] that follows finds a conversation open and takes
/// [`apply_ready_to_screen`]'s refresh path, which keeps the reader where they
/// are and clears the cursor, so the next pass asks for the newest page.
///
/// **`--chat` is spent here only when the cache can answer it.** An id the
/// cached list holds is opened now and consumed. One it does not hold is put
/// back for the `Ready`, whose fresh list may hold it — and which names it when
/// it does not — and the cached head is opened in the meantime.
///
/// A cold cache draws nothing, and the launch is what it always was.
pub(crate) fn open_from_cache(app: &mut App, state: &State) {
    let chats = state.cached.messages.chats();
    if chats.is_empty() {
        return;
    }
    app.set_chats(chats);
    match app.take_initial_chat() {
        Some(id) if app.select_chat_by_id(id) => {}
        Some(id) => {
            app.set_initial_chat(id);
            app.select_chat(0);
        }
        None => app.select_chat(0),
    }
    seed_opened(app, state);
}

/// Puts a fetched chat list on screen, and the reader into it.
///
/// The list is newest first, so the first entry is the conversation that last
/// had something to say — which is the one opening the client should land in.
fn open_first_chat(app: &mut App, chats: Vec<Chat>) {
    let count = chats.len();
    app.set_chats(chats);
    app.ui.status = format!("{count} conversation(s)");

    if count > 0 {
        app.select_chat(0);
    }
}

/// Puts a fetched page where it belongs in the window.
///
/// Whether the window took it is not acted on: the loop redraws on every pass,
/// and a page that added nothing — the overlap with the cursor is one message
/// wide — is an ordinary outcome rather than a failure.
fn apply_page(app: &mut App, direction: FetchDirection, page: Vec<Message>) {
    match direction {
        FetchDirection::Latest => {
            app.apply_latest(page);
        }
        FetchDirection::Older => {
            app.apply_older(page);
        }
        FetchDirection::Newer => {
            app.apply_newer(page);
        }
    }
}

/// Which stretch of the conversation a page that came back was fetched from,
/// in the cache's terms.
///
/// `None` for a paging fetch with no anchor, which is one that short-circuited
/// to an empty page before anything was loaded — nothing to fold in. Whether a
/// page was short is the cursor's answer for the two paging directions, which
/// the fetch has already noted, and the page's own length for the newest page,
/// which the cursor is not told about: the same rule against the same `PAGE`.
fn page_kind(
    direction: FetchDirection,
    anchor: Option<i64>,
    cursor: HistoryCursor,
    len: usize,
) -> Option<PageKind> {
    match direction {
        FetchDirection::Latest => Some(PageKind::Latest { whole: len < PAGE }),
        FetchDirection::Older => anchor.map(|before| PageKind::Older {
            before,
            reached_start: cursor.exhausted_older(),
        }),
        FetchDirection::Newer => anchor.map(|after| PageKind::Newer {
            after,
            reached_end: cursor.exhausted_newer(),
        }),
    }
}

/// Keeps the window's idea of what is left in step with the cursor's.
///
/// Only the cursor can tell a short page from a full one, and the trigger reads
/// the window, so the answer has to be carried across.
fn settle(app: &mut App, cursor: HistoryCursor) {
    if cursor.exhausted_older() {
        app.exhaust(FetchDirection::Older);
    }
    if cursor.exhausted_newer() {
        app.exhaust(FetchDirection::Newer);
    }
}

/// How long to leave a failed direction alone.
///
/// Telegram says how long to wait when it refuses a request for being too
/// frequent, and its answer is the only useful one: a fixed backoff would be
/// either too short to satisfy it or too long for a failure that will pass.
fn backoff(error: &ProtoError) -> Duration {
    match error {
        ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            name, value, ..
        })) if name.contains("FLOOD") => {
            value.map_or(RETRY, |seconds| Duration::from_secs(u64::from(seconds)))
        }
        _ => RETRY,
    }
}

/// What a chat-list retry in progress says on the status line.
///
/// Persistent, not a flash: the wait is the second-longest thing that happens
/// during a launch, and a reader who looks away and back must not find an empty
/// status line and no client — which is the `offline:` screen it is not yet. Both
/// the wait and the count are here because a wait with no visible end of it is
/// the thing this sentence exists to prevent.
fn apply_chat_list_retrying(app: &mut App, retry: &ChatListRetry) {
    // A wait shorter than a second still has to be announced as one: "retrying in
    // 0s" reads as no retry at all.
    let seconds = retry.delay.as_secs().max(1);
    app.set_status(format!(
        "fetching the chat list failed ({:#}); retrying in {}s (attempt {}/{})",
        retry.reason, seconds, retry.attempt, retry.attempts
    ));
    // A wait with a visible end of it is still a wait: reconnecting, beside the
    // sentence that says how long.
    app.set_connection(ConnectionState::Reconnecting);
}

/// What a feed retry in progress says on the status line.
///
/// Persistent, not a flash, for the same reason as the chat-list sentence: a
/// reader who looks away and back must find the wait with its reason and its
/// end, not an empty status line — and never above a confirmation, which is
/// what the rank in `status_text` guarantees. Both the wait and the count are
/// here because a wait with no visible end of it is the thing this sentence
/// exists to prevent.
fn apply_feed_retrying(app: &mut App, retry: &FeedRetry) {
    // A wait shorter than a second still has to be announced as one: "retrying in
    // 0s" reads as no retry at all.
    let seconds = retry.delay.as_secs().max(1);
    app.set_status(format!(
        "the update feed failed ({:#}); retrying in {}s (attempt {}/{})",
        retry.reason, seconds, retry.attempt, retry.attempts
    ));
    // The same wait the launch gets, and the same state beside its sentence.
    app.set_connection(ConnectionState::Reconnecting);
}

/// Whether the launch chat-list fetch may be asked again, and how long to wait.
///
/// `None` once [`CHAT_LIST_ATTEMPTS`] attempts have been spent, which is the one
/// decision this makes and the only reason the bring-up loop is a loop rather
/// than a recursion: past the bound the refusal is a failure of bring-up, and it
/// goes back up as one. The wait is [`backoff`]'s — Telegram's own when it asked
/// for one, the fixed `RETRY` otherwise — because a chat list refused for being
/// asked too often has exactly the same answer as a page that was.
fn chat_list_retry(attempts_used: u8, error: &ProtoError) -> Option<Duration> {
    (attempts_used < CHAT_LIST_ATTEMPTS).then(|| backoff(error))
}

/// Whether a failed update-feed read may be tried again, and how long to wait.
///
/// `None` once [`CHAT_LIST_ATTEMPTS`] errors have been seen, mirroring
/// [`chat_list_retry`]: past the bound the failure goes back up rather than
/// looping. The wait is [`backoff`]'s — Telegram's own when it asked for one,
/// the fixed `RETRY` otherwise.
fn feed_error_retry(errors_seen: u8, error: &ProtoError) -> Option<Duration> {
    (errors_seen < CHAT_LIST_ATTEMPTS).then(|| backoff(error))
}

/// Whether a failure is Telegram asking the client to wait before trying again.
fn is_flood_wait(error: &ProtoError) -> bool {
    matches!(
        error,
        ProtoError::Framework(FrameworkError::Request(RequestError::Rpc { name, .. }))
            if name.contains("FLOOD")
    )
}

/// A short description of a failed operation, for a row and the status line.
///
/// A flood wait is labelled with the wait it asks for, because that is the one
/// part of it the reader can act on; everything else is the error's own
/// description. The wait is reported rather than slept through: holding the
/// in-flight gate for up to a minute would freeze the key with no way out — the
/// gate is freed as soon as the answer arrives, so a re-Enter is possible.
fn failure_reason(error: &ProtoError) -> String {
    if is_flood_wait(error) {
        format!("flood wait, retry in {}s", backoff(error).as_secs())
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::chat::ChatKind;
    use domain::message::MessageStatus;

    use super::*;
    use crate::history_store::HistoryCache;
    use tui::app::AccountState;
    use tui::app::CHAT_SWITCH_DELAY;
    use tui::app::ConnectionState;
    use tui::app::Focus;
    use tui::app::JumpKind;
    use tui::app::REVALIDATING_LABEL;

    /// The conversation the sample messages belong to.
    const CHAT: i64 = 7;

    fn chat(id: i64) -> Chat {
        Chat {
            id,
            title: format!("chat-{id}"),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted: false,
        }
    }

    fn messages(chat_id: i64, ids: impl IntoIterator<Item = i64>) -> Vec<Message> {
        ids.into_iter()
            .map(|id| Message {
                id,
                chat_id,
                text: Cow::Borrowed("text"),
                timestamp: id,
                status: MessageStatus::Received,
                is_outgoing: false,
                reply_to: None,
                media: None,
                media_id: None,
            })
            .collect()
    }

    /// The conversation the forwards are sent to in these tests.
    const DEST: i64 = 9;

    /// An application with the source conversation open and a destination on the
    /// chat list, so both titles can be named.
    fn forward_app() -> App {
        let mut app = app_with_a_conversation(CHAT, 3);
        app.set_chats(vec![chat(CHAT), chat(DEST)]);
        app
    }

    /// An application with one conversation open and `count` messages loaded.
    fn app_with_a_conversation(chat_id: i64, count: i64) -> App {
        let mut app = App::new();
        app.set_chats(vec![chat(chat_id)]);
        app.select_chat(0);
        app.apply_latest(messages(chat_id, 1..=count));
        app
    }

    /// An application whose open conversation has unread messages in front of
    /// what is loaded: the list says the conversation runs to 20, and the window
    /// stops at 8.
    ///
    /// A second conversation is in the list so that opening another one is
    /// something these tests can do — which is how a page in flight comes to be
    /// one nobody is waiting for.
    fn app_with_unread_out_of_reach(unread: u32) -> App {
        let mut app = App::new();
        let mut conversation = chat(CHAT);
        conversation.unread_count = unread;
        conversation.last_message_id = Some(20);
        app.set_chats(vec![conversation, chat(CHAT + 1)]);
        app.select_chat(0);
        app.apply_latest(messages(CHAT, 1..=8));
        app
    }

    /// Asks to be taken to the unread messages the way a reader does: `gg`.
    fn ask_to_jump(app: &mut App) {
        for _ in 0..2 {
            app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        }
    }

    /// Moves the reader up, which is what disengages following.
    fn scroll_up(app: &mut App, times: usize) {
        for _ in 0..times {
            app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        }
    }

    /// A cursor for an open conversation, as the driver would have left it.
    fn opened(peer_id: i64) -> History {
        History {
            cursor: Some(HistoryCursor::new(peer_id)),
            jump: None,
            retry_at: None,
        }
    }

    // ---- what to ask for ------------------------------------------------

    #[test]
    fn nothing_is_asked_for_while_no_conversation_is_open() {
        let app = App::new();

        assert_eq!(
            wanted(&app, History::default(), Instant::now()),
            Wanted::Nothing
        );
    }

    #[test]
    fn opening_a_conversation_asks_for_its_newest_page() {
        let mut app = App::new();
        app.set_chats(vec![chat(CHAT)]);
        app.select_chat(0);

        assert_eq!(
            wanted(&app, History::default(), Instant::now()),
            Wanted::Latest(CHAT),
            "there is nothing loaded to page from either end of"
        );
    }

    #[test]
    fn a_view_pinned_to_the_newest_message_asks_for_nothing() {
        let app = app_with_a_conversation(CHAT, 40);

        assert_eq!(
            wanted(&app, opened(CHAT), Instant::now()),
            Wanted::Nothing,
            "the reader is at the end of what is loaded, and nothing is arriving \
             faster than the feed delivers it"
        );
    }

    #[test]
    fn the_top_of_what_is_loaded_asks_for_what_came_before() {
        let mut app = app_with_a_conversation(CHAT, 40);
        scroll_up(&mut app, 39);

        assert_eq!(
            wanted(&app, opened(CHAT), Instant::now()),
            Wanted::Page(FetchDirection::Older, HistoryCursor::new(CHAT))
        );
    }

    #[test]
    fn a_reader_who_scrolled_away_asks_for_what_came_after() {
        let mut app = app_with_a_conversation(CHAT, 30);
        scroll_up(&mut app, 5);

        assert_eq!(
            wanted(&app, opened(CHAT), Instant::now()),
            Wanted::Page(FetchDirection::Newer, HistoryCursor::new(CHAT))
        );
    }

    /// The cursor describes the conversation on show. One for another
    /// conversation is not a cursor at all as far as this window is concerned,
    /// and what it needs is the newest page.
    #[test]
    fn a_cursor_for_another_conversation_asks_for_a_first_page() {
        let app = app_with_a_conversation(CHAT, 40);

        assert_eq!(
            wanted(&app, opened(CHAT + 1), Instant::now()),
            Wanted::Latest(CHAT)
        );
    }

    /// A jump the reader asked for is answered before paging — and asked for
    /// once, however long the key is held.
    #[test]
    fn a_jump_the_reader_asked_for_outranks_paging() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);

        let jump = Jump {
            peer_id: CHAT,
            target_id: 19,
            kind: JumpKind::Unread,
        };
        assert_eq!(app.pending_jump(), Some(jump), "counting back two from 20");
        assert_eq!(
            wanted(&app, opened(CHAT), Instant::now()),
            Wanted::Jump(jump),
            "and the page it lands in has not been fetched, so paging can wait"
        );

        let on_its_way = History {
            jump: Some(jump),
            ..opened(CHAT)
        };
        assert_eq!(
            wanted(&app, on_its_way, Instant::now()),
            Wanted::Nothing,
            "one jump at a time"
        );
    }

    /// A jump to the message a reply quotes takes the same road as `gg`'s, and
    /// the page that comes back replaces the window as it does for any other
    /// jump: one fetch path, asked for by two keys.
    #[test]
    fn a_reply_jump_reaches_the_network_and_replaces_the_window() {
        let mut app = app_with_unread_out_of_reach(2);
        let quotes: Vec<Message> = messages(CHAT, 1..=8)
            .into_iter()
            .map(|mut message| {
                message.reply_to = Some(19);
                message
            })
            .collect();
        app.apply_latest(quotes);
        for key in [KeyCode::Char('g'), KeyCode::Char('d')] {
            app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        }

        let jump = Jump {
            peer_id: CHAT,
            target_id: 19,
            kind: JumpKind::Reply,
        };
        assert_eq!(app.pending_jump(), Some(jump));
        assert_eq!(
            wanted(&app, opened(CHAT), Instant::now()),
            Wanted::Jump(jump),
            "a reply the client does not hold is a fetch like any other"
        );

        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 16..=20)),
            },
        );

        assert_eq!(
            app.conversation.conversation.window.len(),
            5,
            "the window was replaced"
        );
        assert_eq!(
            app.conversation
                .conversation
                .window
                .get(app.conversation.vim.cursor())
                .map(|message| message.id),
            Some(19),
            "and the reader is on the message that was quoted"
        );
    }

    /// `Ctrl-o` after a jump that replaced the window is a fetch like any
    /// other: the mark it walks to is a message the window no longer holds, so
    /// the page has to come back for it.
    #[test]
    fn a_back_jump_reaches_the_network_and_lands() {
        let mut app = app_with_unread_out_of_reach(2);
        let quotes: Vec<Message> = messages(CHAT, 1..=8)
            .into_iter()
            .map(|mut message| {
                message.reply_to = Some(19);
                message
            })
            .collect();
        app.apply_latest(quotes);
        for key in [KeyCode::Char('g'), KeyCode::Char('d')] {
            app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        }
        let replied = app
            .pending_jump()
            .expect("the reader asked to be taken to the message it quoted");
        apply(
            &mut app,
            &mut State {
                history: opened(CHAT),
                ..State::default()
            },
            Event::Jumped {
                jump: replied,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 16..=20)),
            },
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

        let back = Jump {
            peer_id: CHAT,
            target_id: 8,
            kind: JumpKind::Back,
        };
        assert_eq!(
            app.pending_jump(),
            Some(back),
            "the message the reader left is not in the window that replaced it"
        );
        assert_eq!(
            wanted(&app, opened(CHAT), Instant::now()),
            Wanted::Jump(back),
            "so the page around it is fetched like any other jump's"
        );

        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump: back,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 1..=8)),
            },
        );

        assert_eq!(
            app.conversation
                .conversation
                .window
                .get(app.conversation.vim.cursor())
                .map(|message| message.id),
            Some(8),
            "and the reader is back on the message they left"
        );
        assert_eq!(app.pending_jump(), None);
    }

    /// A jump's answer is reported to the cursor the conversation is described
    /// by, so it waits behind the first page rather than beside it.
    #[test]
    fn a_jump_waits_until_the_conversation_has_a_cursor() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);

        assert_eq!(
            wanted(&app, History::default(), Instant::now()),
            Wanted::Latest(CHAT)
        );
    }

    #[test]
    fn a_fetch_that_failed_holds_every_direction_until_its_backoff_passes() {
        let mut app = app_with_a_conversation(CHAT, 40);
        scroll_up(&mut app, 39);
        let now = Instant::now();

        let holding = History {
            cursor: Some(HistoryCursor::new(CHAT)),
            jump: None,
            retry_at: Some(now + RETRY),
        };
        assert_eq!(wanted(&app, holding, now), Wanted::Nothing);

        assert_eq!(
            wanted(&app, holding, now + RETRY),
            Wanted::Page(FetchDirection::Older, HistoryCursor::new(CHAT)),
            "and asks again once it has"
        );
    }

    // ---- marking a conversation read on open ----------------------------

    /// An application with the conversation open, its list entry showing
    /// `unread` messages up to `last`, and a few messages in the window.
    fn listed(unread: u32, last: Option<i64>) -> App {
        let mut app = App::new();
        let mut conversation = chat(CHAT);
        conversation.unread_count = unread;
        conversation.last_message_id = last;
        app.set_chats(vec![conversation]);
        app.select_chat(0);
        app.apply_latest(messages(CHAT, 1..=3));
        app
    }

    #[test]
    fn an_open_conversation_with_unread_messages_is_read_up_to_its_newest() {
        assert_eq!(
            read_target(&listed(2, Some(20)), &State::default()),
            Some((CHAT, 20))
        );
    }

    #[test]
    fn an_already_read_conversation_sends_nothing() {
        assert_eq!(read_target(&listed(0, Some(20)), &State::default()), None);
    }

    #[test]
    fn nothing_is_sent_without_a_real_message_to_read_up_to() {
        for last in [None, Some(0), Some(-1)] {
            assert_eq!(
                read_target(&listed(2, last), &State::default()),
                None,
                "last_message_id {last:?}"
            );
        }
    }

    #[test]
    fn nothing_is_sent_while_no_conversation_is_open() {
        assert_eq!(read_target(&App::new(), &State::default()), None);
    }

    #[test]
    fn a_ceiling_already_accepted_is_not_sent_again() {
        let app = listed(2, Some(20));
        let acked = State {
            read_acked: Some((CHAT, 20)),
            ..State::default()
        };
        assert_eq!(read_target(&app, &acked), None);

        let older = State {
            read_acked: Some((CHAT, 19)),
            ..State::default()
        };
        assert_eq!(read_target(&app, &older), Some((CHAT, 20)));
    }

    #[test]
    fn an_accepted_marker_clears_the_count_and_is_recorded() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::ReadMarked {
                chat_id: CHAT,
                max_id: 20,
            },
        );

        assert_eq!(app.chats()[0].unread_count, 0);
        assert_eq!(state.read_acked, Some((CHAT, 20)));
        assert_eq!(read_target(&app, &state), None);
    }

    #[test]
    fn a_refused_marker_leaves_the_count_for_the_next_open() {
        // A refusal sends no event, so nothing is applied: the count and the
        // record are what they were, and the next open asks again.
        let app = listed(2, Some(20));
        let state = State::default();

        assert_eq!(app.chats()[0].unread_count, 2);
        assert_eq!(state.read_acked, None);
        assert_eq!(read_target(&app, &state), Some((CHAT, 20)));
    }

    #[test]
    fn an_open_with_no_client_sends_nothing_and_leaves_the_cursor_for_the_client() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        drive(&mut app, &mut state, &tx);

        assert!(rx.try_recv().is_err(), "nothing is sent with no client");
        assert_eq!(app.chats()[0].unread_count, 2);
        assert!(state.history.cursor.is_none());
    }

    #[test]
    fn an_arrival_in_the_open_conversation_asks_for_its_newest_again() {
        let mut app = listed(0, Some(3));
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 4..=4).remove(0))),
        );

        assert_eq!(state.read_owed, Some(CHAT));
        assert_eq!(read_target(&app, &state), Some((CHAT, 4)));
    }

    #[test]
    fn a_second_identical_arrival_is_deduped_once_the_first_is_accepted() {
        let mut app = listed(0, Some(3));
        let mut state = State::default();
        let arrival = || Event::Update(UpdateEvent::NewMessage(messages(CHAT, 4..=4).remove(0)));

        apply(&mut app, &mut state, arrival());
        apply(
            &mut app,
            &mut state,
            Event::ReadMarked {
                chat_id: CHAT,
                max_id: 4,
            },
        );
        apply(&mut app, &mut state, arrival());

        assert_eq!(state.read_owed, Some(CHAT));
        assert_eq!(read_target(&app, &state), None);
    }

    #[test]
    fn an_arrival_in_another_conversation_leaves_the_open_one_alone() {
        let mut app = listed(0, Some(3));
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT + 1, 4..=4).remove(0))),
        );

        assert_eq!(state.read_owed, None);
        assert_eq!(read_target(&app, &state), None);
    }

    /// Shows `CHAT`'s card over the open conversation, as the chat list does when
    /// the highlight stops on the chat that is already open.
    fn card_over_the_open_chat(app: &mut App) {
        app.ui.pane = tui::app::Pane::Profile(tui::app::ProfileId::User(CHAT));
    }

    #[test]
    fn a_card_only_show_marks_nothing_and_keeps_the_count_until_confirm() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();
        card_over_the_open_chat(&mut app);

        assert_eq!(
            read_target(&app, &state),
            None,
            "the card alone asks for nothing"
        );

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 21..=21).remove(0))),
        );

        assert_eq!(
            state.read_owed,
            Some(CHAT),
            "the arrival is kept, not spent on a refused marker"
        );
        assert_eq!(read_target(&app, &state), None);
        assert_eq!(
            app.chats()[0].unread_count,
            3,
            "the arrival is counted, not read"
        );

        // Backing out of the card puts the conversation back on show, and the
        // arrival it was owed is then the marker's to ask for.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(read_target(&app, &state), Some((CHAT, 21)));
    }

    #[test]
    fn confirming_the_card_marks_the_conversation_as_an_open_does() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();
        card_over_the_open_chat(&mut app);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.ui.pane, tui::app::Pane::Conversation);
        assert_eq!(app.conversation.conversation.window.chat_id, CHAT);
        assert_eq!(read_target(&app, &state), Some((CHAT, 20)));

        apply(
            &mut app,
            &mut state,
            Event::ReadMarked {
                chat_id: CHAT,
                max_id: 20,
            },
        );
        assert_eq!(app.chats()[0].unread_count, 0);
    }

    #[test]
    fn confirming_after_an_arrival_marks_once_with_the_newest_message() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();
        card_over_the_open_chat(&mut app);
        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 21..=21).remove(0))),
        );

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            read_target(&app, &state),
            Some((CHAT, 21)),
            "the confirm asks for the newest message, once"
        );

        apply(
            &mut app,
            &mut state,
            Event::ReadMarked {
                chat_id: CHAT,
                max_id: 21,
            },
        );

        assert_eq!(app.chats()[0].unread_count, 0);
        assert_eq!(
            read_target(&app, &state),
            None,
            "a second drive after the single accepted marker sends nothing"
        );
    }

    #[test]
    fn an_arrival_with_no_client_sends_nothing_and_is_not_carried_forward() {
        let mut app = listed(0, Some(3));
        let mut state = State::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 4..=4).remove(0))),
        );
        drive(&mut app, &mut state, &tx);

        assert!(rx.try_recv().is_err(), "nothing is sent with no client");
        assert_eq!(state.read_owed, None, "the flag is taken on the pass");
    }

    #[test]
    fn a_pass_under_the_open_chats_own_card_keeps_the_arrival_owed() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        card_over_the_open_chat(&mut app);
        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 21..=21).remove(0))),
        );

        drive(&mut app, &mut state, &tx);

        assert!(rx.try_recv().is_err(), "the card sends no marker");
        assert_eq!(
            state.read_owed,
            Some(CHAT),
            "a pass under the card does not spend the arrival"
        );
    }

    #[test]
    fn a_latest_under_a_card_owes_its_read_marker_to_the_pass_after_back() {
        let mut app = listed(2, Some(20));
        let mut state = State::default();
        card_over_the_open_chat(&mut app);

        assert_eq!(latest_read(&app, &mut state), None, "the card refuses it");
        assert_eq!(state.read_owed, Some(CHAT), "the marker is owed");

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // The pass after backing out: the arrival gating drive uses.
        let arrived = !card_covers_conversation(&app) && state.read_owed.take().is_some();
        assert!(arrived);
        assert_eq!(read_target(&app, &state), Some((CHAT, 20)));
    }

    #[test]
    fn a_latest_with_no_card_asks_now_and_owes_nothing() {
        let app = listed(2, Some(20));
        let mut state = State::default();

        assert_eq!(latest_read(&app, &mut state), Some((CHAT, 20)));
        assert_eq!(state.read_owed, None);
    }

    // ---- what a failure costs -------------------------------------------

    #[test]
    fn telegram_s_own_answer_is_what_a_flood_wait_waits_for() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        }));

        assert_eq!(backoff(&error), Duration::from_secs(31));
    }

    /// A flood wait with no number is still a flood wait: the fixed backoff is
    /// the conservative answer rather than the immediate retry.
    #[test]
    fn a_flood_wait_without_a_number_still_waits() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: None,
        }));

        assert_eq!(backoff(&error), RETRY);
    }

    #[test]
    fn anything_else_waits_the_fixed_time() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Network(
            "connection reset".to_owned(),
        )));

        assert_eq!(backoff(&error), RETRY);
    }

    // ---- what a chat-list failure costs ----------------------------------

    #[test]
    fn a_chat_list_refusal_waits_what_telegram_asked_for() {
        let flood = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        }));

        assert_eq!(chat_list_retry(0, &flood), Some(Duration::from_secs(31)));
        assert_eq!(
            chat_list_retry(CHAT_LIST_ATTEMPTS - 1, &flood),
            Some(Duration::from_secs(31)),
            "the last attempt still gets its wait"
        );
    }

    #[test]
    fn a_chat_list_refusal_of_another_kind_waits_the_fixed_time() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Network(
            "connection reset".to_owned(),
        )));

        assert_eq!(chat_list_retry(1, &error), Some(RETRY));
    }

    /// The bound is what stops the retry; the delay is never consulted again
    /// once it is reached, however willing the error is to wait.
    #[test]
    fn a_chat_list_that_will_not_answer_stops_at_the_bound() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        }));

        assert_eq!(chat_list_retry(CHAT_LIST_ATTEMPTS, &error), None);
        assert_eq!(chat_list_retry(CHAT_LIST_ATTEMPTS + 1, &error), None);
    }

    // ---- what a feed failure costs -------------------------------------

    #[test]
    fn a_feed_refusal_waits_what_telegram_asked_for() {
        let flood = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        }));

        assert_eq!(feed_error_retry(0, &flood), Some(Duration::from_secs(31)));
        assert_eq!(
            feed_error_retry(CHAT_LIST_ATTEMPTS - 1, &flood),
            Some(Duration::from_secs(31)),
            "the last error still gets its wait"
        );
    }

    #[test]
    fn a_feed_refusal_of_another_kind_waits_the_fixed_time() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Network(
            "connection reset".to_owned(),
        )));

        assert_eq!(feed_error_retry(1, &error), Some(RETRY));
    }

    /// The bound is what stops the retry; the delay is never consulted again
    /// once it is reached, however willing the error is to wait.
    #[test]
    fn a_feed_that_will_not_answer_stops_at_the_bound() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        }));

        assert_eq!(feed_error_retry(CHAT_LIST_ATTEMPTS, &error), None);
        assert_eq!(feed_error_retry(CHAT_LIST_ATTEMPTS + 1, &error), None);
    }

    #[test]
    fn a_chat_list_being_retried_says_so_on_the_status_line() {
        let mut app = App::new();
        let mut state = State::default();
        app.flash("something went wrong");

        apply(
            &mut app,
            &mut state,
            Event::ChatListRetrying(ChatListRetry {
                reason: ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
                    code: 420,
                    name: "FLOOD_WAIT".to_owned(),
                    value: Some(31),
                })),
                delay: Duration::from_secs(31),
                attempt: 1,
                attempts: CHAT_LIST_ATTEMPTS,
            }),
        );

        assert!(
            app.ui.status.contains("31s") && app.ui.status.contains("1/3"),
            "got {:?}",
            app.ui.status
        );
        assert!(
            !app.expire_status(Instant::now() + Duration::from_secs(10)),
            "the sentence does not go away on its own: {:?}",
            app.ui.status
        );
    }

    /// The feed's wait names the same three things the launch's does: the
    /// reason, the wait, and the count. Persistent too — a flash deadline left
    /// over from something transient must not take it down on the next tick.
    #[test]
    fn a_feed_retry_says_so_on_the_status_line() {
        let mut app = App::new();
        let mut state = State::default();
        app.flash("something went wrong");

        apply(
            &mut app,
            &mut state,
            Event::FeedRetrying(FeedRetry {
                reason: ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
                    code: 420,
                    name: "FLOOD_WAIT".to_owned(),
                    value: Some(31),
                })),
                delay: Duration::from_secs(31),
                attempt: 1,
                attempts: CHAT_LIST_ATTEMPTS,
            }),
        );

        assert!(
            app.ui.status.contains("FLOOD_WAIT")
                && app.ui.status.contains("31s")
                && app.ui.status.contains("1/3"),
            "got {:?}",
            app.ui.status
        );
        assert!(
            !app.expire_status(Instant::now() + Duration::from_secs(10)),
            "the sentence does not go away on its own: {:?}",
            app.ui.status
        );
    }

    // ---- what a fetched list means --------------------------------------

    #[test]
    fn a_fetched_list_puts_the_reader_in_the_newest_conversation() {
        let mut app = App::new();

        open_first_chat(&mut app, vec![chat(CHAT), chat(CHAT + 1)]);

        assert_eq!(app.chats().len(), 2);
        assert_eq!(app.list.selected_chat, 0);
        assert_eq!(
            app.conversation.conversation.window.chat_id, CHAT,
            "the list is newest first, so the first entry is the one to open"
        );
        assert!(app.ui.status.contains('2'), "got {:?}", app.ui.status);
    }

    #[test]
    fn a_fetched_list_with_nobody_in_it_opens_nothing() {
        let mut app = App::new();

        open_first_chat(&mut app, Vec::new());

        assert!(app.chats().is_empty());
        assert_eq!(app.conversation.conversation.window.chat_id, 0);
    }

    /// The highlight moves on the keystroke, but the conversation it names is
    /// opened by the driver once the reader has stopped — and opening it has to
    /// forget the cursor, or the new conversation would be described by the old
    /// one's history and its first page would never be asked for.
    #[test]
    fn the_driver_opens_the_conversation_the_reader_stopped_on() {
        let mut app = app_with_unread_out_of_reach(2);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Off the chat list, onto the second conversation.
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));

        drive(&mut app, &mut state, &tx);
        assert_eq!(
            app.conversation.conversation.window.chat_id, CHAT,
            "the highlight has moved but the reader has not stopped"
        );

        std::thread::sleep(CHAT_SWITCH_DELAY);
        drive(&mut app, &mut state, &tx);

        assert_eq!(
            app.ui.pane,
            tui::app::Pane::Profile(tui::app::ProfileId::User(CHAT + 1)),
            "the reader stopped on the second chat, so its card is shown"
        );
        assert_eq!(
            app.conversation.conversation.window.chat_id, CHAT,
            "and the conversation is left as it was: browsing does not open it"
        );

        drive(&mut app, &mut state, &tx);
        assert_ne!(
            wanted(&app, state.history, Instant::now()),
            Wanted::Latest(CHAT + 1),
            "so nothing asks for the second chat's messages while only its card shows"
        );
    }

    // ---- what arrives ---------------------------------------------------

    #[test]
    fn an_older_page_lands_in_front_of_the_window() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        app.begin_fetch(FetchDirection::Older);

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Older,
                anchor: None,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, -1..=0)),
            },
        );

        assert_eq!(app.conversation.conversation.window.len(), 5);
        assert!(
            !app.is_fetching(FetchDirection::Older),
            "the direction is open again"
        );
    }

    /// A page around a message the reader asked for replaces the window rather
    /// than extending it, and the cursor is told so: it now describes the window
    /// that is on screen, not the one that was.
    #[test]
    fn a_page_around_a_jump_replaces_the_window() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);
        let jump = app
            .pending_jump()
            .expect("the reader asked to be taken to the unread messages");
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 16..=20)),
            },
        );

        assert_eq!(app.conversation.conversation.window.len(), 5);
        assert_eq!(
            app.conversation
                .conversation
                .window
                .get(app.conversation.vim.cursor())
                .map(|message| message.id),
            Some(19),
            "the reader is on the message the jump was for"
        );
        assert_eq!(app.pending_jump(), None, "and the jump is over");
        assert!(
            !app.conversation.conversation.window.exhausted_older
                && !app.conversation.conversation.window.exhausted_newer,
            "a window that jumped is surrounded by the unknown on both sides"
        );
        assert_eq!(
            state
                .history
                .cursor
                .and_then(|cursor| cursor.oldest_loaded_id()),
            Some(16),
            "the cursor counts from the window that replaced the old one"
        );
    }

    /// However a jump ended, it is over: a failure must not leave the key wedged,
    /// and it must not hold up the paging the reader did not interrupt.
    #[test]
    fn a_failed_jump_says_so_and_leaves_the_key_free() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);
        let jump = app
            .pending_jump()
            .expect("the reader asked to be taken to the unread messages");
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump,
                cursor: HistoryCursor::new(CHAT),
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );

        assert!(
            app.ui.status.contains("history:"),
            "got {:?}",
            app.ui.status
        );
        assert_eq!(
            app.pending_jump(),
            None,
            "asking again is one keystroke away, and a wedged key is not"
        );
        assert!(state.history.jump.is_none());
        assert!(
            state.history.retry_at.is_none(),
            "nothing asks again on its own, so holding every direction would stall \
             the paging the reader did not interrupt"
        );
        assert_eq!(
            app.conversation.conversation.window.len(),
            8,
            "and the reader stayed where they were"
        );
    }

    /// A page for a conversation the reader has left: the window refuses it, and
    /// the cursor is not told about a page that did not land.
    #[test]
    fn a_jump_for_a_conversation_that_is_no_longer_open_is_dropped() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);
        let jump = app
            .pending_jump()
            .expect("the reader asked to be taken to the unread messages");
        app.select_chat(1);

        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 16..=20)),
            },
        );

        assert_eq!(
            state
                .history
                .cursor
                .and_then(|cursor| cursor.oldest_loaded_id()),
            None,
            "the cursor kept describing the conversation it was for"
        );
    }

    /// A jump the reader abandoned — `Esc`, not that any more — leaves the
    /// cursor describing what is still on screen.
    #[test]
    fn an_abandoned_jump_leaves_the_cursor_alone() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);
        let jump = app
            .pending_jump()
            .expect("the reader asked to be taken to the unread messages");

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.pending_jump(), None);

        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 16..=20)),
            },
        );

        assert_eq!(
            app.conversation.conversation.window.len(),
            8,
            "the window kept what it had"
        );
        assert_eq!(
            state
                .history
                .cursor
                .and_then(|cursor| cursor.oldest_loaded_id()),
            None,
            "and the cursor was not told about a window that was never replaced"
        );
    }

    #[test]
    fn a_page_for_a_conversation_that_is_no_longer_open_is_dropped() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let mut state = State {
            history: opened(CHAT + 1),
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Latest,
                anchor: None,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 1..=2)),
            },
        );

        assert_eq!(
            app.conversation.conversation.window.len(),
            3,
            "the window on show kept what it had"
        );
        assert_eq!(
            state.history.cursor.map(|cursor| cursor.peer_id()),
            Some(CHAT + 1),
            "and the cursor still describes the conversation it was for"
        );
    }

    #[test]
    fn a_failed_first_page_leaves_the_conversation_to_be_opened_again() {
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Latest,
                anchor: None,
                cursor: HistoryCursor::new(CHAT),
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );

        assert!(
            app.ui.status.contains("history:"),
            "got {:?}",
            app.ui.status
        );
        assert!(
            state.history.cursor.is_none(),
            "nothing was loaded, so the next pass opens the conversation again"
        );
        assert!(state.history.retry_at.is_some(), "but not immediately");
    }

    #[test]
    fn a_client_that_could_not_be_brought_up_says_so_on_the_status_line() {
        let mut app = App::new();
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!(
                "no telegram application credentials configured"
            )),
        );

        assert!(
            app.ui.status.starts_with("offline:") && app.ui.status.contains("credentials"),
            "got {:?}",
            app.ui.status
        );
    }

    /// The launch bring-up is counted as in flight before it is issued: the state
    /// that can bring the client up is built for exactly that bring-up.
    #[test]
    fn a_state_that_can_bring_the_client_up_starts_with_one_in_flight() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let state = State::new(Config::default(), tx);

        assert!(
            state.bringing_up,
            "a `:retry` before the launch answered would be a second client"
        );
    }

    /// Signing out removes the drafts file, best-effort: the words belong to
    /// the account that is leaving, and a failed delete still signs out.
    #[test]
    fn signing_out_removes_the_drafts_file() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = DraftFile::new(dir.path().join("televim.drafts.json"));
        file.save(&[(CHAT, "unsent".to_owned())], Some("+1555"));
        assert!(dir.path().join("televim.drafts.json").exists());

        let mut app = app_with_a_conversation(CHAT, 2);
        let mut state = State::default();
        state.set_draft_file(file);
        apply_logged_out(&mut app, &mut state, Ok(()));

        assert!(
            !dir.path().join("televim.drafts.json").exists(),
            "the words went with the account"
        );
        assert_eq!(
            app.ui.status, "signed out",
            "and the sign-out itself is unaffected: {:?}",
            app.ui.status
        );
    }

    /// Signing out removes the history file too: the cached messages belong
    /// to the account that is leaving.
    #[test]
    fn signing_out_removes_the_history_file() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        HistoryFile::new(path.clone()).save(&HistoryCache::default(), Some("+1555"));
        assert!(path.exists());

        let mut app = app_with_a_conversation(CHAT, 2);
        let mut state = State::default();
        state.set_history_file(HistoryFile::new(path.clone()));
        apply_logged_out(&mut app, &mut state, Ok(()));

        assert!(!path.exists(), "the messages went with the account");
        assert_eq!(app.ui.status, "signed out");
    }

    // ---- the history cache ------------------------------------------------

    /// A state with [`CHAT`] open and the history file at `path`.
    fn caching_state(path: &std::path::Path) -> State {
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        state.set_history_file(HistoryFile::new(path.to_path_buf()));
        state
    }

    /// A page of [`CHAT`] arriving the way the fetch sends it.
    fn page_arrives(
        app: &mut App,
        state: &mut State,
        direction: FetchDirection,
        anchor: Option<i64>,
        page: Vec<Message>,
    ) {
        apply(
            app,
            state,
            Event::History {
                direction,
                anchor,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(page),
            },
        );
    }

    /// What the loop does after the pass, waited out: the write is taken off
    /// the loop's thread, so a test has to wait for it to land.
    async fn persist(state: &mut State) {
        state.persist_history(&[]);
        if let Some(write) = state.cached.write.take() {
            write.await.expect("the history write ran");
        }
    }

    fn cached_ids(state: &State) -> Vec<i64> {
        state
            .cached_history(CHAT)
            .iter()
            .map(|message| message.id)
            .collect()
    }

    fn file_ids(path: &std::path::Path) -> Vec<i64> {
        HistoryFile::new(path.to_path_buf())
            .load()
            .cache
            .get(CHAT)
            .iter()
            .map(|message| message.id)
            .collect()
    }

    #[tokio::test]
    async fn a_latest_page_lands_in_the_cache_and_on_disk() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = caching_state(&path);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=3),
        );
        assert_eq!(cached_ids(&state), vec![1, 2, 3], "in memory at once");
        assert!(!path.exists(), "and on disk only once the loop writes it");

        persist(&mut state).await;
        assert_eq!(file_ids(&path), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn only_numbered_messages_are_persisted() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = caching_state(&path);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, -2..=3),
        );
        persist(&mut state).await;

        assert_eq!(
            file_ids(&path),
            vec![1, 2, 3],
            "no placeholder reaches the file"
        );
    }

    #[tokio::test]
    async fn a_second_page_merges_and_the_cap_keeps_the_newest() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = caching_state(&path);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=150),
        );
        persist(&mut state).await;
        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 101..=250),
        );
        persist(&mut state).await;

        let kept = file_ids(&path);
        assert_eq!(kept, cached_ids(&state), "the file is the cache");
        assert_eq!(
            kept,
            (51..=250).collect::<Vec<_>>(),
            "merged rather than replaced, then cut to the newest 200"
        );
    }

    /// An older page joins the cache through the anchor it was counted from,
    /// which the event carries because the cursor beside it has moved on.
    #[test]
    fn an_older_page_from_the_cached_oldest_joins_the_cache() {
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 101..=200),
        );
        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Older,
            Some(101),
            messages(CHAT, 51..=100),
        );
        assert_eq!(cached_ids(&state), (51..=200).collect::<Vec<_>>());

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Older,
            Some(10),
            messages(CHAT, 1..=9),
        );
        assert_eq!(
            cached_ids(&state),
            (51..=200).collect::<Vec<_>>(),
            "a page counted from outside the cache cannot be shown to join it"
        );
    }

    #[tokio::test]
    async fn a_failed_write_does_not_fail_the_page() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        // A regular file where the directory should be, so the write fails.
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, "").expect("the blocking file");
        let path = blocker.join("televim.history.json");
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = caching_state(&path);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=3),
        );
        persist(&mut state).await;

        assert!(!path.exists());
        assert_eq!(cached_ids(&state), vec![1, 2, 3], "the cache is untouched");
        assert_eq!(
            app.conversation.conversation.window.len(),
            3,
            "and the page reached the screen"
        );
    }

    /// While a write is in flight no second one starts beside it, so an older
    /// snapshot can never land over a newer one; the newer is written once
    /// the first has finished.
    #[tokio::test]
    async fn a_write_waits_for_the_one_in_flight() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = caching_state(&path);
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        state.cached.write = Some(tokio::spawn(async move {
            let _ = held.await;
        }));

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=3),
        );
        state.persist_history(&[]);
        assert!(!path.exists(), "nothing started beside the write in flight");
        assert!(state.cached.dirty, "the snapshot is still owed");

        release.send(()).expect("the held write is waiting");
        while !state
            .cached
            .write
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            tokio::task::yield_now().await;
        }
        persist(&mut state).await;
        assert_eq!(file_ids(&path), vec![1, 2, 3]);
    }

    /// What the loop does after the pass with the list on screen, waited out.
    async fn persist_list(state: &mut State, app: &App) {
        state.persist_history(app.chats());
        if let Some(write) = state.cached.write.take() {
            write.await.expect("the history write ran");
        }
    }

    fn file_chats(path: &std::path::Path) -> Vec<(i64, Option<String>)> {
        HistoryFile::new(path.to_path_buf())
            .load()
            .cache
            .chats()
            .iter()
            .map(|chat| (chat.id, chat.last_message.as_deref().map(str::to_owned)))
            .collect()
    }

    /// The list a `Ready` lands is written behind, a quiet pass writes
    /// nothing, and the feed moving a preview owes the file the new list.
    #[tokio::test]
    async fn the_chat_list_is_written_behind_as_it_lands_and_moves() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = App::new();
        let mut state = State::default();
        state.set_history_file(HistoryFile::new(path.clone()));

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT), chat(CHAT + 1)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );
        persist_list(&mut state, &app).await;
        assert_eq!(file_chats(&path), vec![(CHAT, None), (CHAT + 1, None)]);

        state.persist_history(app.chats());
        assert!(
            !state.cached.dirty && state.cached.write.is_none(),
            "the same list on the next pass is no write"
        );

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT + 1, 9..=9).remove(0))),
        );
        persist_list(&mut state, &app).await;
        assert_eq!(
            file_chats(&path),
            vec![(CHAT, None), (CHAT + 1, Some("text".to_owned()))],
            "the arrival moved its conversation's preview"
        );
    }

    #[tokio::test]
    async fn signing_out_empties_the_cache_in_memory() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = caching_state(&path);
        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=3),
        );
        persist(&mut state).await;

        apply_logged_out(&mut app, &mut state, Ok(()));
        persist(&mut state).await;

        assert!(cached_ids(&state).is_empty(), "nothing left to seed from");
        assert!(!state.cached.dirty, "and nothing owed to the file");
        assert!(!path.exists(), "nor written back after the removal");
    }

    // ---- the feed into the cache ------------------------------------------

    /// [`CHAT`] open with its newest page, `1..=3`, landed — so its cached run
    /// is current — and nothing owed to the file yet.
    fn current_state() -> (App, State) {
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=3),
        );
        state.cached.dirty = false;
        (app, state)
    }

    fn arrives(app: &mut App, state: &mut State, id: i64) {
        apply(
            app,
            state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, id..=id).remove(0))),
        );
    }

    /// Edits and deletions are safe on any run, so they reach one the wire
    /// has not confirmed this session: here, one restored from the file.
    #[test]
    fn feed_edits_and_deletions_reach_the_cache_and_owe_a_write() {
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = state_caching(CHAT, 1..=3);

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::MessageEdited {
                chat_id: CHAT,
                message_id: 2,
                new_text: Cow::Borrowed("edited"),
            }),
        );
        assert_eq!(&*state.cached_history(CHAT)[1].text, "edited");
        assert!(state.cached.dirty, "the edit is owed to the file");

        state.cached.dirty = false;
        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::MessagesDeleted {
                message_ids: vec![1],
            }),
        );
        assert_eq!(cached_ids(&state), vec![2, 3]);
        assert!(state.cached.dirty, "the deletion is owed to the file");
    }

    #[test]
    fn a_feed_arrival_joins_the_cache_once_the_newest_page_has_landed() {
        let (mut app, mut state) = current_state();

        arrives(&mut app, &mut state, 4);

        assert_eq!(cached_ids(&state), vec![1, 2, 3, 4]);
        assert!(state.cached.dirty);
    }

    /// A run restored from the file may end long before the newest message,
    /// so an arrival is not appended to it.
    #[test]
    fn a_feed_arrival_is_left_out_of_a_run_not_confirmed_this_session() {
        let mut app = app_with_a_conversation(CHAT, 0);
        let mut state = state_caching(CHAT, 1..=3);

        arrives(&mut app, &mut state, 10);

        assert_eq!(cached_ids(&state), vec![1, 2, 3]);
        assert!(!state.cached.dirty, "nothing changed, nothing owed");
    }

    /// A send's answer carries its real identifier, and that is what is
    /// cached; the placeholder it replaces never was.
    #[test]
    fn a_send_the_server_numbered_joins_a_current_cache() {
        let (mut app, mut state) = current_state();
        let mut sent = messages(CHAT, 4..=4).remove(0);
        sent.is_outgoing = true;
        sent.status = MessageStatus::Sent;

        apply(
            &mut app,
            &mut state,
            Event::Sent {
                chat_id: CHAT,
                temp_id: -1,
                result: Ok(sent),
            },
        );

        assert_eq!(cached_ids(&state), vec![1, 2, 3, 4]);
        assert!(state.cached_history(CHAT)[3].is_outgoing);
        assert!(state.cached.dirty);
    }

    /// The feed ending, reading past a failure, or a new client coming up
    /// each may have missed a message, so no run is current behind them —
    /// until the next newest page lands.
    #[test]
    fn an_interrupted_feed_forgets_which_runs_are_current() {
        type Interrupt = fn(&mut App, &mut State);
        let interruptions: [(&str, Interrupt); 3] = [
            ("the feed ended", |app, state| {
                apply(app, state, Event::FeedEnded);
            }),
            ("the feed read past a failure", |app, state| {
                apply(
                    app,
                    state,
                    Event::FeedRetrying(FeedRetry {
                        reason: ProtoError::Framework(FrameworkError::UnknownPeer(CHAT)),
                        delay: RETRY,
                        attempt: 1,
                        attempts: CHAT_LIST_ATTEMPTS,
                    }),
                );
            }),
            ("a client came up", |app, state| {
                apply_ready_to_screen(
                    app,
                    state,
                    vec![chat(CHAT)],
                    Ok(domain::account::Account::default()),
                    tui::SessionStore::Keyring,
                );
            }),
        ];

        for (what, interrupt) in interruptions {
            let (mut app, mut state) = current_state();

            interrupt(&mut app, &mut state);
            arrives(&mut app, &mut state, 4);

            assert_eq!(cached_ids(&state), vec![1, 2, 3], "{what}");
            assert!(!state.cached.dirty, "{what}");
        }

        let (mut app, mut state) = current_state();
        apply(&mut app, &mut state, Event::FeedEnded);
        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=4),
        );
        arrives(&mut app, &mut state, 5);
        assert_eq!(
            cached_ids(&state),
            vec![1, 2, 3, 4, 5],
            "the next newest page makes the run current again"
        );
    }

    /// Signing out with no drafts file is still a sign-out: `clear` on a
    /// missing file succeeds rather than failing the logout.
    #[test]
    fn signing_out_without_a_drafts_file_still_signs_out() {
        let dir = tempfile::tempdir().expect("a scratch directory");

        let mut app = app_with_a_conversation(CHAT, 2);
        let mut state = State::default();
        state.set_draft_file(DraftFile::new(dir.path().join("televim.drafts.json")));
        apply_logged_out(&mut app, &mut state, Ok(()));

        assert_eq!(app.ui.status, "signed out");
    }

    /// An `offline:` is the end of a bring-up, so the retry the reader types at it
    /// is a retry of a bring-up that has stopped.
    #[test]
    fn an_offline_bring_up_is_no_longer_in_flight() {
        let mut app = App::new();
        let mut state = State {
            bringing_up: true,
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!("connection reset")),
        );

        assert!(!state.bringing_up, "got the offline screen");
    }

    /// The request is read before the client is looked up, because the state a
    /// retry is needed in is the state with no client — where the action drain
    /// never runs. The bring-up is issued, which is what the guard records.
    ///
    /// On a runtime because that is where a bring-up can be spawned at all: it is
    /// `tokio::spawn`, and a bring-up issued off one is a panic rather than a
    /// request.
    #[tokio::test]
    async fn a_retry_request_brings_the_client_up_again() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            cfg: Some(Config::default()),
            tx: Some(tx.clone()),
            ..State::default()
        };
        app.request_retry();

        drive(&mut app, &mut state, &tx);

        assert!(
            state.bringing_up,
            "nothing else sets it on this path, so this is the bring-up"
        );
        assert!(
            !app.take_retry_request(),
            "and the request was taken, so a second pass asks for nothing"
        );
    }

    /// A bring-up already in flight — the launch, or a retry's own attempt loop —
    /// is not restarted by a second `:retry`. The reader is told, transiently:
    /// the refusal is about this second press, not about the state of the client.
    #[tokio::test]
    async fn a_retry_asked_for_during_a_bring_up_is_refused() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            cfg: Some(Config::default()),
            tx: Some(tx.clone()),
            bringing_up: true,
            ..State::default()
        };
        app.request_retry();

        drive(&mut app, &mut state, &tx);

        assert!(
            app.ui.status.contains("already trying to connect"),
            "got {:?}",
            app.ui.status
        );
    }

    // ---- opening a conversation from the cache ---------------------------

    /// A state whose cache holds `ids` for `peer`, as a launch restores it.
    fn state_caching(peer: i64, ids: std::ops::RangeInclusive<i64>) -> State {
        let mut cache = HistoryCache::default();
        cache.put(peer, &messages(peer, ids));
        let mut state = State::default();
        state.restore_history(cache);
        state
    }

    /// An application with [`CHAT`] just opened and nothing loaded.
    fn app_just_opened() -> App {
        let mut app = App::new();
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(0);
        app
    }

    fn window_ids(app: &App) -> Vec<i64> {
        app.conversation
            .conversation
            .window
            .iter()
            .map(|message| message.id)
            .collect()
    }

    /// The driver's own pass, short of the request: what `wanted` names for a
    /// conversation that has just been opened, begun.
    fn open_pass(app: &mut App, state: &mut State) {
        assert_eq!(
            wanted(app, state.history, Instant::now()),
            Wanted::Latest(CHAT),
            "a conversation just opened asks for its newest page"
        );
        begin_latest(app, state, CHAT);
    }

    /// A warm cache paints the conversation before the wire has said anything,
    /// and says on the status line that it is waiting to be replaced; the
    /// newest page then replaces every cached row.
    #[test]
    fn a_cached_conversation_is_shown_until_its_newest_page_replaces_it() {
        let mut app = app_just_opened();
        let mut state = state_caching(CHAT, 1..=3);

        open_pass(&mut app, &mut state);

        assert_eq!(window_ids(&app), vec![1, 2, 3], "the cache is on screen");
        assert!(app.is_fetching(FetchDirection::Latest));
        assert_eq!(app.status_text(), REVALIDATING_LABEL);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 4..=6),
        );

        assert_eq!(window_ids(&app), vec![4, 5, 6], "the page replaced it");
        assert!(!app.is_revalidating());
        assert_ne!(app.status_text(), REVALIDATING_LABEL);
    }

    /// A cold cache is today's open: an empty window under the `Loading…` row
    /// until the page lands, and no word of a revalidation.
    #[test]
    fn a_cold_cache_opens_a_conversation_as_it_always_has() {
        let mut app = app_just_opened();
        let mut state = State::default();

        open_pass(&mut app, &mut state);

        assert!(app.conversation.conversation.window.is_empty());
        assert!(app.is_fetching(FetchDirection::Latest), "Loading… is drawn");
        assert!(!app.is_revalidating());
        assert_ne!(app.status_text(), REVALIDATING_LABEL);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 1..=2),
        );

        assert_eq!(window_ids(&app), vec![1, 2]);
    }

    /// What is cached for another conversation is never shown in this one.
    #[test]
    fn another_conversations_cache_does_not_seed_the_one_opened() {
        let mut app = app_just_opened();
        let mut state = state_caching(CHAT + 1, 1..=3);

        open_pass(&mut app, &mut state);

        assert!(app.conversation.conversation.window.is_empty());
        assert!(!app.is_revalidating());
    }

    /// A newest page that failed is asked for again once the backoff has
    /// passed, and that second open finds the cached rows still on screen: it
    /// leaves them be, and is revalidating again.
    #[test]
    fn a_retried_newest_page_leaves_the_cached_window_alone() {
        let mut app = app_just_opened();
        let mut state = state_caching(CHAT, 1..=3);
        open_pass(&mut app, &mut state);

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Latest,
                anchor: None,
                cursor: HistoryCursor::new(CHAT),
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );
        assert!(app.ui.status.contains("history:"), "the failure is said");
        assert_eq!(window_ids(&app), vec![1, 2, 3], "and the cache stays");

        state.history.retry_at = None;
        open_pass(&mut app, &mut state);

        assert_eq!(window_ids(&app), vec![1, 2, 3]);
        assert!(app.is_revalidating());
    }

    /// A launch with a warm cache: the chat list's round trip opens the first
    /// conversation, and the same pass seeds it, so the frame drawn after the
    /// list is the cached conversation rather than `Loading…`.
    #[test]
    fn the_launch_landing_opens_on_the_cached_conversation() {
        let mut app = App::new();
        let mut state = state_caching(CHAT, 1..=3);

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT), chat(CHAT + 1)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );
        open_pass(&mut app, &mut state);

        assert_eq!(window_ids(&app), vec![1, 2, 3]);
        assert_eq!(app.status_text(), REVALIDATING_LABEL);
    }

    /// `--chat` lands on the conversation it names, and that one is seeded
    /// rather than the head of the list.
    #[test]
    fn the_initial_chat_opens_on_its_cached_conversation() {
        let mut app = App::new();
        app.set_initial_chat(CHAT);
        let mut state = state_caching(CHAT, 1..=3);

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT + 1), chat(CHAT)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );
        open_pass(&mut app, &mut state);

        assert_eq!(app.current_chat_id(), CHAT);
        assert_eq!(window_ids(&app), vec![1, 2, 3]);
    }

    // ---- reading offline -------------------------------------------------

    /// A failed bring-up leaves no client, and a chat switch still paints
    /// the cache: nothing is asked for, the newest page is left unbegun for
    /// the client that comes up, and the status line still says `offline:`.
    #[test]
    fn a_chat_switch_offline_seeds_from_the_cache_and_asks_for_nothing() {
        let mut app = app_just_opened();
        let mut state = state_caching(CHAT + 1, 1..=3);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!("no route to the datacenter")),
        );

        app.select_chat(1);
        drive(&mut app, &mut state, &tx);

        assert_eq!(window_ids(&app), vec![1, 2, 3], "the cache is on screen");
        assert!(
            !app.is_fetching(FetchDirection::Latest),
            "nothing asked for"
        );
        assert!(
            !app.is_revalidating(),
            "so nothing is said to be on its way"
        );
        assert_eq!(state.history.cursor, None, "the newest page is still owed");
        assert!(
            app.status_text().starts_with("offline:"),
            "cached rows are not an answer: {:?}",
            app.status_text()
        );

        app.select_chat(0);
        drive(&mut app, &mut state, &tx);
        assert!(
            app.conversation.conversation.window.is_empty(),
            "an uncached conversation opens empty, as it always has offline"
        );
    }

    /// Once a newest page has been begun for the conversation, an empty
    /// window is the wire's answer, and losing the client does not paint the
    /// cache over it.
    #[test]
    fn a_window_the_wire_answered_empty_is_not_seeded_offline() {
        let mut app = app_just_opened();
        let mut state = state_caching(CHAT, 1..=3);
        state.history = opened(CHAT);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        drive(&mut app, &mut state, &tx);

        assert!(app.conversation.conversation.window.is_empty());
    }

    // ---- a launch drawn from the cache ----------------------------------

    /// A state as a warm launch restores it: the list `[CHAT, CHAT + 1]`, and
    /// `1..=3` cached for [`CHAT`].
    fn warm_state() -> State {
        let mut cache = HistoryCache::default();
        cache.put(CHAT, &messages(CHAT, 1..=3));
        assert!(cache.set_chats(&[chat(CHAT), chat(CHAT + 1)]));
        let mut state = State::default();
        state.restore_history(cache);
        state
    }

    /// An application as the runtime builds it before the first frame.
    fn launching() -> App {
        let mut app = App::new();
        "connecting…".clone_into(&mut app.ui.status);
        app
    }

    /// The conversation the window holds — not the highlight, which a list
    /// that lost the conversation moves off it.
    fn open_id(app: &App) -> i64 {
        app.conversation.conversation.window.chat_id
    }

    fn ready(app: &mut App, state: &mut State, chats: Vec<Chat>) {
        apply_ready_to_screen(
            app,
            state,
            chats,
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );
    }

    /// Before any `Ready`, with no client: the cached list and the first
    /// conversation, seeded — and the status line and the dot still saying
    /// that nothing has connected, because nothing has.
    #[test]
    fn a_warm_start_draws_the_cached_list_and_conversation_before_any_ready() {
        let mut app = launching();
        let state = warm_state();

        open_from_cache(&mut app, &state);

        assert!(state.client.is_none());
        assert_eq!(
            app.chats().iter().map(|chat| chat.id).collect::<Vec<_>>(),
            vec![CHAT, CHAT + 1]
        );
        assert_eq!(open_id(&app), CHAT);
        assert_eq!(window_ids(&app), vec![1, 2, 3], "the cache is on screen");
        assert!(
            !app.is_fetching(FetchDirection::Latest),
            "nothing asked for"
        );
        assert_eq!(app.status_text(), "connecting…");
        assert_eq!(app.connection(), ConnectionState::Connecting);
        assert_eq!(state.history.cursor, None, "the newest page is still owed");
    }

    /// The `Ready` lands over the open conversation: the list is refreshed
    /// around it, the reader stays, and the next pass asks for the newest page,
    /// which replaces the cached rows.
    #[test]
    fn a_ready_over_the_cached_launch_refreshes_around_it_and_asks_for_latest() {
        let mut app = launching();
        let mut state = warm_state();
        open_from_cache(&mut app, &state);

        ready(&mut app, &mut state, vec![chat(CHAT + 1), chat(CHAT)]);

        assert_eq!(app.connection(), ConnectionState::Connected);
        assert_eq!(open_id(&app), CHAT, "the reader stays");
        assert_eq!(window_ids(&app), vec![1, 2, 3]);
        assert_eq!(app.list.selected_chat, 1, "the highlight followed the id");
        assert_ne!(app.status_text(), "connecting…", "it has connected");

        open_pass(&mut app, &mut state);
        assert_eq!(window_ids(&app), vec![1, 2, 3], "not seeded twice");
        assert_eq!(app.status_text(), REVALIDATING_LABEL);

        page_arrives(
            &mut app,
            &mut state,
            FetchDirection::Latest,
            None,
            messages(CHAT, 4..=6),
        );
        assert_eq!(window_ids(&app), vec![4, 5, 6]);
    }

    /// The cached conversation is gone from the fresh list: the reconnect
    /// rule holds — the window stays, the highlight goes to the top, and the
    /// sentence says so — and the newest page is still asked for.
    #[test]
    fn a_ready_without_the_cached_conversation_keeps_it_and_says_so() {
        let mut app = launching();
        let mut state = warm_state();
        open_from_cache(&mut app, &state);

        ready(&mut app, &mut state, vec![chat(CHAT + 1)]);

        assert_eq!(open_id(&app), CHAT);
        assert_eq!(app.list.selected_chat, 0);
        assert!(
            app.ui.status.contains("no longer"),
            "got {:?}",
            app.ui.status
        );
        assert_eq!(
            wanted(&app, state.history, Instant::now()),
            Wanted::Latest(CHAT)
        );
    }

    /// `--chat` naming a cached conversation is opened before the wire answers,
    /// and spent: the `Ready` leaves the reader there.
    #[test]
    fn a_cached_initial_chat_opens_before_the_ready_and_is_spent() {
        let mut app = launching();
        app.set_initial_chat(CHAT + 1);
        let mut state = warm_state();

        open_from_cache(&mut app, &state);
        assert_eq!(open_id(&app), CHAT + 1);

        ready(&mut app, &mut state, vec![chat(CHAT), chat(CHAT + 1)]);
        assert_eq!(open_id(&app), CHAT + 1);
        assert_eq!(app.take_initial_chat(), None);
        assert!(!app.ui.status.contains("no chat with id"));
    }

    /// `--chat` naming a conversation the cache does not list is left for the
    /// `Ready`: the cached head is open meanwhile, and the fresh list either
    /// holds the id and moves the reader there, or does not and names it.
    #[test]
    fn an_uncached_initial_chat_waits_for_the_ready() {
        let mut app = launching();
        app.set_initial_chat(99);
        let mut state = warm_state();
        open_from_cache(&mut app, &state);
        assert_eq!(open_id(&app), CHAT, "the cached head meanwhile");

        ready(&mut app, &mut state, vec![chat(99), chat(CHAT)]);
        assert_eq!(open_id(&app), 99, "the fresh list held it");

        let mut app = launching();
        app.set_initial_chat(99);
        let mut state = warm_state();
        open_from_cache(&mut app, &state);

        ready(&mut app, &mut state, vec![chat(CHAT), chat(CHAT + 1)]);
        assert_eq!(open_id(&app), CHAT, "nowhere to go, so it stays");
        assert!(
            app.ui.status.contains("99"),
            "and the id is named: {:?}",
            app.ui.status
        );
    }

    /// A `Ready` with no session closes the cached conversation and forgets
    /// the cache, in memory and on disk, before the form opens: whoever signs
    /// in next may be another account.
    #[test]
    fn a_ready_with_no_session_takes_the_cache_down_before_the_form() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("televim.history.json");
        let mut app = launching();
        app.session.credentials_configured = true;
        let mut state = warm_state();
        HistoryFile::new(path.clone()).save(&state.cached.messages, None);
        state.set_history_file(HistoryFile::new(path.clone()));
        open_from_cache(&mut app, &state);

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Err(String::new()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(open_id(&app), 0, "no conversation behind the form");
        assert!(app.chats().is_empty());
        assert_eq!(app.signin_field(), Some(tui::app::LoginField::Phone));
        assert!(state.cached_history(CHAT).is_empty());
        assert!(state.cached.messages.chats().is_empty());
        assert!(!path.exists(), "and the file is gone");
        state.persist_history(app.chats());
        assert!(!state.cached.dirty, "nothing owed to the file");
    }

    /// The landing resumes the open conversation's draft into the line, and
    /// a warm launch lets the reader type into it before the wire answers:
    /// either way the `Ready` that follows leaves the words where they are.
    #[test]
    fn the_open_conversations_draft_survives_the_ready() {
        for (what, mut state) in [("warm", warm_state()), ("cold", State::default())] {
            let mut app = launching();
            app.drafts.restore(vec![(CHAT, "unsent".to_owned())]);
            open_from_cache(&mut app, &state);

            ready(&mut app, &mut state, vec![chat(CHAT), chat(CHAT + 1)]);

            assert_eq!(app.input.line.text(), "unsent", "{what}");
        }
    }

    /// A bring-up that fails leaves the cached list and conversation readable
    /// under the `offline:` sentence.
    #[test]
    fn a_failed_bring_up_leaves_the_cached_launch_readable() {
        let mut app = launching();
        let mut state = warm_state();
        open_from_cache(&mut app, &state);

        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!("no route to the datacenter")),
        );

        assert_eq!(app.chats().len(), 2);
        assert_eq!(window_ids(&app), vec![1, 2, 3]);
        assert!(app.status_text().starts_with("offline:"));
        assert_eq!(app.connection(), ConnectionState::Offline);
    }

    /// A cold cache draws nothing, and the `Ready` lands as it always has.
    #[test]
    fn a_cold_cache_launches_as_it_always_has() {
        let mut app = launching();
        let mut state = State::default();

        open_from_cache(&mut app, &state);

        assert!(app.chats().is_empty());
        assert_eq!(open_id(&app), 0);
        assert_eq!(app.status_text(), "connecting…");

        ready(&mut app, &mut state, vec![chat(CHAT), chat(CHAT + 1)]);
        assert_eq!(open_id(&app), CHAT);
        assert!(app.conversation.conversation.window.is_empty());
    }

    // ---- reconnecting when the feed ends --------------------------------

    /// One automatic reconnect, and only while nothing else is being brought up:
    /// the caller can tell the two refusals apart by [`State::bringing_up`], so a
    /// bring-up already on its way is left to answer for itself while the spent
    /// attempt is the failure.
    #[test]
    fn the_automatic_reconnect_is_allowed_once_and_never_over_a_bring_up() {
        assert!(
            auto_reconnect(&State::default()),
            "the first feed end may rebuild the client"
        );
        assert!(
            !auto_reconnect(&State {
                bringing_up: true,
                ..State::default()
            }),
            "a bring-up already in flight is not stacked on"
        );
        assert!(
            !auto_reconnect(&State {
                auto_reconnect_used: true,
                ..State::default()
            }),
            "the one attempt is spent"
        );
    }

    /// The feed's end says a reconnect is under way and gives up the client. The
    /// sentence is persistent and not a flash: it is replaced by the reconnect's
    /// own answer, not by a clock.
    #[test]
    fn the_feed_ending_says_it_is_reconnecting_and_gives_up_the_client() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        let mut state = State::default();

        apply(&mut app, &mut state, Event::FeedEnded);

        assert_eq!(app.ui.status, "reconnecting");
        assert!(
            state.reconnect_requested,
            "the driver is told to carry the reconnect out"
        );

        // The client flag is observable through the sign-in path, which reports
        // rather than claiming a request is on its way when there is no client.
        app.begin_signin();
        for ch in "123".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            app.take_action().is_none(),
            "nothing was queued against a client that is gone"
        );
        assert_eq!(
            app.ui.status, "not connected yet — the client is not up",
            "the client flag was cleared"
        );
    }

    /// The request is read at the top of the pass and issued through the same
    /// single-flight guard the reader's retry uses. On a runtime because a
    /// bring-up is `tokio::spawn`, and a bring-up issued off one is a panic
    /// rather than a request.
    #[tokio::test]
    async fn a_feed_end_brings_the_client_up_again_once() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            cfg: Some(Config::default()),
            tx: Some(tx.clone()),
            ..State::default()
        };

        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);

        assert!(state.bringing_up, "the bring-up was issued");
        assert!(
            state.auto_reconnect_used,
            "and the one automatic attempt is spent"
        );
        assert!(
            !state.take_reconnect_request(),
            "the request was taken, so a second pass asks for nothing"
        );
    }

    /// The second feed end before any update has arrived is not another rebuild:
    /// the one attempt is gone, so it lands on the reader-visible failure.
    #[test]
    fn a_second_feed_end_without_an_update_says_offline() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            auto_reconnect_used: true,
            ..State::default()
        };

        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);

        assert!(
            app.ui.status.starts_with("offline:") && app.ui.status.contains("feed"),
            "got {:?}",
            app.ui.status
        );
        assert!(!state.bringing_up, "and it is not a bring-up");
    }

    /// An update is the feed working, so it earns the next automatic reconnect:
    /// otherwise a reconnect to a feed that immediately ends again would rebuild
    /// the client for ever.
    #[test]
    fn an_update_earns_the_next_automatic_reconnect() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let mut state = State {
            auto_reconnect_used: true,
            ..State::default()
        };

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 4..=4).remove(0))),
        );

        assert!(
            !state.auto_reconnect_used,
            "a working feed re-arms the one reconnect"
        );
    }

    // ---- connection state -------------------------------------------------

    /// A state built before any event is a launch, and a launch is waiting for
    /// its `Ready`: the state agrees with the `"connecting…"` the runtime
    /// writes before the first bring-up answers.
    #[test]
    fn a_new_app_starts_connecting() {
        assert_eq!(App::new().connection(), ConnectionState::Connecting);
    }

    /// A chat-list retry in progress is a rebuild being waited out: the state
    /// holds reconnecting beside the sentence that says how long.
    #[test]
    fn a_chat_list_retry_marks_the_connection_as_reconnecting() {
        let mut app = App::new();
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::ChatListRetrying(ChatListRetry {
                reason: ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
                    code: 420,
                    name: "FLOOD_WAIT".to_owned(),
                    value: Some(31),
                })),
                delay: Duration::from_secs(31),
                attempt: 1,
                attempts: CHAT_LIST_ATTEMPTS,
            }),
        );

        assert_eq!(app.connection(), ConnectionState::Reconnecting);
        assert!(
            app.ui.status.contains("retrying in 31s"),
            "the sentence is untouched: {:?}",
            app.ui.status
        );
    }

    /// The feed's wait is the launch's wait, and the state beside it is the
    /// same one.
    #[test]
    fn a_feed_retry_marks_the_connection_as_reconnecting() {
        let mut app = App::new();
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::FeedRetrying(FeedRetry {
                reason: ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
                    code: 420,
                    name: "FLOOD_WAIT".to_owned(),
                    value: Some(31),
                })),
                delay: Duration::from_secs(31),
                attempt: 1,
                attempts: CHAT_LIST_ATTEMPTS,
            }),
        );

        assert_eq!(app.connection(), ConnectionState::Reconnecting);
        assert!(
            app.ui.status.contains("retrying in 31s"),
            "the sentence is untouched: {:?}",
            app.ui.status
        );
    }

    /// The feed's end asks for a rebuild: reconnecting, in words and in state.
    #[test]
    fn a_feed_end_marks_the_connection_as_reconnecting() {
        let mut app = App::new();
        let mut state = State::default();
        app.set_connection(ConnectionState::Connected);

        apply(&mut app, &mut state, Event::FeedEnded);

        assert_eq!(app.connection(), ConnectionState::Reconnecting);
        assert_eq!(app.ui.status, "reconnecting");
    }

    /// A spent budget is the reader-visible failure: the second feed end before
    /// any update lands as `offline:`, and the state lands as offline with it.
    #[test]
    fn an_exhausted_reconnect_marks_the_connection_as_offline() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            auto_reconnect_used: true,
            ..State::default()
        };
        app.set_connection(ConnectionState::Reconnecting);

        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);

        assert_eq!(app.connection(), ConnectionState::Offline);
        assert_eq!(app.ui.status, "offline: the update feed ended again");
    }

    /// A bring-up that failed says `offline:`, and the state says offline with
    /// it.
    #[test]
    fn an_offline_marks_the_connection_as_offline() {
        let mut app = App::new();
        let mut state = State::default();
        app.set_connection(ConnectionState::Connected);

        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!("connection reset")),
        );

        assert_eq!(app.connection(), ConnectionState::Offline);
        assert_eq!(app.ui.status, "offline: connection reset");
    }

    /// An update is the feed working: the connection holds connected, and the
    /// one reconnect the working feed earned is re-armed as before.
    #[test]
    fn an_update_marks_the_connection_as_connected() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let mut state = State {
            auto_reconnect_used: true,
            ..State::default()
        };
        app.set_connection(ConnectionState::Reconnecting);

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 4..=4).remove(0))),
        );

        assert_eq!(app.connection(), ConnectionState::Connected);
        assert!(
            !state.auto_reconnect_used,
            "a working feed still re-arms the one reconnect"
        );
    }

    /// A `Ready` is the client being up: connected, from whatever held before.
    #[test]
    fn a_ready_marks_the_connection_as_connected() {
        let mut app = App::new();
        let mut state = State::default();
        app.set_connection(ConnectionState::Reconnecting);

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(app.connection(), ConnectionState::Connected);
    }

    // ---- feed errors that recover, then exhaust ---------------------------

    /// A feed failure below the bound is waited out on the feed, not escalated:
    /// the policy answers `Some`, which is the pump sleeping and reading on —
    /// so no rebuild is asked for and nothing says `offline:`.
    #[test]
    fn a_feed_error_below_the_bound_schedules_a_wait_not_a_rebuild() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Network(
            "connection reset".to_owned(),
        )));

        assert!(
            feed_error_retry(0, &error).is_some(),
            "the first failure is waited out"
        );
        assert!(
            feed_error_retry(CHAT_LIST_ATTEMPTS - 1, &error).is_some(),
            "and so is the last one inside the bound"
        );

        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State::default();

        drive(&mut app, &mut state, &tx);

        assert!(!state.reconnect_requested, "no rebuild is asked for");
        assert!(!state.bringing_up, "and none is in flight");
        assert!(
            !app.ui.status.starts_with("offline:"),
            "and nothing says it: {:?}",
            app.ui.status
        );
    }

    /// Past the bound the pump gives up the feed — `None` is what sends it to
    /// record the position and report the end — which is the feed's end asking
    /// for the one rebuild. The retries before it spend nothing of the
    /// reconnect; only the rebuild the driver issues consumes it (Q4). The
    /// `Ready` that answers keeps the reader's place and drops the stale
    /// anchors.
    #[tokio::test]
    async fn a_feed_that_keeps_failing_is_rebuilt_around_the_reader() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Network(
            "connection reset".to_owned(),
        )));

        assert_eq!(
            feed_error_retry(CHAT_LIST_ATTEMPTS, &error),
            None,
            "the bound is what ends the feed"
        );

        let mut app = App::new();
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);
        app.select_chat(0);
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        for character in "half a th".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // Leaving the conversation parks the draft under it; the window the
        // reader is in is loaded after, so it is the one the rebuild finds.
        app.select_chat(1);
        app.apply_latest(messages(CHAT + 1, 1..=2));
        let read_at = app.conversation.vim.cursor();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            cfg: Some(Config::default()),
            tx: Some(tx.clone()),
            history: History {
                cursor: Some(HistoryCursor::new(CHAT + 1)),
                jump: Some(Jump {
                    peer_id: CHAT + 1,
                    target_id: 20,
                    kind: JumpKind::Unread,
                }),
                retry_at: Some(Instant::now() + RETRY),
            },
            ..State::default()
        };

        // What the pump sends on exhaustion, after recording the position.
        apply(&mut app, &mut state, Event::FeedEnded);

        assert!(
            state.reconnect_requested,
            "the driver is told to carry the reconnect out"
        );
        assert!(
            !state.auto_reconnect_used,
            "the retries and the request spend nothing of the one reconnect"
        );

        drive(&mut app, &mut state, &tx);

        assert!(state.bringing_up, "the bring-up was issued");
        assert!(
            state.auto_reconnect_used,
            "and only the rebuild consumes the one attempt"
        );

        // What the rebuild answers with. The old client is still the screen's
        // until this lands; the re-fetch only reorders the list.
        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT + 1), chat(CHAT)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.conversation.conversation.window.chat_id,
            CHAT + 1,
            "the conversation the reader was in is still the one on screen"
        );
        assert_eq!(
            app.conversation.conversation.window.len(),
            2,
            "with its loaded window intact"
        );
        assert_eq!(
            app.conversation.vim.cursor(),
            read_at,
            "and the reader where they were"
        );
        assert_eq!(
            app.list.selected_chat, 0,
            "the highlight followed the conversation's id into the reordered list"
        );
        assert_eq!(state.history.cursor, None, "the cursor goes");
        assert_eq!(state.history.jump, None, "and the jump on its way");
        assert_eq!(state.history.retry_at, None, "and the retry gate");

        // The draft parked under the other conversation is not a fact about
        // the page on show, so the rebuild leaves it alone.
        app.select_chat(1);
        assert_eq!(
            app.input.line.text(),
            "half a th",
            "and the parked draft is still where it was left"
        );
    }

    /// The rebuild the exhausted feed asked for is the one automatic attempt,
    /// so a second feed end before any update has arrived is the
    /// reader-visible failure rather than another rebuild.
    #[tokio::test]
    async fn a_second_exhausted_feed_end_before_any_update_says_the_feed_ended_again() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            cfg: Some(Config::default()),
            tx: Some(tx.clone()),
            ..State::default()
        };

        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);
        assert!(state.auto_reconnect_used, "the one attempt is spent");

        // The rebuild answers with its own failure rather than a client, and
        // no update arrives to re-arm the reconnect.
        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!("connection reset")),
        );
        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);

        assert_eq!(
            app.ui.status, "offline: the update feed ended again",
            "got {:?}",
            app.ui.status
        );
        assert!(!state.bringing_up, "and it is not a bring-up");
    }

    // ---- feed sentences that stay up ------------------------------------

    /// The regression `status_until` is: a flash deadline left over from
    /// something transient must not take a persistent feed sentence down on the
    /// next tick. Fired through `drive`, which is the clock that would do it.
    #[test]
    fn a_reconnecting_feed_sentence_survives_the_tick() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State::default();
        app.flash("something went wrong");

        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);

        assert_eq!(app.ui.status, "reconnecting", "got {:?}", app.ui.status);
    }

    /// The same deadline, cleared the same way, on the failure slot: an
    /// `offline:` is not a thing that passes on its own either.
    #[test]
    fn an_offline_sentence_survives_longer_than_a_flash() {
        let mut app = App::new();
        let mut state = State::default();
        app.flash("something went wrong");

        apply(
            &mut app,
            &mut state,
            Event::Offline(anyhow::anyhow!("connection reset")),
        );

        assert_eq!(app.ui.status, "offline: connection reset");
        assert!(
            !app.expire_status(Instant::now() + Duration::from_secs(10)),
            "the sentence does not go away on its own: {:?}",
            app.ui.status
        );
    }

    /// The whole visible sequence of an exhaustion with no reconnect left: the
    /// waits name their reason, wait and count, and the end past the bound is
    /// the reader-visible failure rather than another rebuild.
    #[test]
    fn an_exhausted_feed_end_with_no_reconnect_left_says_offline() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            // A previous rebuild spent the one attempt, and no update has
            // arrived since to re-arm it.
            auto_reconnect_used: true,
            ..State::default()
        };
        let retry = || FeedRetry {
            reason: ProtoError::Framework(FrameworkError::Request(RequestError::Network(
                "connection reset".to_owned(),
            ))),
            delay: RETRY,
            attempt: 0,
            attempts: CHAT_LIST_ATTEMPTS,
        };
        app.flash("something went wrong");

        for attempt in 1..=CHAT_LIST_ATTEMPTS {
            apply(
                &mut app,
                &mut state,
                Event::FeedRetrying(FeedRetry { attempt, ..retry() }),
            );
        }

        assert!(
            app.ui.status.contains("connection reset")
                && app
                    .ui
                    .status
                    .contains(&format!("{CHAT_LIST_ATTEMPTS}/{CHAT_LIST_ATTEMPTS}")),
            "the last wait is still waited out loud: {:?}",
            app.ui.status
        );

        // Past the bound the pump records the position and reports the end.
        apply(&mut app, &mut state, Event::FeedEnded);
        drive(&mut app, &mut state, &tx);

        assert_eq!(
            app.ui.status, "offline: the update feed ended again",
            "got {:?}",
            app.ui.status
        );
        assert!(!state.bringing_up, "and it is not a bring-up");
        assert!(
            !app.expire_status(Instant::now() + Duration::from_secs(10)),
            "and the failure stays up: {:?}",
            app.ui.status
        );
    }

    /// The way out of that failure is the reader's own `:retry`: unlike the
    /// automatic reconnect it is not a budgeted attempt, so it re-runs the
    /// bring-up — whose chat-list budget starts over inside it — and leaves
    /// the spent flag for the next update to clear.
    #[tokio::test]
    async fn a_retry_after_an_exhausted_feed_brings_the_client_up_again() {
        let mut app = App::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State {
            cfg: Some(Config::default()),
            tx: Some(tx.clone()),
            auto_reconnect_used: true,
            ..State::default()
        };
        app.ui.status = "offline: the update feed ended again".to_owned();

        app.request_retry();
        drive(&mut app, &mut state, &tx);

        assert!(state.bringing_up, "the bring-up was issued");
        assert!(
            !app.take_retry_request(),
            "the request was taken, so a second pass asks for nothing"
        );
        assert!(
            state.auto_reconnect_used,
            "the manual retry spends nothing of the automatic one"
        );
        assert_eq!(app.ui.status, "reconnecting", "got {:?}", app.ui.status);
    }

    /// The feed's event reaches the windows through the same call the screen's
    /// own tests pin, so this is only that the wiring is there at all.
    #[test]
    fn an_update_reaches_the_open_conversation() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::NewMessage(messages(CHAT, 4..=4).remove(0))),
        );

        assert_eq!(app.conversation.conversation.window.newest_id(), Some(4));
    }

    /// "Deleted 200 of 250" and "failed" are different events: one leaves a
    /// conversation to finish cleaning up, the other leaves the same one. Only the
    /// first is actionable, so it is the one the status line carries.
    #[test]
    fn a_deletion_that_got_part_way_says_how_far() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let partial = || {
            ProtoError::from(FrameworkError::PartialDelete {
                deleted: 200,
                source: Box::new(RequestError::Network("reset".to_owned())),
            })
        };

        assert_eq!(deleted_before_failure(&partial()), Some(200));

        apply_deleted(&mut app, CHAT, &[1, 2, 3], Err(partial()));

        assert_eq!(
            app.ui.status, "delete: 200 of 3 went through, the rest did not",
            "got {:?}",
            app.ui.status
        );
    }

    #[test]
    fn a_deletion_that_failed_before_it_started_is_reported_as_a_plain_failure() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let failed = || {
            ProtoError::from(FrameworkError::Request(RequestError::Network(
                "reset".to_owned(),
            )))
        };

        assert_eq!(deleted_before_failure(&failed()), None);

        apply_deleted(&mut app, CHAT, &[1], Err(failed()));

        assert_eq!(
            app.ui.status, "delete: network error: reset",
            "got {:?}",
            app.ui.status
        );
    }

    /// A forward that lands names the destination, and the count is what landed.
    #[test]
    fn a_forward_that_lands_names_the_destination() {
        let mut app = forward_app();

        apply_forwarded(&mut app, CHAT, DEST, 3, Ok(3));

        assert_eq!(
            app.ui.status, "Forwarded 3 message(s) to chat-9",
            "got {:?}",
            app.ui.status
        );
    }

    /// A partial forward says how many landed of how many were asked for, and
    /// names the source, since the rest can be found there.
    #[test]
    fn a_partial_forward_counts_what_landed_and_names_the_source() {
        let mut app = forward_app();
        let partial = ProtoError::from(FrameworkError::PartialForward {
            forwarded: 40,
            source: Box::new(RequestError::Network("reset".to_owned())),
        });

        apply_forwarded(&mut app, CHAT, DEST, 100, Err(partial));

        assert_eq!(
            app.ui.status, "Forwarded 40 of 100 message(s) from chat-7; the rest failed",
            "got {:?}",
            app.ui.status
        );
    }

    /// The wire's content-protection refusal names the source, because that is the
    /// conversation that does not allow it.
    #[test]
    fn a_refused_forward_names_the_source() {
        let mut app = forward_app();
        let refused = ProtoError::from(FrameworkError::Request(RequestError::Rpc {
            code: 406,
            name: "CHAT_FORWARDS_RESTRICTED".to_owned(),
            value: None,
        }));

        assert!(is_forward_refusal(&refused));
        apply_forwarded(&mut app, CHAT, DEST, 2, Err(refused));

        assert_eq!(
            app.ui.status, "chat-7 does not allow forwarding",
            "got {:?}",
            app.ui.status
        );
    }

    /// Only the content-protection name is a refusal; a flood wait or a plain
    /// network failure is reported as the failure it is.
    #[test]
    fn only_the_protected_content_name_is_a_forward_refusal() {
        let flood = ProtoError::from(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        }));
        let network = ProtoError::from(FrameworkError::Request(RequestError::Network(
            "reset".to_owned(),
        )));

        assert!(!is_forward_refusal(&flood));
        assert!(!is_forward_refusal(&network));
    }

    /// A forward that failed before anything landed is a plain failure line.
    #[test]
    fn a_forward_that_failed_outright_is_a_plain_failure() {
        let mut app = forward_app();
        let failed = ProtoError::from(FrameworkError::Request(RequestError::Network(
            "reset".to_owned(),
        )));

        apply_forwarded(&mut app, CHAT, DEST, 2, Err(failed));

        assert_eq!(
            app.ui.status, "forward: network error: reset",
            "got {:?}",
            app.ui.status
        );
    }

    /// A failure for a conversation the reader has left is not worth a line they
    /// cannot act on.
    #[test]
    fn a_deletion_that_failed_elsewhere_is_silent() {
        let mut app = app_with_a_conversation(CHAT, 3);

        apply_deleted(
            &mut app,
            CHAT + 1,
            &[1],
            Err(ProtoError::from(FrameworkError::PartialDelete {
                deleted: 200,
                source: Box::new(RequestError::Network("reset".to_owned())),
            })),
        );

        assert_eq!(app.ui.status, "televim", "got {:?}", app.ui.status);
    }

    // ---- what a send answers --------------------------------------------
    /// A result for a conversation the reader has left must still free the send
    /// key: the gate is released before the conversation is checked, because a
    /// return that happened first would wedge it with no visible symptom.
    #[test]
    fn a_send_result_frees_the_key_even_for_a_conversation_that_was_left() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.conversation.queue_send("hi", None);
        app.begin_send(temp_id);

        // The reader opens another conversation while the send is on its way.
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);
        assert_eq!(
            app.conversation.sending,
            Some(temp_id),
            "the send is still in flight"
        );

        let mut state = State::default();
        apply(
            &mut app,
            &mut state,
            Event::Sent {
                chat_id: CHAT,
                temp_id,
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );

        assert!(
            app.conversation.sending.is_none(),
            "a dropped result must not leave the send key wedged"
        );
    }

    /// The other half: the result changes nothing in the conversation the reader
    /// opened, because it was not for that one.
    #[test]
    fn a_send_result_for_a_conversation_that_is_no_longer_open_changes_nothing() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.conversation.queue_send("hi", None);
        app.begin_send(temp_id);
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);

        let mut state = State::default();
        apply(
            &mut app,
            &mut state,
            Event::Sent {
                chat_id: CHAT,
                temp_id,
                result: Ok(messages(CHAT, 99..=99).remove(0)),
            },
        );

        assert!(
            app.conversation.conversation.window.is_empty(),
            "the conversation the reader opened is still empty"
        );
    }

    /// One of the two things the temp-id reset in `App::select_chat` relies on:
    /// a send in flight in the conversation the reader left holds the key, so the
    /// next conversation cannot be handed the same placeholder id while the old
    /// result is outstanding.
    #[test]
    fn a_send_in_flight_holds_the_key_across_a_conversation_change() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.conversation.queue_send("first", None);
        app.begin_send(temp_id);
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);

        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        for character in "second".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            app.conversation.conversation.window.is_empty(),
            "the in-flight send holds the key, so the new conversation is not given the id"
        );
        assert!(
            app.ui.status.contains("already on its way"),
            "and the refusal is visible: {:?}",
            app.ui.status
        );
    }

    /// The other thing: a result for a conversation the reader left is dropped by
    /// `chat_id`, even when the open conversation has been handed the same
    /// placeholder id.
    ///
    /// The state here is not reachable through the interface — the test above is
    /// what keeps it unreachable — so this pins the rule rather than describing
    /// behaviour. It is the only test that fails if the `chat_id` check is
    /// removed as apparent dead code, which is the point of writing it at all.
    #[test]
    fn a_stale_result_does_not_confirm_a_placeholder_reissued_the_same_id() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let old_temp = app.conversation.conversation.queue_send("first", None);
        app.begin_send(old_temp);
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);

        // Reaching past the gate is deliberate: without it, this is the
        // collision the `chat_id` check has to survive.
        let new_temp = app.conversation.conversation.queue_send("second", None);
        app.begin_send(new_temp);
        assert_eq!(
            new_temp, old_temp,
            "the re-created view reissues the placeholder id"
        );

        let mut state = State::default();
        apply(
            &mut app,
            &mut state,
            Event::Sent {
                chat_id: CHAT,
                temp_id: old_temp,
                result: Ok(messages(CHAT, 99..=99).remove(0)),
            },
        );

        assert_eq!(
            app.conversation
                .conversation
                .message(new_temp)
                .map(|message| message.status),
            Some(MessageStatus::Sending),
            "a result for another conversation must not confirm this one's message"
        );
        assert!(
            app.conversation.conversation.window.newest_id().is_none(),
            "and the real message did not land here"
        );
    }

    #[test]
    fn a_send_result_replaces_the_placeholder_the_reader_was_shown() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.conversation.queue_send("hi", None);
        app.begin_send(temp_id);

        let mut state = State::default();
        apply(
            &mut app,
            &mut state,
            Event::Sent {
                chat_id: CHAT,
                temp_id,
                result: Ok(messages(CHAT, 4..=4).remove(0)),
            },
        );

        assert!(
            app.conversation.conversation.message(temp_id).is_none(),
            "the placeholder left"
        );
        assert_eq!(app.conversation.conversation.window.newest_id(), Some(4));
        assert!(app.conversation.sending.is_none(), "the key is free again");
    }

    #[test]
    fn a_failed_send_keeps_the_message_and_says_why() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.conversation.queue_send("hi", None);
        app.begin_send(temp_id);

        let mut state = State::default();
        apply(
            &mut app,
            &mut state,
            Event::Sent {
                chat_id: CHAT,
                temp_id,
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );

        assert_eq!(
            app.conversation
                .conversation
                .message(temp_id)
                .map(|message| message.status),
            Some(MessageStatus::Failed),
            "the reader's message stays, marked as failed"
        );
        assert!(
            app.conversation.conversation.failure(temp_id).is_some(),
            "and it carries a reason"
        );
        assert!(
            app.conversation.sending.is_none(),
            "the key is free to try again"
        );
    }

    /// A flood wait is a delay, not a freeze: it is labelled with the wait it
    /// asks for, and the key is freed at once rather than for the whole wait.
    #[test]
    fn a_flood_wait_is_labelled_with_the_wait_it_asks_for() {
        let error = ProtoError::Framework(FrameworkError::Request(RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(42),
        }));

        assert_eq!(failure_reason(&error), "flood wait, retry in 42s");
    }

    #[test]
    fn anything_else_is_labelled_with_its_own_description() {
        let error = ProtoError::Framework(FrameworkError::UnknownPeer(CHAT));

        assert!(
            failure_reason(&error).contains("peer cache"),
            "got {:?}",
            failure_reason(&error)
        );
    }

    // ---- what a search answers ------------------------------------------

    /// A search whose matches are the given places.
    ///
    /// The local pass is skipped: these tests are about matching an *answer* to
    /// the question it was asked for, and a real window would only give the
    /// fixture a list it then has to be reasoned about. The query is left as
    /// *answering an unrelated search* would.
    fn awaiting(query: &str) -> App {
        let mut app = app_with_a_conversation(CHAT, 3);
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in query.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = app.take_action();
        app
    }

    #[test]
    fn a_search_result_lands_on_the_match_list() {
        let mut app = awaiting("text");
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT,
                query: "text".to_owned(),
                result: Ok(SearchResults {
                    ids: vec![1, 3],
                    total: 2,
                }),
            },
        );

        assert_eq!(app.search_query(), Some("text"));
        assert_eq!(app.search().ids().to_vec(), vec![1, 3]);
        assert_eq!(app.search().total(), 2);
    }

    #[test]
    fn a_search_result_for_another_conversation_is_dropped() {
        let mut app = awaiting("text");
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT + 1,
                query: "text".to_owned(),
                result: Ok(SearchResults {
                    ids: vec![9],
                    total: 1,
                }),
            },
        );

        assert_eq!(
            app.search().source(),
            domain::search::SearchSource::Local,
            "the local list is still the one on screen"
        );
        assert_eq!(app.search().total(), 3, "and not the dropped answer's");
    }

    #[test]
    fn a_search_result_for_a_replaced_query_is_dropped() {
        let mut app = awaiting("first");
        let mut state = State::default();
        // The first search is answered, and the reader then types another.
        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT,
                query: "first".to_owned(),
                result: Ok(SearchResults {
                    ids: vec![2],
                    total: 1,
                }),
            },
        );
        let _ = app.take_action();
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "second".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = app.take_action();

        // The answer to the first search arrives while the second is being
        // asked for: it must not overwrite what the reader is looking at.
        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT,
                query: "first".to_owned(),
                result: Ok(SearchResults {
                    ids: vec![3],
                    total: 1,
                }),
            },
        );

        assert_eq!(
            app.search_query(),
            Some("second"),
            "the query the reader is asking still stands"
        );
        assert_eq!(
            app.search().total(),
            0,
            "and the second search's own local list is untouched"
        );
    }

    /// A failed search leaves the local list alone, and — the natural wrong
    /// wiring — must not hold up history paging.
    #[test]
    fn a_failed_search_keeps_the_local_list_and_does_not_stall_paging() {
        let mut app = awaiting("text");
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT,
                query: "text".to_owned(),
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );

        assert_eq!(
            app.search().ids().to_vec(),
            vec![1, 2, 3],
            "the reader asked a question and the window answered it"
        );
        assert!(
            state.history.retry_at.is_none(),
            "search failure must not hold every direction of history paging"
        );
    }

    #[test]
    fn a_flood_wait_on_a_search_is_labelled_with_the_wait_it_asks_for() {
        let mut app = awaiting("text");
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT,
                query: "text".to_owned(),
                result: Err(ProtoError::Framework(FrameworkError::Request(
                    RequestError::Rpc {
                        code: 420,
                        name: "FLOOD_WAIT".to_owned(),
                        value: Some(42),
                    },
                ))),
            },
        );

        assert!(
            app.search().label().contains("flood wait, retry in 42s"),
            "got {:?}",
            app.search().label()
        );
    }

    /// The old `gg` jump path is untouched by search, and a jump landing keeps
    /// the match list so the next `n` works.
    #[test]
    fn a_search_survives_a_jump_page() {
        let mut app = app_with_unread_out_of_reach(2);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "text".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = app.take_action();

        apply(
            &mut app,
            &mut state,
            Event::Searched {
                chat_id: CHAT,
                query: "text".to_owned(),
                result: Ok(SearchResults {
                    ids: vec![3, 19],
                    total: 2,
                }),
            },
        );

        ask_to_jump(&mut app);
        let jump = app
            .pending_jump()
            .expect("the reader asked to be taken to the unread messages");

        apply(
            &mut app,
            &mut state,
            Event::Jumped {
                jump,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 16..=20)),
            },
        );

        assert!(
            app.search().is_active(),
            "the landed page keeps the match list, so the next n works"
        );
        assert!(app.search().is_match(19));
    }

    // ---- signing in ----------------------------------------------------

    /// The one account meaning that is not a failure: no session at all.
    ///
    /// It travels as an empty `Err` rather than as an `Offline` because the
    /// signed-out card is a state the reader is in — and it draws no reason,
    /// which is why an empty reason is not a blank line on it.
    #[test]
    fn a_client_up_with_no_session_leaves_the_card_signed_out() {
        let mut app = App::new();
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Err(String::new()),
            tui::SessionStore::Keyring,
        );

        assert!(
            matches!(&app.session.account, AccountState::Unavailable(reason) if reason.is_empty()),
            "signed out with no reason to draw, not offline: {:?}",
            app.session.account
        );
        assert_eq!(app.session.session_store, tui::SessionStore::Keyring);
    }

    /// Credentials, no session: the reader lands on the form rather than on an
    /// empty chat list with a hint row they have to notice.
    ///
    /// It is `begin_signin` — the entry point `:signin` uses — and not a second
    /// way in, so the two cannot open differently.
    #[test]
    fn a_client_up_with_no_session_opens_the_sign_in_field() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Err(String::new()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(app.signin_field(), Some(tui::app::LoginField::Phone));
        assert_eq!(app.ui.focus, tui::Focus::Input, "the field has the keys");
    }

    /// A reason is not a sign-in. A session that authorizes with a profile this
    /// build could not read is a signed-in account, and answering that with a
    /// form would be wrong twice.
    #[test]
    fn a_reason_is_not_taken_for_a_missing_session() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Err("the profile could not be read".to_owned()),
            tui::SessionStore::Keyring,
        );

        assert!(app.signin().is_none(), "the account is signed in");
    }

    // ---- a launch carrying --chat -----------------------------------------

    /// A `Ready` landing on a pending `--chat` id opens that conversation:
    /// the id is taken once the list is set, and the launch lands there
    /// rather than on the first chat.
    #[test]
    fn a_ready_with_a_known_pending_chat_id_selects_it() {
        let mut app = App::new();
        app.set_initial_chat(CHAT + 1);
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT), chat(CHAT + 1)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.conversation.conversation.window.chat_id,
            CHAT + 1,
            "the requested conversation is the one on screen"
        );
        assert_eq!(app.list.selected_chat, 1, "and the highlight is on it");
        assert_eq!(
            app.take_initial_chat(),
            None,
            "the id applied once and is gone"
        );
    }

    /// An id the list does not hold is not invented: the launch lands on the
    /// first chat, and the status line names the unknown id persistently —
    /// unlike `:chat`, which stays silent.
    #[test]
    fn a_ready_with_an_unknown_pending_chat_id_lands_first_and_names_it() {
        let mut app = App::new();
        app.set_initial_chat(999);
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT), chat(CHAT + 1)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.conversation.conversation.window.chat_id, CHAT,
            "the launch landing, not an invented conversation"
        );
        assert_eq!(app.list.selected_chat, 0);
        assert!(
            app.ui.status.contains("999"),
            "the sentence names the id: {:?}",
            app.ui.status
        );
        assert_eq!(app.chats().len(), 2, "and nothing was added to the list");
    }

    /// The pending selection applies before the sign-in flow is entered, so a
    /// signed-out launch with `--chat` lands on the requested chat and then
    /// opens the form — not the other way round.
    #[test]
    fn a_signed_out_launch_with_a_pending_chat_id_selects_then_signs_in() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        app.set_initial_chat(CHAT + 1);
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT), chat(CHAT + 1)],
            Err(String::new()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.conversation.conversation.window.chat_id,
            CHAT + 1,
            "the requested conversation is on screen under the form"
        );
        assert_eq!(app.signin_field(), Some(tui::app::LoginField::Phone));
        assert_eq!(app.ui.focus, tui::Focus::Input, "the field has the keys");
    }

    // ---- a client brought back up over an open conversation --------------

    /// A `Ready` landing while a conversation is open is the client being
    /// brought back up, not a launch: the list is refreshed around the reader's
    /// place rather than the reader being moved to the top of it. The highlight
    /// comes back by id, because the re-fetched list is ordered by recency and
    /// an index means a different conversation on either side of the fetch.
    #[test]
    fn a_ready_over_an_open_conversation_keeps_it_and_restores_the_highlight_by_id() {
        let mut app = app_with_unread_out_of_reach(2);
        let read_at = app.conversation.vim.cursor();
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT + 1), chat(CHAT)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.conversation.conversation.window.chat_id, CHAT,
            "the conversation the reader was in is still the one on screen"
        );
        assert_eq!(
            app.conversation.conversation.window.len(),
            8,
            "with its loaded window intact"
        );
        assert_eq!(
            app.conversation.vim.cursor(),
            read_at,
            "and the reader where they were"
        );
        assert_eq!(
            app.list.selected_chat, 1,
            "the highlight followed the conversation's id into the reordered list"
        );
    }

    /// The stale network anchors describe a list and a feed that are gone, so
    /// they go with the `Ready`: a cursor whose page will never arrive is what
    /// wedges `wanted` on a preserved window (G5).
    #[test]
    fn a_ready_over_an_open_conversation_clears_the_stale_network_anchors() {
        let mut app = app_with_a_conversation(CHAT, 5);
        let mut state = State {
            history: History {
                cursor: Some(HistoryCursor::new(CHAT)),
                jump: Some(Jump {
                    peer_id: CHAT,
                    target_id: 20,
                    kind: JumpKind::Unread,
                }),
                retry_at: Some(Instant::now() + RETRY),
            },
            ..State::default()
        };

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(state.history.cursor, None, "the cursor goes");
        assert_eq!(state.history.jump, None, "and the jump on its way");
        assert_eq!(state.history.retry_at, None, "and the retry gate");
    }

    /// With the cursor cleared, a preserved window re-anchors from its own
    /// newest message: a paging direction, rather than the `Wanted::Nothing`
    /// that a stale cursor naming an empty window would leave forever.
    #[test]
    fn paging_re_anchors_after_a_ready_over_an_open_conversation() {
        let mut app = app_with_a_conversation(CHAT, 5);
        let mut state = State {
            history: opened(CHAT),
            ..State::default()
        };

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            wanted(&app, state.history, Instant::now()),
            Wanted::Latest(CHAT),
            "the preserved conversation asks for its newest page again"
        );
    }

    /// A re-fetch that no longer holds the reader's conversation keeps the
    /// window and falls back to the top of the list, saying so, rather than
    /// silently opening whichever conversation now happens to be first.
    #[test]
    fn a_ready_whose_conversation_is_gone_keeps_the_window_and_says_so() {
        let mut app = app_with_a_conversation(CHAT, 5);
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT + 1)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.conversation.conversation.window.chat_id, CHAT,
            "the window the reader was reading is preserved"
        );
        assert_eq!(
            app.list.selected_chat, 0,
            "the highlight falls back to the top"
        );
        assert!(
            app.ui.status.contains("no longer"),
            "and the reader is told where they landed: {:?}",
            app.ui.status
        );
    }

    /// A launch has nothing open, so the old landing is untouched: the reader
    /// is still put into the newest conversation rather than left on an empty
    /// screen.
    #[test]
    fn a_ready_with_no_conversation_open_still_selects_the_first_chat() {
        let mut app = App::new();
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            vec![chat(CHAT), chat(CHAT + 1)],
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(app.list.selected_chat, 0);
        assert_eq!(app.conversation.conversation.window.chat_id, CHAT);
    }

    // ---- a stored session that cannot be read --------------------------

    #[test]
    fn a_stored_session_that_cannot_be_read_is_discarded_and_the_startup_carries_on() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("session.json");
        // Bytes that are not a session: what a truncated write, or a file that
        // belongs to something else, looks like.
        std::fs::write(&path, b"not json").expect("the corrupt bytes are written");
        let store = FileStore::with_key_provider(&path, Arc::new(session_store_tests::Fixed));

        let sentence = discard_corrupt_session(&store).expect("corrupt bytes are discarded");
        assert!(!sentence.is_empty(), "the reader is told what happened");

        assert!(
            matches!(store.load(), Ok(None)),
            "so the store is the one a reader with no session has, rather than an \
             unreadable file that refuses every build"
        );
        assert!(
            !path.exists(),
            "and the unusable file is gone, rather than left to fail again"
        );
    }

    /// The other answer, and the one that must not touch anything: a store with
    /// a session in it is left exactly as it was found.
    #[test]
    fn a_healthy_stored_session_is_left_alone() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("session.json");
        let store = FileStore::with_key_provider(&path, Arc::new(session_store_tests::Fixed));
        store
            .save(&telegram_framework::SessionData::default())
            .expect("a session is written");

        assert_eq!(discard_corrupt_session(&store), None);
        assert!(
            store.load().expect("the session still reads").is_some(),
            "the reader's session is still there"
        );
    }

    /// Nothing stored is not corruption, and neither is a store that could not
    /// be reached: only `Corrupt` is a session to throw away, so every other
    /// answer leaves the store alone and leaves a failure where it was.
    #[test]
    fn a_store_with_nothing_in_it_is_not_a_corrupt_session() {
        let store = telegram_framework::MemoryStore::new();

        assert_eq!(discard_corrupt_session(&store), None);
        assert!(matches!(store.load(), Ok(None)), "and nothing was cleared");
    }

    /// The sentence is what the reader is left with, and it is the *status*
    /// line rather than a flash: a reader who reaches the sign-in form a minute
    /// later has to be able to read why their session is gone.
    #[test]
    fn a_discarded_session_says_so_on_the_status_line() {
        let mut app = App::new();
        let mut state = State::default();

        apply(
            &mut app,
            &mut state,
            Event::SessionDiscarded("the stored session could not be read".to_owned()),
        );

        assert_eq!(app.ui.status, "the stored session could not be read");
        assert!(
            state.client.is_none(),
            "the sentence is about a session, not about a client"
        );

        // No deadline on it: `expire_status` is the clock a flash carries.
        assert!(
            !app.expire_status(Instant::now() + Duration::from_secs(10)),
            "the sentence does not go away on its own"
        );
        assert_eq!(app.ui.status, "the stored session could not be read");
    }

    // ---- signing out ---------------------------------------------------

    /// The screen a successful sign-out leaves: no conversations, no account,
    /// no client, and **no field**.
    ///
    /// The last one is the assertion that earns the test. Opening the sign-in
    /// here as well would be the obvious thing to do and it is wrong twice:
    /// `set_chats` with an empty list closes the conversation and forgets the
    /// line's purpose, so the reader's next `⏎` would go down the message path.
    /// The `Ready` that follows opens the field instead.
    #[test]
    fn signing_out_empties_the_screen_and_the_client() {
        let mut app = App::new();
        let mut state = State::default();

        apply_logged_out(&mut app, &mut state, Ok(()));

        assert!(
            app.chats().is_empty(),
            "the conversations went with the session"
        );
        assert!(
            matches!(&app.session.account, AccountState::Unavailable(reason) if reason.is_empty()),
            "signed out, and not as a failure to read a profile: {:?}",
            app.session.account
        );
        assert!(state.client.is_none(), "and so did the client behind it");
        assert!(app.signin().is_none(), "the field is the next event's job");
        assert_eq!(app.ui.status, "signed out");
    }

    /// The whole sequence in two events: the sign-out empties the screen, and
    /// the fresh client's `Ready` — which finds no session, because the old one
    /// cleared the store — is what puts the reader back in front of a form.
    #[test]
    fn a_signed_out_client_comes_back_asking_to_sign_in() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        let mut state = State::default();

        apply_logged_out(&mut app, &mut state, Ok(()));
        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Err(String::new()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(app.signin_field(), Some(tui::app::LoginField::Phone));
        assert_eq!(
            app.ui.focus,
            tui::Focus::Input,
            "and the keys are going there"
        );
    }

    /// A failure is a failure, and the reader is still signed in: the framework
    /// revokes over the network warn-only and fails only when the *local*
    /// session could not be cleared. So the screen is left exactly as it was —
    /// wiping it would show a signed-in reader a signed-out program.
    #[test]
    fn a_sign_out_that_could_not_clear_the_session_leaves_the_reader_signed_in() {
        let mut app = App::new();
        let mut state = State::default();
        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        apply_logged_out(
            &mut app,
            &mut state,
            Err("the session could not be cleared".to_owned()),
        );

        assert!(matches!(app.session.account, AccountState::Known(_)));
        assert_eq!(
            app.ui.status,
            "could not sign out: the session could not be cleared"
        );
    }

    /// The account itself is the other answer: nothing opens, because there is
    /// nothing to sign in to.
    #[test]
    fn a_signed_in_account_opens_no_sign_in_field() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert!(app.signin().is_none());
        assert_eq!(app.signin_field(), None);
    }

    /// And a `Ready` landing on a flow the reader is already inside does not
    /// restart it. The reader who has sent a code keeps the step they sent it
    /// at — which is why both halves of this decision are read before
    /// `login_complete` rather than after.
    #[test]
    fn a_ready_does_not_replace_a_sign_in_the_reader_already_started() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        app.begin_signin();
        app.login_advanced(
            domain::session::SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        assert_eq!(app.signin_field(), Some(tui::app::LoginField::Code));
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Err(String::new()),
            tui::SessionStore::Keyring,
        );

        assert_eq!(
            app.signin_field(),
            Some(tui::app::LoginField::Code),
            "still the step they sent a code for, not a fresh phone field"
        );
    }

    /// Signing in ends with a `Ready` behind the account, and that is what takes
    /// the overlay down — a reader who typed a password and then typed nothing
    /// else must not be left with the form still up.
    #[test]
    fn a_signed_in_account_takes_the_sign_in_surface_down() {
        let mut app = App::new();
        app.session.credentials_configured = true;
        app.begin_signin();
        assert!(app.signin().is_some(), "the flow is up to begin with");
        let mut state = State::default();

        apply_ready_to_screen(
            &mut app,
            &mut state,
            Vec::new(),
            Ok(domain::account::Account::default()),
            tui::SessionStore::Keyring,
        );

        assert!(
            app.signin().is_none(),
            "and the conversation is the program again"
        );
        assert!(matches!(app.session.account, AccountState::Known(_)));
    }

    /// A machine with no `api_id` and `api_hash` gets a sentence rather than a
    /// bring-up failure: nothing failed, because nothing was built.
    #[test]
    fn no_credentials_opens_the_sentence_rather_than_a_failure() {
        let mut app = App::new();
        let mut state = State::default();

        apply(&mut app, &mut state, Event::NoCredentials);

        assert_eq!(app.signin(), Some(&tui::app::SignIn::NoCredentials));
        assert_eq!(app.ui.status, "televim has no application credentials");
        assert!(
            state.client.is_none(),
            "there is no client to have asked for anything"
        );
    }

    // ---- finding a person to start a conversation with --------------------

    /// A person a user search offered.
    fn candidate(user_id: i64) -> UserCandidate {
        UserCandidate {
            user_id,
            display_name: format!("user-{user_id}"),
            username: Some(format!("user{user_id}")),
        }
    }

    /// Runs `/`-on-the-chat-list and submits `query`, leaving the search open.
    ///
    /// The lookup the `⏎` queues is the caller's, not this test's, so it is
    /// drained rather than left to surprise the next read.
    fn begin_user_query(app: &mut App, query: &str) {
        app.ui.focus = Focus::ChatList;
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for character in query.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        while app.take_action().is_some() {}
    }

    /// An application with one conversation in the list and it open.
    fn app_with_a_chat() -> App {
        let mut app = App::new();
        app.set_chats(vec![chat(CHAT)]);
        app.select_chat(0);
        app
    }

    /// A handle is a query with an optional leading `@` and no whitespace; a
    /// name is anything else.
    #[test]
    fn a_username_looks_like_a_handle_and_a_name_does_not() {
        assert!(looks_like_username("@alice"));
        assert!(looks_like_username("alice"));
        assert!(
            looks_like_username("  @alice  "),
            "the prompt's surrounding spaces are not part of the name"
        );
        assert!(
            !looks_like_username("alice smith"),
            "whitespace makes it a name"
        );
        assert!(!looks_like_username(""), "an empty query names nobody");
        assert!(!looks_like_username("@"), "a bare at is not a handle");
    }

    /// One candidate is the answer to open; zero or many is a list to show.
    #[test]
    fn a_lone_candidate_is_opened_and_several_are_listed() {
        assert_eq!(sole_candidate(&[candidate(1)]).map(|u| u.user_id), Some(1));
        assert!(
            sole_candidate(&[candidate(1), candidate(2)]).is_none(),
            "two candidates are a list, not an answer"
        );
        assert!(
            sole_candidate(&[]).is_none(),
            "nobody is not an answer either"
        );
    }

    /// A name search that listed exactly one person opens that person's chat.
    #[test]
    fn a_lone_listed_candidate_opens_the_chat() {
        let mut app = app_with_a_chat();
        let mut state = State::default();
        begin_user_query(&mut app, "ada");

        apply(
            &mut app,
            &mut state,
            Event::UsersListed {
                query: "ada".to_owned(),
                users: vec![candidate(CHAT + 1)],
            },
        );

        assert_eq!(
            app.current_chat_id(),
            CHAT + 1,
            "the lone result is the answer"
        );
        assert_eq!(app.chats().len(), 2, "and it was listed");
        assert!(!app.user_search().is_active(), "the overlay is put away");
    }

    /// A resolution that found nobody is not an answer to act on: the name
    /// search it falls back to is still in flight, and its list lands after.
    #[test]
    fn a_resolution_without_a_person_defers_to_the_listed_candidates() {
        let mut app = app_with_a_chat();
        let mut state = State::default();
        begin_user_query(&mut app, "ada");

        apply(
            &mut app,
            &mut state,
            Event::UserResolved {
                query: "ada".to_owned(),
                user: None,
            },
        );
        assert!(
            app.user_search().is_active(),
            "the fallback is still on its way"
        );

        apply(
            &mut app,
            &mut state,
            Event::UsersListed {
                query: "ada".to_owned(),
                users: vec![candidate(CHAT + 1), candidate(CHAT + 2)],
            },
        );

        assert!(
            app.user_search().is_active(),
            "two candidates are a list to choose from"
        );
        assert_eq!(app.user_search().candidates().len(), 2);
        assert_eq!(
            app.current_chat_id(),
            CHAT,
            "and no chat was opened outright"
        );
    }

    /// A resolution for a query the reader has replaced opens nothing: the chat
    /// it would open is one nobody is looking for any more.
    #[test]
    fn a_resolution_for_a_replaced_query_opens_nothing() {
        let mut app = app_with_a_chat();
        let mut state = State::default();
        begin_user_query(&mut app, "bar");

        apply(
            &mut app,
            &mut state,
            Event::UserResolved {
                query: "foo".to_owned(),
                user: Some(candidate(CHAT + 1)),
            },
        );

        assert_eq!(app.current_chat_id(), CHAT, "the stale answer is dropped");
        assert!(app.user_search().is_active());
    }

    /// A lookup that failed records the reason on the search's own label.
    #[test]
    fn a_failed_lookup_lands_on_the_user_search() {
        let mut app = app_with_a_chat();
        let mut state = State::default();
        begin_user_query(&mut app, "ada");

        apply(
            &mut app,
            &mut state,
            Event::UserLookupFailed {
                query: "ada".to_owned(),
                reason: "flood wait, retry in 5s".to_owned(),
            },
        );

        assert!(app.user_search().is_active());
        assert_eq!(
            app.user_search().label(),
            "/ada — no candidates (search failed: flood wait, retry in 5s)"
        );
    }

    /// A settled sticker releases its in-flight mark only when it is for the
    /// chat on show: a settle for another chat leaves the mark held, so the
    /// pair is not queued twice, and a failed fetch for this chat releases it,
    /// so the next miss re-requests.
    #[test]
    fn a_settled_sticker_releases_its_mark_only_for_the_chat_on_show() {
        let mut app = App::new();
        let chat = app.current_chat_id();
        app.conversation.stickers.request(chat, 7);
        assert_eq!(app.conversation.stickers.take_pending(), vec![(chat, 7)]);

        apply_sticker_settled(&mut app, chat + 1, 7, Err("gone".to_owned()));
        app.conversation.stickers.request(chat, 7);
        assert!(
            app.conversation.stickers.take_pending().is_empty(),
            "a settle for another chat leaves the pair in flight"
        );

        apply_sticker_settled(&mut app, chat, 7, Err("gone".to_owned()));
        app.conversation.stickers.request(chat, 7);
        assert_eq!(
            app.conversation.stickers.take_pending(),
            vec![(chat, 7)],
            "a failed fetch for this chat is re-requested on the next miss"
        );
    }
}

#[cfg(test)]
mod session_store_tests {
    use telegram_framework::{FileKey, SessionData};

    use super::*;
    use crate::config::Secret;

    /// A key that never touches the OS credential store.
    #[derive(Debug)]
    pub(super) struct Fixed;

    impl KeyProvider for Fixed {
        fn sealing_key(&self) -> Result<([u8; 16], FileKey), SessionError> {
            Ok(([7; 16], FileKey::from_bytes([7; 32])))
        }

        fn opening_key(&self, _salt: &[u8; 16]) -> Result<FileKey, SessionError> {
            Ok(FileKey::from_bytes([7; 32]))
        }
    }

    /// A credential store that cannot be reached: a headless machine.
    #[derive(Debug)]
    struct NoKeyring;

    impl KeyProvider for NoKeyring {
        fn sealing_key(&self) -> Result<([u8; 16], FileKey), SessionError> {
            Err(SessionError::Unavailable("no secret service".to_owned()))
        }

        fn opening_key(&self, _salt: &[u8; 16]) -> Result<FileKey, SessionError> {
            Err(SessionError::Unavailable("no secret service".to_owned()))
        }
    }

    fn file_config(path: &std::path::Path, passphrase: Option<&str>) -> Config {
        Config {
            session_path: Some(path.to_owned()),
            session_passphrase: passphrase.map(|text| Secret::from(text.to_owned())),
            ..Config::default()
        }
    }

    /// The panel names the store, and it names the one the session is in. The
    /// two are the same decision rather than two reads of a field, so this test
    /// is about the description alone — which is the half a reader sees.
    #[test]
    fn the_panel_names_the_store_the_session_goes_in() {
        let cfg = Config::default();
        assert_eq!(
            cfg.session_path, None,
            "nothing named, so nothing asked for"
        );
        assert_eq!(session_description(&cfg), tui::SessionStore::Keyring);

        let cfg = Config {
            session_path: Some("/tmp/televim.session".into()),
            ..Config::default()
        };
        assert_eq!(
            session_description(&cfg),
            tui::SessionStore::EncryptedFile("/tmp/televim.session".into()),
            "a file is named, and named as the encrypted thing it is"
        );

        // The key is not the store: the passphrase changes where the key comes
        // from and nothing the reader is told about where the session is.
        let cfg = file_config("/tmp/televim.session".as_ref(), Some("hunter2"));
        assert_eq!(
            session_description(&cfg),
            tui::SessionStore::EncryptedFile("/tmp/televim.session".into())
        );
    }

    /// With a passphrase set, what lands on disk is the envelope, and the keyring
    /// is never asked.
    #[test]
    fn a_passphrase_makes_the_file_ciphertext_and_the_keyring_is_not_asked() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("session.bin");
        let cfg = file_config(&path, Some("test"));

        let Session { store, keys } =
            session_store_with(&cfg, || panic!("the keyring is not asked"));
        keys.expect("a file has a key source")
            .sealing_key()
            .expect("a passphrase is a key");
        store
            .save(&SessionData::default())
            .expect("the session is saved");

        let bytes = std::fs::read(&path).expect("the file exists");
        assert!(bytes.starts_with(b"TVIM1"), "an envelope, not JSON");
        assert!(
            !bytes.windows(7).any(|window| window == b"version"),
            "and nothing readable inside it"
        );
        assert!(
            store.load().expect("it opens again").is_some(),
            "under the same passphrase"
        );
    }

    /// No passphrase falls through to the keyring's key, and a blank one is no
    /// passphrase.
    #[test]
    fn without_a_passphrase_the_keyring_key_seals_the_file() {
        for passphrase in [None, Some("   ")] {
            let dir = tempfile::tempdir().expect("a scratch directory");
            let path = dir.path().join("session.bin");
            let cfg = file_config(&path, passphrase);

            let Session { store, keys } = session_store_with(&cfg, || Arc::new(Fixed));
            keys.expect("a file has a key source")
                .sealing_key()
                .expect("the keyring has a key");
            store
                .save(&SessionData::default())
                .expect("the session is saved");

            assert!(
                std::fs::read(&path)
                    .expect("the file exists")
                    .starts_with(b"TVIM1"),
                "ciphertext under the keyring key (passphrase {passphrase:?})"
            );
        }
    }

    /// The credential store is not even looked at when there is no file for it
    /// to be the key to.
    #[test]
    fn no_file_configured_means_no_key_source() {
        let cfg = Config::default();
        let session = session_store_with(&cfg, || panic!("the keyring is not asked"));
        assert!(session.keys.is_none());
    }

    /// Neither key: an `offline:` sentence that names what to set, and the file
    /// is exactly as it was — no discard, and no plaintext written to get round
    /// it.
    #[test]
    fn with_no_key_at_all_the_reader_is_told_which_and_the_file_is_untouched() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("session.bin");
        let sealed = {
            let store = FileStore::with_key_provider(&path, Arc::new(Fixed));
            store
                .save(&SessionData::default())
                .expect("a session is written");
            std::fs::read(&path).expect("the file exists")
        };
        let cfg = file_config(&path, None);

        let Session { store, keys } = session_store_with(&cfg, || Arc::new(NoKeyring));
        assert_eq!(
            discard_corrupt_session(&*store),
            None,
            "an unreachable key is not a corrupt session"
        );
        let error = keys
            .expect("a file has a key source")
            .sealing_key()
            .expect_err("there is no key");
        let sentence = format!("{:#}", key_trouble(&error));
        assert!(
            sentence.contains("TELEVIM_SESSION_PASSPHRASE") && sentence.contains("keyring"),
            "the sentence names both keys: {sentence}"
        );
        assert!(sentence.contains("left as it was"), "{sentence}");

        assert_eq!(
            std::fs::read(&path).expect("the file is still there"),
            sealed,
            "byte for byte"
        );
    }

    /// A wrong passphrase is a typo, not a corrupt session: the file stays, and
    /// the sentence points at the passphrase.
    #[test]
    fn a_wrong_passphrase_leaves_the_file_and_names_the_passphrase() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let path = dir.path().join("session.bin");
        FileStore::with_key_provider(&path, Arc::new(PassphraseProvider::new("right")))
            .save(&SessionData::default())
            .expect("a session is written");
        let sealed = std::fs::read(&path).expect("the file exists");

        let cfg = file_config(&path, Some("wrong"));
        let Session { store, .. } = session_store_with(&cfg, || Arc::new(NoKeyring));

        assert_eq!(discard_corrupt_session(&*store), None, "not discarded");
        let error = store.load().expect_err("the wrong key does not open it");
        assert!(matches!(error, SessionError::Load(_)), "{error:?}");
        let sentence = format!("{:#}", key_trouble(&error));
        assert!(
            sentence.contains("TELEVIM_SESSION_PASSPHRASE"),
            "{sentence}"
        );
        assert_eq!(
            std::fs::read(&path).expect("the file is still there"),
            sealed
        );
    }
}

/// Tests for the media open: the suffix table, the failure sentences, the kind
/// lookup and the file write. Its own module because the helpers above want
/// their own sample messages, not the ones the chat tests build.
#[cfg(test)]
mod media_tests {
    use std::borrow::Cow;
    use std::path::PathBuf;

    use domain::message::MessageStatus;

    use super::*;

    /// The conversation the sample messages belong to.
    const CHAT: i64 = 7;
    /// A media message of `kind` in `chat_id`, for the view the open looks in.
    fn media_message(chat_id: i64, id: i64, kind: Option<MediaKind>) -> Message {
        Message {
            id,
            chat_id,
            text: Cow::Borrowed(""),
            timestamp: id,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: kind,
            media_id: None,
        }
    }

    /// An oversize media names the limit, and every other failure is a sentence
    /// that says nothing was saved.
    #[test]
    fn every_download_failure_becomes_a_sentence_that_says_nothing_was_saved() {
        let too_large = ProtoError::Framework(FrameworkError::MediaTooLarge {
            peer_id: 1,
            message_id: 2,
            size: MEDIA_LIMIT + 1,
            limit: MEDIA_LIMIT,
        });
        let text = media_failure(&too_large);
        assert!(text.contains("16 MiB"), "{text}");
        assert!(text.contains("nothing was saved"), "{text}");

        let unavailable = ProtoError::Framework(FrameworkError::MediaUnavailable {
            peer_id: 1,
            message_id: 2,
        });
        assert!(media_failure(&unavailable).contains("no media televim can fetch"));

        let out_of_range = ProtoError::MessageIdOutOfRange {
            peer_id: 1,
            id: i64::MAX,
        };
        assert!(media_failure(&out_of_range).contains("outside telegram's range"));
    }

    /// The kind is read only from a message on show, in the chat it was asked
    /// for: a message that is gone, bare, or in another chat is refused.
    #[test]
    fn an_open_resolves_the_kind_only_for_a_message_on_show_with_media() {
        let mut view = ConversationView::new(CHAT);
        view.window.replace([
            media_message(CHAT, 1, Some(MediaKind::Gif)),
            media_message(CHAT, 2, None),
        ]);

        assert_eq!(open_media_kind(&view, CHAT, 1), Some(MediaKind::Gif));
        assert_eq!(open_media_kind(&view, CHAT, 2), None, "carries no media");
        assert_eq!(open_media_kind(&view, CHAT, 3), None, "not on show");
        assert_eq!(open_media_kind(&view, CHAT + 1, 1), None, "another chat");
    }

    /// A miss stores the download and answers with its file; the next open of the
    /// same message is answered from disk, with no download to ask for.
    #[test]
    fn a_download_is_stored_once_and_the_next_open_is_served_from_disk() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let cache = Mutex::new(MediaCache::open(dir.path().to_path_buf(), None, None));

        assert!(
            cached_media(&cache, CHAT, 5, None).is_none(),
            "nothing cached yet"
        );

        let Event::MediaSaved { path, .. } =
            keep_download(&cache, CHAT, 5, None, MediaKind::Video, b"clip")
        else {
            panic!("a stored download answers with its file");
        };
        assert_eq!(std::fs::read(&path).expect("the file"), b"clip");

        let Some(Event::MediaSaved {
            path: again,
            message_id,
            ..
        }) = cached_media(&cache, CHAT, 5, None)
        else {
            panic!("the cached message is served from disk");
        };
        assert_eq!(again, path);
        assert_eq!(message_id, 5);
    }

    /// An edit drops its message's cached file, and only that one.
    #[test]
    fn an_edited_message_drops_its_cache_entry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let mut app = App::new();
        let mut state = State::default();
        state.set_media_cache(MediaCache::open(dir.path().to_path_buf(), None, None));
        let cache = state.media_cache.clone().expect("the cache is set");
        keep_download(&cache, CHAT, 5, None, MediaKind::Video, b"clip");
        keep_download(&cache, CHAT, 6, None, MediaKind::Video, b"more");
        keep_download(&cache, CHAT, 7, None, MediaKind::Video, b"own");
        keep_download(&cache, CHAT + 1, 5, None, MediaKind::Video, b"other");

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::MessageEdited {
                chat_id: CHAT,
                message_id: 6,
                new_text: Cow::Borrowed("edited"),
            }),
        );
        assert!(
            cached_media(&cache, CHAT, 6, None).is_none(),
            "the edited file goes"
        );
        assert!(cached_media(&cache, CHAT, 5, None).is_some(), "others stay");
    }

    /// A deletion drops the cached file whichever way it arrives, the feed's or
    /// the reader's own. The feed names no chat, so the id's copy in every chat
    /// goes.
    #[test]
    fn a_deleted_message_is_dropped_from_the_cache_in_every_chat() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let mut app = App::new();
        let mut state = State::default();
        state.set_media_cache(MediaCache::open(dir.path().to_path_buf(), None, None));
        let cache = state.media_cache.clone().expect("the cache is set");
        keep_download(&cache, CHAT, 5, None, MediaKind::Video, b"clip");
        keep_download(&cache, CHAT + 1, 5, None, MediaKind::Video, b"other");
        keep_download(&cache, CHAT, 7, None, MediaKind::Video, b"own");

        apply(
            &mut app,
            &mut state,
            Event::Update(UpdateEvent::MessagesDeleted {
                message_ids: vec![5],
            }),
        );
        assert!(cached_media(&cache, CHAT, 5, None).is_none());
        assert!(
            cached_media(&cache, CHAT + 1, 5, None).is_none(),
            "the same id in another chat goes too"
        );

        apply(
            &mut app,
            &mut state,
            Event::Deleted {
                chat_id: CHAT,
                message_ids: vec![7],
                result: Ok(()),
            },
        );
        assert!(
            cached_media(&cache, CHAT, 7, None).is_none(),
            "the reader's own delete too"
        );
    }

    /// A file downloaded for one chat is cached under its media id too, so the
    /// same file forwarded into another chat is served from disk. A hit is an
    /// answer, and `save_media` returns it before any download is asked for.
    #[test]
    fn a_forwarded_file_in_another_chat_is_served_from_the_cache() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let cache = Mutex::new(MediaCache::open(dir.path().to_path_buf(), None, None));
        let Event::MediaSaved { path, .. } =
            keep_download(&cache, CHAT, 5, Some(77), MediaKind::Video, b"clip")
        else {
            panic!("a stored download answers with its file");
        };

        let Some(Event::MediaSaved {
            path: forwarded,
            chat_id,
            message_id,
        }) = cached_media(&cache, CHAT + 1, 9, Some(77))
        else {
            panic!("the forwarded copy is served from disk");
        };
        assert_eq!(forwarded, path);
        assert_eq!((chat_id, message_id), (CHAT + 1, 9));
    }

    /// Without an id a message is looked up by its own key alone, as it always
    /// was: another chat's copy is not its file, and an id that was never stored
    /// falls back to the message's key.
    #[test]
    fn a_file_without_an_id_falls_back_to_the_message_key() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let cache = Mutex::new(MediaCache::open(dir.path().to_path_buf(), None, None));
        keep_download(&cache, CHAT, 5, None, MediaKind::Video, b"clip");

        assert!(
            cached_media(&cache, CHAT + 1, 9, None).is_none(),
            "no id and another chat is a miss"
        );
        assert!(cached_media(&cache, CHAT, 5, None).is_some());
        assert!(
            cached_media(&cache, CHAT, 5, Some(77)).is_some(),
            "an id that is not stored falls back to the message's own key"
        );
    }

    /// The progress callback reports each chunk on the channel, in order; once the
    /// cancel flag is set it answers `false`, and that chunk is not reported.
    #[test]
    fn progress_reports_each_chunk_and_a_cancel_stops_the_next() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = media_progress(tx, CHAT, 9, Arc::clone(&cancel));

        assert!(progress(3, Some(10)));
        assert!(progress(7, Some(10)));
        cancel.store(true, Ordering::Relaxed);
        assert!(
            !progress(8, Some(10)),
            "a cancel stops the transfer before its next chunk"
        );

        let mut seen = Vec::new();
        while let Ok(AppEvent::Net(Event::MediaProgress {
            downloaded, total, ..
        })) = rx.try_recv()
        {
            seen.push((downloaded, total));
        }
        assert_eq!(
            seen,
            vec![(3, Some(10)), (7, Some(10))],
            "the refused chunk is not reported"
        );
    }

    /// A cancel reaches the in-flight download of its own message only, and a
    /// download that settles leaves the in-flight list, so a later cancel finds
    /// nothing to set.
    #[test]
    fn a_cancel_sets_only_its_own_download_and_settling_forgets_it() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State::new(Config::default(), tx);
        let mut app = App::new();
        let ours = Arc::new(AtomicBool::new(false));
        let other = Arc::new(AtomicBool::new(false));
        state.media_cancel.push(MediaCancel {
            chat_id: CHAT,
            message_id: 9,
            flag: Arc::clone(&ours),
        });
        state.media_cancel.push(MediaCancel {
            chat_id: CHAT,
            message_id: 10,
            flag: Arc::clone(&other),
        });

        state.cancel_media(CHAT, 9);
        assert!(ours.load(Ordering::Relaxed));
        assert!(
            !other.load(Ordering::Relaxed),
            "a neighbour is not cancelled"
        );

        apply(
            &mut app,
            &mut state,
            Event::MediaCancelled {
                chat_id: CHAT,
                message_id: 9,
            },
        );
        assert_eq!(
            state.media_cancel.len(),
            1,
            "the settled download is forgotten"
        );
        assert!(
            state.take_media().is_empty(),
            "a cancelled download queues no viewer"
        );
    }

    /// Signing out cancels every download in flight: each flag is set, and the
    /// entries stay until their settle event removes them.
    #[test]
    fn signing_out_cancels_every_media_download_in_flight() {
        let mut app = App::new();
        let mut state = State::default();
        let flags: Vec<Arc<AtomicBool>> = (0..2)
            .map(|message_id| {
                let flag = Arc::new(AtomicBool::new(false));
                state.media_cancel.push(MediaCancel {
                    chat_id: CHAT,
                    message_id,
                    flag: Arc::clone(&flag),
                });
                flag
            })
            .collect();

        apply_logged_out(&mut app, &mut state, Ok(()));

        assert!(
            flags.iter().all(|flag| flag.load(Ordering::Relaxed)),
            "every in-flight download is cancelled by the sign-out"
        );
        assert_eq!(
            state.media_cancel.len(),
            2,
            "the entries leave with their settle events, not the sign-out"
        );
    }

    /// A saved file is queued for the viewer and said on the status line; a
    /// failed one is said there and queues nothing.
    #[test]
    fn apply_queues_a_saved_media_path_and_flashes_a_failure() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = State::new(Config::default(), tx);
        let mut app = App::new();

        apply(
            &mut app,
            &mut state,
            Event::MediaSaved {
                chat_id: CHAT,
                message_id: 9,
                path: PathBuf::from("televim-1-7-9.mp4"),
            },
        );
        assert!(
            app.ui.status.contains("televim-1-7-9.mp4"),
            "{}",
            app.ui.status
        );
        assert_eq!(
            state.media.take_pending(),
            vec![PathBuf::from("televim-1-7-9.mp4")]
        );

        apply(
            &mut app,
            &mut state,
            Event::MediaFailed {
                chat_id: CHAT,
                message_id: 9,
                reason: "media is over the 16 MiB limit; nothing was saved".to_owned(),
            },
        );
        assert_eq!(
            app.ui.status,
            "media is over the 16 MiB limit; nothing was saved"
        );
        assert!(state.media.take_pending().is_empty());
    }
}
