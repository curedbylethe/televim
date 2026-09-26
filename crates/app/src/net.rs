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
use domain::updates::UpdateEvent;
use proto::{HistoryCursor, ProtoClient, ProtoError, UpdateStream};
use telegram_framework::{
    Client, ClientBuilder, FileStore, FrameworkError, KeyringStore, RequestError, SessionStore,
    SignInResult,
};
use tokio::sync::mpsc::UnboundedSender;
use tui::app::{App, FetchDirection, Jump};

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
    /// The client is up and the chat list has been fetched.
    Ready {
        /// The wrapper the fetches go through.
        client: Arc<ProtoClient>,

        /// The conversations, newest first.
        chats: Vec<Chat>,
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

    /// An update arrived for a conversation televim displays.
    Update(UpdateEvent),
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

    // The list is fetched before the feed is taken. Resolving what arrived while
    // the client was offline reads peers back out of the session, and iterating
    // the dialogs is what puts them there.
    let updates = client
        .subscribe_updates()
        .context("taking the update feed")?;

    let _ = tx.send(AppEvent::Net(Event::Ready { client, chats }));
    tokio::spawn(pump(updates, tx.clone()));

    Ok(())
}

/// Where the session is kept: the file the configuration names, or the machine's
/// own credential store.
fn session_store(cfg: &Config) -> Box<dyn SessionStore> {
    match &cfg.session_path {
        Some(path) => Box::new(FileStore::new(path.clone())),
        None => Box::new(KeyringStore::default()),
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
}

/// Asks for whatever page the screen is about to need.
///
/// Called after every event and every tick, so it has to be cheap and it has to
/// be idempotent: what it decides is [`wanted`]'s, and what it does is start one
/// fetch and record that it did.
pub fn drive(app: &mut App, state: &mut State, tx: &UnboundedSender<AppEvent>) {
    // Nothing is open, so there is no conversation for a cursor to describe —
    // nor a jump to be waiting on, because closing a conversation forgets one.
    if app.conversation.window.chat_id == 0 {
        state.history.cursor = None;
        state.history.jump = None;
    }

    let Some(client) = state.client.clone() else {
        return;
    };

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

/// Folds something that arrived from the network into the screen's state.
pub fn apply(app: &mut App, state: &mut State, event: Event) {
    match event {
        Event::Ready { client, chats } => {
            open_first_chat(app, chats);
            state.client = Some(client);
        }

        Event::Offline(reason) => app.status = format!("offline: {reason:#}"),

        Event::Update(event) => {
            // Whether it moved anything is not acted on: the loop redraws on
            // every pass, so the report has no decision to feed here.
            let _ = app.apply_update(&event);
        }

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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::chat::ChatKind;
    use domain::message::MessageStatus;

    use super::*;

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
}
