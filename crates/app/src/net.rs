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
//! * **Brings the client up** — build it from the configuration, sign in when
//!   the stored session is not enough, fetch the chat list, take the update
//!   feed. Every failure is reported to the screen rather than raised, because
//!   a client that cannot connect is a state the reader is in, not a crash.
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

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use domain::chat::Chat;
use domain::message::Message;
use domain::search::SEARCH_MATCHES;
use domain::updates::UpdateEvent;
use proto::{HistoryCursor, ProtoClient, ProtoError, SearchResults, UpdateStream};
use telegram_framework::{
    Client, ClientBuilder, FileStore, FrameworkError, KeyringStore, RequestError, SessionStore,
    SignInResult,
};
use tokio::sync::mpsc::UnboundedSender;
use tui::app::{Action, App, FetchDirection, Jump};

use crate::config::Config;
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

/// Something that happened away from the keyboard.
pub enum Event {
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
        account: Result<domain::account::Account, String>,

        /// Where the session is kept, as the panel says it.
        session_store: tui::SessionStore,
    },

    /// The client could not be brought up.
    ///
    /// Carries the reason, and the chain behind it: the screen has one line to
    /// say what went wrong, and the most specific answer is the useful one.
    Offline(anyhow::Error),

    /// A page came back, or the fetch that asked for it failed.
    History {
        /// What was asked for.
        direction: FetchDirection,

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
}

/// What the loop knows about the network between events.
#[derive(Default)]
pub struct State {
    /// The client, once it is up.
    client: Option<Arc<ProtoClient>>,

    /// The cursor for the conversation on show, and how long it has to be left
    /// alone.
    history: History,
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
        if let Err(reason) = bring_up(&cfg, &tx).await {
            let _ = tx.send(AppEvent::Net(Event::Offline(reason)));
        }
    });
}

/// Builds the client, signs in if it has to, and fetches the chat list.
async fn bring_up(cfg: &Config, tx: &UnboundedSender<AppEvent>) -> Result<()> {
    let (api_id, api_hash) = cfg.credentials().context(
        "no telegram application credentials configured; set TELEVIM_API_ID and TELEVIM_API_HASH",
    )?;

    let client = ClientBuilder::new(api_id, api_hash)
        .session_store(session_store(cfg))
        .build()
        .await
        .context("building the client")?;

    if !client
        .is_authorized()
        .await
        .context("checking whether the stored session is signed in")?
    {
        log_in(&client, cfg).await?;
    }

    let client = Arc::new(ProtoClient::new(client));
    let chats = client
        .fetch_private_chats()
        .await
        .context("fetching the chat list")?;

    // Read once, here, and not when the panel is opened: the profile does not
    // change under a reader, and a panel that blanked and refilled every time
    // would be a panel they could not trust. A failure is carried rather than
    // raised, because the conversations are here either way and the screen is
    // usable without a profile — the panel says why it has none.
    let account = match client.fetch_account().await {
        Ok(account) => Ok(account),
        Err(error) => {
            tracing::warn!(%error, "the account's own profile could not be read");
            Err(format!("{error:#}"))
        }
    };

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
        session_store: session_description(cfg),
    }));
    tokio::spawn(pump(updates, tx.clone()));

    Ok(())
}

/// Where the session is kept, in the words the profile panel says it in.
///
/// The decision, and the only one: [`session_store`] builds the store out of
/// what this returns, so the panel cannot describe a store the session is not
/// in. Reading the same field twice would leave that to a test, and a test
/// cannot see a field read two ways.
fn session_description(cfg: &Config) -> tui::SessionStore {
    match &cfg.session_path {
        Some(path) => tui::SessionStore::PlaintextFile(path.clone()),
        None => tui::SessionStore::Keyring,
    }
}

/// The store itself: the file the configuration names, or the machine's own
/// credential store.
fn session_store(cfg: &Config) -> Box<dyn SessionStore> {
    match session_description(cfg) {
        tui::SessionStore::Keyring => Box::new(KeyringStore::default()),
        tui::SessionStore::PlaintextFile(path) => Box::new(FileStore::new(path)),
    }
}

/// Signs in with the credentials the configuration carries.
///
/// A stored session is the ordinary case and this is not reached; when it is,
/// the phone number and the code Telegram sent have to be in the configuration,
/// because the screen has nowhere to ask for them yet. Once the session has been
/// written, neither is read again.
async fn log_in(client: &Client, cfg: &Config) -> Result<()> {
    let (phone, code) = cfg.login_credentials().context(
        "the stored session is not signed in; set TELEVIM_PHONE and TELEVIM_CODE to sign in",
    )?;

    let token = client
        .request_login_code(phone)
        .await
        .context("requesting a login code")?;

    match client
        .sign_in(&token, code)
        .await
        .context("submitting the login code")?
    {
        SignInResult::Success => Ok(()),

        SignInResult::PasswordRequired(password_token) => {
            let hint = password_token
                .hint()
                .map_or_else(String::new, |hint| format!(" (hint: {hint})"));
            let password = cfg.password.as_deref().with_context(|| {
                format!("this account has two-factor authentication; set TELEVIM_PASSWORD{hint}")
            })?;

            client
                .check_password(password_token, password)
                .await
                .context("submitting the two-factor password")
        }

        // `SignInResult` is `#[non_exhaustive]`, and a step this build does not
        // know is reported rather than guessed at.
        _ => anyhow::bail!("telegram answered with a sign-in step this build does not know"),
    }
}

/// Forwards the feed to the loop for as long as it lasts.
///
/// The feed ends when the client shuts down, and `None` is that: the task stops
/// rather than retrying. A failure while resolving a gap in the sequence is
/// logged and carried on with — the feed stays usable, and it resumes where it
/// left off.
async fn pump(mut updates: UpdateStream, tx: UnboundedSender<AppEvent>) {
    while let Some(result) = updates.next().await {
        match result {
            Ok(event) => {
                if tx.send(AppEvent::Net(Event::Update(event))).is_err() {
                    // The loop is gone, so there is nothing left to tell.
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "the update feed reported a failure it can recover from");
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

    // Nothing is open, so there is no conversation for a cursor to describe —
    // nor a jump to be waiting on, because closing a conversation forgets one.
    if app.conversation.window.chat_id == 0 {
        state.history.cursor = None;
        state.history.jump = None;
    }

    // A conversation the reader has highlighted and stopped on. Taken before the
    // client is looked up, because opening a conversation is what asks for its
    // newest page, and the two have to happen on the same pass or the window is
    // replaced and then asked about a quarter of a second later.
    if let Some(index) = app.take_pending_chat(Instant::now()) {
        app.select_chat(index);
        state.history.cursor = None;
    }

    let Some(client) = state.client.clone() else {
        return;
    };

    // The reader's outbound requests are drained before `wanted`, which returns
    // early while a history page backs off and while nothing is open: a send
    // must not be held up by either. An action is left in place while there is
    // no client, because a message typed offline must not be thrown away.
    while let Some(action) = app.take_action() {
        request_action(&client, action, tx);
    }

    match wanted(app, state.history, Instant::now()) {
        Wanted::Nothing => {}

        Wanted::Latest(peer_id) => {
            let cursor = HistoryCursor::new(peer_id);
            // Recorded before the request rather than after it, so that the
            // next pass — which is a quarter of a second away — does not ask for
            // the same page again while this one is still on its way.
            state.history.cursor = Some(cursor);
            app.begin_fetch(FetchDirection::Latest);
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

/// Decides which page to ask for, from what the screen and the cursor say.
fn wanted(app: &App, history: History, now: Instant) -> Wanted {
    let open = app.conversation.window.chat_id;
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

        let result = match direction {
            FetchDirection::Latest => client.fetch_latest(cursor.peer_id(), PAGE).await,
            FetchDirection::Older => client.fetch_older(&mut cursor, PAGE).await,
            FetchDirection::Newer => client.fetch_newer(&mut cursor, PAGE).await,
        };

        // The loop may have gone; there is then nothing to report the page to.
        let _ = tx.send(AppEvent::Net(Event::History {
            direction,
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

/// Performs the operation the reader asked for, and hands the answer back.
///
/// The same shape as the two above, and for the same reason: a round trip in the
/// event loop would stop the reader's keystrokes from being read while it runs.
fn request_action(client: &Arc<ProtoClient>, action: Action, tx: &UnboundedSender<AppEvent>) {
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
        }
    });
}

/// Folds something that arrived from the network into the screen's state.
pub fn apply(app: &mut App, state: &mut State, event: Event) {
    match event {
        Event::Ready {
            client,
            chats,
            account,
            session_store,
        } => {
            open_first_chat(app, chats);
            app.set_session_store(session_store);
            app.set_account(account);
            state.client = Some(client);
        }

        // A contact's profile, and nothing else: no status and no cursor. The
        // card is already on show and already says it is waiting, so the answer
        // fills it in — and an answer for a card the reader has left is dropped by
        // `set_contact`, which is the only place that knows which card is on show.
        Event::Contact { peer_id, result } => {
            app.set_contact(peer_id, result.map_err(|error| format!("{error:#}")));
        }

        // The panel is told too, not only the status line. A status is a flash:
        // it is gone within seconds, and a reader who opens the profile a minute
        // later must still be told why it is empty rather than shown a blank
        // panel they cannot tell from a broken one.
        Event::Offline(reason) => {
            let reason = format!("{reason:#}");
            app.set_account(Err(reason.clone()));
            app.status = format!("offline: {reason}");
        }

        Event::Update(event) => {
            // Whether it moved anything is not acted on: the loop redraws on
            // every pass, so the report has no decision to feed here.
            let _ = app.apply_update(&event);
        }

        Event::Searched {
            chat_id,
            query,
            result,
        } => apply_searched(app, chat_id, &query, result),

        Event::Sent {
            chat_id,
            temp_id,
            result,
        } => apply_sent(app, chat_id, temp_id, result),

        Event::Edited {
            chat_id,
            message_id,
            result,
        } => apply_edited(app, chat_id, message_id, result),

        Event::Deleted {
            chat_id,
            message_ids,
            result,
        } => apply_deleted(app, chat_id, &message_ids, result),

        Event::History {
            direction,
            mut cursor,
            result,
        } => {
            // However it ended, the direction is open again: a fetch that failed
            // must not close a conversation for good.
            app.end_fetch(direction);

            // A page was fetched for one conversation, and the reader can open
            // another while it is in flight. The window refuses a page that is
            // not its own; the cursor has to be refused here too, or it would
            // start describing a conversation that is no longer on screen.
            if state.history.cursor.map(|open| open.peer_id()) != Some(cursor.peer_id()) {
                return;
            }

            match result {
                Ok(page) => {
                    if direction == FetchDirection::Latest {
                        cursor.reset_to(&page);
                    }
                    apply_page(app, direction, page);
                    settle(app, cursor);
                    state.history.cursor = Some(cursor);
                }

                Err(error) => {
                    app.status = format!("history: {error}");
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

        Event::Jumped {
            jump,
            mut cursor,
            result,
        } => {
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
                    app.status = format!("history: {error}");
                }
            }
        }
    }
}

/// Folds a send's answer into the conversation it was for.
///
/// The in-flight gate is freed before the conversation is checked: a result for
/// a conversation the reader has left still has to free the send key, or it
/// would stay wedged for every conversation they open after, with no visible
/// symptom to explain it.
fn apply_sent(app: &mut App, chat_id: i64, temp_id: i64, result: Result<Message, ProtoError>) {
    app.end_send(temp_id);

    if app.conversation.window.chat_id != chat_id {
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
    if app.conversation.window.chat_id != chat_id {
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

            if app.conversation.window.chat_id == chat_id {
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

/// Puts a fetched chat list on screen, and the reader into it.
///
/// The list is newest first, so the first entry is the conversation that last
/// had something to say — which is the one opening the client should land in.
fn open_first_chat(app: &mut App, chats: Vec<Chat>) {
    let count = chats.len();
    app.set_chats(chats);
    app.status = format!("{count} conversation(s)");

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
    use tui::app::CHAT_SWITCH_DELAY;

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
            })
            .collect()
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

    // ---- what a fetched list means --------------------------------------

    #[test]
    fn a_fetched_list_puts_the_reader_in_the_newest_conversation() {
        let mut app = App::new();

        open_first_chat(&mut app, vec![chat(CHAT), chat(CHAT + 1)]);

        assert_eq!(app.chats().len(), 2);
        assert_eq!(app.selected_chat, 0);
        assert_eq!(
            app.conversation.window.chat_id, CHAT,
            "the list is newest first, so the first entry is the one to open"
        );
        assert!(app.status.contains('2'), "got {:?}", app.status);
    }

    #[test]
    fn a_fetched_list_with_nobody_in_it_opens_nothing() {
        let mut app = App::new();

        open_first_chat(&mut app, Vec::new());

        assert!(app.chats().is_empty());
        assert_eq!(app.conversation.window.chat_id, 0);
    }

    /// The highlight moves on the keystroke, but the conversation it names is
    /// opened by the driver once the reader has stopped — and opening it has to
    /// forget the cursor, or the new conversation would be described by the old
    /// one's history and its first page would never be asked for.
    #[test]
    fn the_driver_opens_the_conversation_the_reader_stopped_on() {
        let mut app = app_with_unread_out_of_reach(2);
        let mut state = State {
            client: None,
            history: opened(CHAT),
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Off the chat list, onto the second conversation.
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));

        drive(&mut app, &mut state, &tx);
        assert_eq!(
            app.conversation.window.chat_id, CHAT,
            "the highlight has moved but the reader has not stopped"
        );

        std::thread::sleep(CHAT_SWITCH_DELAY);
        drive(&mut app, &mut state, &tx);

        assert_eq!(app.conversation.window.chat_id, CHAT + 1);
        assert!(
            state.history.cursor.is_none(),
            "the old cursor is forgotten"
        );

        drive(&mut app, &mut state, &tx);
        assert_eq!(
            wanted(&app, state.history, Instant::now()),
            Wanted::Latest(CHAT + 1),
            "so the new conversation's first page is asked for on the next pass"
        );
    }

    // ---- what arrives ---------------------------------------------------

    #[test]
    fn an_older_page_lands_in_front_of_the_window() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let mut state = State {
            client: None,
            history: opened(CHAT),
        };
        app.begin_fetch(FetchDirection::Older);

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Older,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, -1..=0)),
            },
        );

        assert_eq!(app.conversation.window.len(), 5);
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
            client: None,
            history: opened(CHAT),
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

        assert_eq!(app.conversation.window.len(), 5);
        assert_eq!(
            app.conversation
                .window
                .get(app.vim.cursor())
                .map(|message| message.id),
            Some(19),
            "the reader is on the message the jump was for"
        );
        assert_eq!(app.pending_jump(), None, "and the jump is over");
        assert!(
            !app.conversation.window.exhausted_older && !app.conversation.window.exhausted_newer,
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
            client: None,
            history: opened(CHAT),
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

        assert!(app.status.contains("history:"), "got {:?}", app.status);
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
            app.conversation.window.len(),
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
            client: None,
            history: opened(CHAT),
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

    /// A jump the reader overrode — `G`, take me to the end instead — leaves the
    /// cursor describing what is still on screen.
    #[test]
    fn an_abandoned_jump_leaves_the_cursor_alone() {
        let mut app = app_with_unread_out_of_reach(2);
        ask_to_jump(&mut app);
        let jump = app
            .pending_jump()
            .expect("the reader asked to be taken to the unread messages");

        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert_eq!(app.pending_jump(), None);

        let mut state = State {
            client: None,
            history: opened(CHAT),
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
            app.conversation.window.len(),
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
            client: None,
            history: opened(CHAT + 1),
        };

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Latest,
                cursor: HistoryCursor::new(CHAT),
                result: Ok(messages(CHAT, 1..=2)),
            },
        );

        assert_eq!(
            app.conversation.window.len(),
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
            client: None,
            history: opened(CHAT),
        };

        apply(
            &mut app,
            &mut state,
            Event::History {
                direction: FetchDirection::Latest,
                cursor: HistoryCursor::new(CHAT),
                result: Err(ProtoError::Framework(FrameworkError::UnknownPeer(CHAT))),
            },
        );

        assert!(app.status.contains("history:"), "got {:?}", app.status);
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
            app.status.starts_with("offline:") && app.status.contains("credentials"),
            "got {:?}",
            app.status
        );
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

        assert_eq!(app.conversation.window.newest_id(), Some(4));
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
            app.status, "delete: 200 of 3 went through, the rest did not",
            "got {:?}",
            app.status
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
            app.status, "delete: network error: reset",
            "got {:?}",
            app.status
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

        assert_eq!(app.status, "televim", "got {:?}", app.status);
    }

    // ---- what a send answers --------------------------------------------
    /// A result for a conversation the reader has left must still free the send
    /// key: the gate is released before the conversation is checked, because a
    /// return that happened first would wedge it with no visible symptom.
    #[test]
    fn a_send_result_frees_the_key_even_for_a_conversation_that_was_left() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.queue_send("hi", None);
        app.begin_send(temp_id);

        // The reader opens another conversation while the send is on its way.
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);
        assert_eq!(app.sending, Some(temp_id), "the send is still in flight");

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
            app.sending.is_none(),
            "a dropped result must not leave the send key wedged"
        );
    }

    /// The other half: the result changes nothing in the conversation the reader
    /// opened, because it was not for that one.
    #[test]
    fn a_send_result_for_a_conversation_that_is_no_longer_open_changes_nothing() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.queue_send("hi", None);
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
            app.conversation.window.is_empty(),
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
        let temp_id = app.conversation.queue_send("first", None);
        app.begin_send(temp_id);
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);

        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        for character in "second".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            app.conversation.window.is_empty(),
            "the in-flight send holds the key, so the new conversation is not given the id"
        );
        assert!(
            app.status.contains("already on its way"),
            "and the refusal is visible: {:?}",
            app.status
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
        let old_temp = app.conversation.queue_send("first", None);
        app.begin_send(old_temp);
        app.set_chats(vec![chat(CHAT), chat(CHAT + 1)]);
        app.select_chat(1);

        // Reaching past the gate is deliberate: without it, this is the
        // collision the `chat_id` check has to survive.
        let new_temp = app.conversation.queue_send("second", None);
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
                .message(new_temp)
                .map(|message| message.status),
            Some(MessageStatus::Sending),
            "a result for another conversation must not confirm this one's message"
        );
        assert!(
            app.conversation.window.newest_id().is_none(),
            "and the real message did not land here"
        );
    }

    #[test]
    fn a_send_result_replaces_the_placeholder_the_reader_was_shown() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.queue_send("hi", None);
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
            app.conversation.message(temp_id).is_none(),
            "the placeholder left"
        );
        assert_eq!(app.conversation.window.newest_id(), Some(4));
        assert!(app.sending.is_none(), "the key is free again");
    }

    #[test]
    fn a_failed_send_keeps_the_message_and_says_why() {
        let mut app = app_with_a_conversation(CHAT, 3);
        let temp_id = app.conversation.queue_send("hi", None);
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
                .message(temp_id)
                .map(|message| message.status),
            Some(MessageStatus::Failed),
            "the reader's message stays, marked as failed"
        );
        assert!(
            app.conversation.failure(temp_id).is_some(),
            "and it carries a reason"
        );
        assert!(app.sending.is_none(), "the key is free to try again");
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
            client: None,
            history: opened(CHAT),
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
}

#[cfg(test)]
mod session_store_tests {
    use super::*;

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
            tui::SessionStore::PlaintextFile("/tmp/televim.session".into()),
            "a file is named, and named as the plaintext thing it is"
        );
    }
}
