//! End-to-end tests for the client the application assembles.
//!
//! `app` is the composition root, and it is the only crate that may hold both a
//! `telegram_framework::Client` and the `proto` wrapper around it. That makes it
//! the only place the two halves can be exercised together, against a real
//! datacenter, rather than one at a time against fixtures.
//!
//! They talk to Telegram for real, so they are opt-in: set `TELEVIM_TEST_DC=1`
//! and the credentials below, and they will run. Without it every test reports
//! that it was skipped and returns, which keeps CI — where there is no account
//! to log into — green.
//!
//! | Variable                | Required | Meaning                                                |
//! | :---------------------- | :------- | :----------------------------------------------------- |
//! | `TELEVIM_TEST_DC`       | yes      | Opt in. Must be `1`.                                   |
//! | `TELEVIM_API_ID`        | yes      | Application identifier from <https://my.telegram.org>. |
//! | `TELEVIM_API_HASH`      | yes      | Application hash matching `TELEVIM_API_ID`.            |
//! | `TELEVIM_TEST_PHONE`    | login    | Phone number of the test account, in `+…` form.        |
//! | `TELEVIM_TEST_CODE`     | login    | The login code Telegram delivers for that account.     |
//! | `TELEVIM_TEST_PASSWORD` | 2FA only | Two-factor password, for an account that has one.      |
//!
//! Each test requests its own login code, and Telegram throttles that hard, so
//! run them sparingly.
//!
//! # What one account cannot check
//!
//! Three things this seam wants proven cannot be provoked with the single
//! account these tests have. They are recorded here rather than left as a silent
//! hole:
//!
//! - **An update arriving for real.** Nothing sends to the test account while a
//!   run is in progress, so the feed is usually silent and the drain below
//!   checks zero updates. Provoking one needs a second account, and a send API
//!   the framework does not expose yet.
//! - **The offline gap.** `catch_up` replays what arrived while the client was
//!   not running, which again needs something to send to it in the meantime.
//! - **A conversation with a known amount of history.** The history test pages
//!   through whichever conversation the account has, so it cannot say in advance
//!   how many pages it will walk or whether it will reach either end of one.
//!   What it can say is what came back, and it prints that. A conversation
//!   shorter than the page budget is walked to both of its ends, which is the
//!   case that proves a short page settles a direction; a longer one is walked
//!   back a few pages and then forwards again, which is the case that proves the
//!   two directions agree about the messages between them.
//! - **Which message a reader stopped at.** Nothing in the wire format says it,
//!   so the first unread is arithmetic over the count and the newest identifier —
//!   approximate wherever deletions left gaps in the numbering. The history test
//!   fetches a page around the estimate and prints whether it was a message at
//!   all; the unit tests carry the cases that can be settled exactly.
//!
//! Each is covered as far as one account allows. The fetched list's ordering and
//! the history's are asserted directly, which is deterministic, and the counts —
//! of updates checked, of updates the framework discarded, of pages walked — are
//! printed, so a run that proved little says so instead of looking like a pass.

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use domain::chat::Chat;
use domain::history::unread_target;
use domain::message::Message;
use domain::updates::{ChatList, UpdateEvent};
use proto::{HistoryCursor, ProtoClient, ProtoError, UpdateStream};
use telegram_framework::session::FileStore;
use telegram_framework::{Client, ClientBuilder, FrameworkError, SignInResult};

/// How long to wait for the feed before accepting that nothing is arriving.
///
/// The feed is quiet whenever nobody is sending to the account, and it does not
/// say so: it simply produces no event. A test therefore has to bound its wait
/// rather than await forever.
const FEED_WINDOW: Duration = Duration::from_secs(10);

/// How many messages one page asks for.
///
/// The largest page Telegram will return, so a page shorter than this is the
/// conversation running out rather than the request being small — which is the
/// whole of what settles a direction.
const PAGE: usize = 100;

/// How many pages of one conversation a run walks through.
///
/// A busy account could have thousands, and what is being checked is the paging
/// arithmetic rather than the size of the account's history. Four pages show a
/// chain; a conversation shorter than that is walked to its end, which is the
/// case that proves a short page settles its direction.
const PAGE_BUDGET: usize = 4;

/// Credentials and configuration for the opt-in tests.
struct TestDc {
    api_id: i32,
    api_hash: String,
    phone: Option<String>,
    code: Option<String>,
    password: Option<String>,
}

impl TestDc {
    /// Reads the configuration, or returns `None` when the tests are not
    /// enabled.
    fn from_env() -> Option<Self> {
        if env::var("TELEVIM_TEST_DC").as_deref() != Ok("1") {
            return None;
        }

        Some(Self {
            api_id: env::var("TELEVIM_API_ID").ok()?.parse().ok()?,
            api_hash: env::var("TELEVIM_API_HASH").ok()?,
            phone: env::var("TELEVIM_TEST_PHONE").ok(),
            code: env::var("TELEVIM_TEST_CODE").ok(),
            password: env::var("TELEVIM_TEST_PASSWORD").ok(),
        })
    }

    /// The phone number and login code, or `None` when the test needs to skip.
    fn login_credentials(&self) -> Option<(&str, &str)> {
        Some((self.phone.as_deref()?, self.code.as_deref()?))
    }
}

/// Builds a client backed by the session file at `path`.
async fn build_client(dc: &TestDc, path: &Path) -> Client {
    ClientBuilder::new(dc.api_id, dc.api_hash.clone())
        .session_store(Box::new(FileStore::new(path)))
        .build()
        .await
        .expect("the client builds")
}

/// A scratch session path, so the tests never touch the developer's own.
fn session_path() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary directory is created");
    let path = dir.path().join("session.json");
    (dir, path)
}

/// Logs in, taking whichever branch the account needs.
///
/// Returns `false` when Telegram answered with a step this build does not know,
/// so the caller can skip rather than fail on a step it was never written for.
async fn log_in(client: &Client, dc: &TestDc) -> bool {
    let (phone, code) = dc
        .login_credentials()
        .expect("TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE are set");

    let token = client
        .request_login_code(phone)
        .await
        .expect("telegram sends a login code");

    match client
        .sign_in(&token, code)
        .await
        .expect("the login code is accepted")
    {
        SignInResult::Success => true,
        SignInResult::PasswordRequired(password_token) => {
            let password = dc
                .password
                .as_deref()
                .expect("this account has two-factor authentication; set TELEVIM_TEST_PASSWORD");
            client
                .check_password(password_token, password)
                .await
                .expect("the two-factor password is accepted");
            true
        }
        _ => {
            eprintln!("skipped: telegram answered with a sign-in step this build does not know");
            false
        }
    }
}

/// What one turn of the feed produced.
enum FeedStep {
    /// An update arrived.
    Update(UpdateEvent),

    /// The feed reported a failure it can carry on from.
    Failed(ProtoError),

    /// The window closed, or the feed ended because the client is shutting
    /// down. Neither is a failure.
    Quiet,
}

/// Awaits the next turn of the feed, or gives up once `deadline` has passed.
///
/// A failure is reported rather than raised. Resolving a gap in the sequence is
/// a request like any other, so a transient error says nothing about whether the
/// two layers name conversations the same way — which is what this file is
/// about — and the subscription stays usable afterwards. Counting it and
/// carrying on keeps a flaky network from failing a test it has no bearing on.
async fn next_before(updates: &mut UpdateStream, deadline: tokio::time::Instant) -> FeedStep {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return FeedStep::Quiet;
    }

    match tokio::time::timeout(remaining, updates.next()).await {
        Ok(Some(Ok(event))) => FeedStep::Update(event),
        Ok(Some(Err(error))) => FeedStep::Failed(error),
        Ok(None) | Err(_) => FeedStep::Quiet,
    }
}

/// Asserts that a fetched list is ordered the way a chat list shows it.
///
/// This is the one property of the fetch that is worth checking without a second
/// account: the framework sorts the dialogs and the sort has to survive the
/// translation into `domain` types. `None` sorts below every timestamp, so a
/// conversation with no messages at all belongs at the end.
fn assert_newest_first(chats: &[Chat]) {
    for pair in chats.windows(2) {
        assert!(
            pair[0].last_timestamp >= pair[1].last_timestamp,
            "the fetch must return conversations newest first, but {:?} came before {:?}",
            pair[0].last_timestamp,
            pair[1].last_timestamp
        );
    }
}

/// Logs in for real, and proves the client that comes out of it is the one the
/// application would have: built from the store, and able to fetch.
///
/// The login flow itself is the framework's, and it has its own tests. What is
/// checked here is the seam the application sits on — that a session written by
/// a login is enough to rebuild an authorised client, that such a client can be
/// handed to `ProtoClient` and produce a chat list, and that the list arrives in
/// the order a chat list is rendered in.
#[tokio::test]
async fn login_round_trip_yields_a_client_that_can_fetch_the_chat_list() {
    let Some(dc) = TestDc::from_env() else {
        eprintln!("skipped: set TELEVIM_TEST_DC=1 to run against a real datacenter");
        return;
    };
    if dc.login_credentials().is_none() {
        eprintln!("skipped: set TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE");
        return;
    }

    let (_dir, path) = session_path();

    let client = build_client(&dc, &path).await;
    if !log_in(&client, &dc).await {
        return;
    }
    assert!(
        client
            .is_authorized()
            .await
            .expect("authorization is checked"),
        "the client should be authorized after logging in"
    );

    // The session has to survive a restart on its own, because that is how the
    // application starts: it builds a client from whatever the store holds, and
    // only logs in when that is not enough.
    client.persist_session().expect("the session is persisted");
    drop(client);
    assert!(path.exists(), "the session file should have been written");

    let resumed = build_client(&dc, &path).await;
    assert!(
        resumed
            .is_authorized()
            .await
            .expect("authorization is checked"),
        "a client rebuilt from the stored session should be authorized"
    );

    let chats = ProtoClient::new(resumed)
        .fetch_private_chats()
        .await
        .expect("the chat list is fetched");

    assert!(
        chats.iter().all(Chat::is_private),
        "the fetch must return only the conversations televim displays"
    );
    assert_newest_first(&chats);
}

/// Fetches the chat list, takes the feed, and checks that the two agree.
///
/// The assertion that carries this test is the agreement itself: the feed is
/// filtered to private conversations inside the framework, and the list is
/// filtered inside `proto`, and the two only line up if both layers name a
/// conversation by the same identifier. `domain` cannot check that — an event
/// for a conversation it does not hold is indistinguishable from one that
/// changed nothing — so an end-to-end run is the only place the mistake would
/// show, and it would show as every event being silently dropped.
///
/// Nothing is sent to the test account while this runs, so the feed usually
/// yields nothing and the loop ends on its own. A recoverable failure while
/// resolving a gap is counted rather than raised — it says nothing about the
/// identifiers — and both that count and the number of updates the framework
/// discarded are reported, so a run that checked nothing says so rather than
/// looking like a pass.
#[tokio::test]
async fn the_feed_names_only_conversations_the_fetch_returned() {
    let Some(dc) = TestDc::from_env() else {
        eprintln!("skipped: set TELEVIM_TEST_DC=1 to run against a real datacenter");
        return;
    };
    if dc.login_credentials().is_none() {
        eprintln!("skipped: set TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE");
        return;
    }

    let (_dir, path) = session_path();
    let client = build_client(&dc, &path).await;
    if !log_in(&client, &dc).await {
        return;
    }

    let proto = ProtoClient::new(client);

    // Fetch before subscribing: the feed replays whatever arrived while the
    // client was offline, and resolving that reads peers back out of the
    // session — the fetch is what puts them there.
    let chats = proto
        .fetch_private_chats()
        .await
        .expect("the chat list is fetched");
    assert!(
        chats.iter().all(Chat::is_private),
        "the feed can only be checked against a list of the conversations televim displays"
    );
    assert_newest_first(&chats);

    // Declared after `proto`, so it is dropped first: the subscription owns the
    // stream underneath and has to outlive nothing but its own polling.
    let mut updates = proto
        .subscribe_updates()
        .expect("the feed is taken for the first time");

    // An account has one ordered update sequence, so the feed can be taken
    // once. The refusal comes from the framework's own bookkeeping rather than
    // from Telegram, so it is checkable without waiting for anything.
    let error = proto
        .subscribe_updates()
        .expect_err("a second subscription must be refused");
    assert!(
        matches!(
            error,
            ProtoError::Framework(FrameworkError::UpdatesAlreadySubscribed)
        ),
        "expected the framework's refusal, got {error:?}"
    );

    let mut list = ChatList::with_chats(chats.clone());
    let deadline = tokio::time::Instant::now() + FEED_WINDOW;
    let mut seen = 0_usize;
    let mut failed = 0_usize;

    loop {
        match next_before(&mut updates, deadline).await {
            FeedStep::Update(event) => {
                // An arrival is the one event whose outcome is predictable in
                // advance: it changes the list exactly when it belongs to a
                // conversation the fetch returned. Every other event depends on
                // messages this client may never have held.
                let arrival = match &event {
                    UpdateEvent::NewMessage(message) => Some(message.chat_id),
                    _ => None,
                };

                let landed = list.apply_update(event);

                if let Some(chat_id) = arrival {
                    let held = chats.iter().any(|chat| chat.id == chat_id);
                    assert_eq!(
                        landed, held,
                        "an arrival for conversation {chat_id} must land exactly when the \
                         fetch returned that conversation; a mismatch means the two layers \
                         name conversations by different identifiers"
                    );
                }

                seen += 1;
            }
            FeedStep::Failed(error) => {
                eprintln!("the feed reported a failure it can recover from: {error}");
                failed += 1;
            }
            FeedStep::Quiet => break,
        }
    }

    eprintln!(
        "checked {seen} update(s) over {FEED_WINDOW:?} with {failed} recoverable failure(s); \
         the framework discarded {}",
        updates.dropped()
    );

    // The per-arrival check above is the sharp one, but it only fires when
    // something arrives. These look at the state the drain left behind, so the
    // run asserts something about the identifier space either way.
    assert_eq!(
        list.chats.len(),
        chats.len(),
        "the feed folds messages into conversations; it never adds or removes one"
    );
    assert!(
        list.messages
            .iter()
            .all(|message| chats.iter().any(|chat| chat.id == message.chat_id)),
        "every message in the window must belong to a conversation the fetch returned"
    );
}

/// Asserts that a page is in the order a window reads it.
///
/// Telegram answers newest first and `proto` turns the page around, so this is
/// the one property of the translation that a datacenter can confirm and a
/// fixture cannot: a fixture is written in whichever order the test author had
/// in mind.
fn assert_ascending(page: &[Message]) {
    for pair in page.windows(2) {
        assert!(
            pair[0].id < pair[1].id,
            "a page must come out oldest first, but {} came before {}",
            pair[0].id,
            pair[1].id
        );
    }
}

/// Fetches the chat list and picks a conversation there is history to page
/// through, or reports that the account has none.
///
/// A conversation whose dialog carried a preview is one with at least one
/// message in it, which is what makes it worth paging through. Picking by the
/// preview rather than by trying each conversation in turn is also the cheaper
/// answer: asking is a round trip.
async fn conversation_with_history(proto: &ProtoClient) -> Option<Chat> {
    // The list comes first, and not only because the feed wants it that way: a
    // peer cannot be addressed at all until this fetch has disclosed its access
    // hash.
    let chats = proto
        .fetch_private_chats()
        .await
        .expect("the chat list is fetched");

    let found = chats.iter().find(|chat| chat.last_message_id.is_some());

    if found.is_none() {
        eprintln!(
            "skipped: none of the {} conversation(s) has a message to page through",
            chats.len()
        );
    }

    found.cloned()
}

/// Which side of its anchor a page in a walk is on.
///
/// Named rather than a `bool`, because the two are the same assertion read
/// backwards and a boolean would leave every call site saying which it meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Counting towards the past: the page holds what is in front of the anchor.
    Older,

    /// Counting towards the present: the page holds what is behind it.
    Newer,
}

/// Asserts that a fetched page holds messages from one conversation, in the order
/// a window reads them, and only messages on the side of `anchor` it was counted
/// from.
///
/// The last of those is the one no fixture can settle. A shift with the wrong
/// sign does not fail: Telegram answers with an entirely ordinary page from the
/// other side of the anchor, and it is this assertion that says so.
fn assert_page_beside(page: &[Message], chat_id: i64, anchor: i64, side: Side) {
    assert_ascending(page);
    assert!(
        page.iter().all(|message| message.chat_id == chat_id),
        "a page is fetched for one conversation, and every message names that one"
    );

    let (rule, comparison) = match side {
        Side::Older => ("an older", "older than"),
        Side::Newer => ("a newer", "newer than"),
    };

    for message in page {
        let beside = match side {
            Side::Older => message.id < anchor,
            Side::Newer => message.id > anchor,
        };
        assert!(
            beside,
            "{rule} page must hold only messages {comparison} its anchor {anchor}, \
             but it held message {}",
            message.id
        );
    }
}

/// Asserts that a walk recovered every message an outward walk had already found.
///
/// Bounded by how far the recovering walk got: a shift wide enough to skip a
/// message leaves a hole *inside* the range it covered, and running out of pages
/// before reaching the far end is not the same thing as skipping one.
fn assert_nothing_skipped(seen: &[i64], already_seen: &[i64], start: i64) {
    let covered = seen.iter().copied().max().unwrap_or(start);

    for id in already_seen
        .iter()
        .copied()
        .filter(|id| *id > start && *id <= covered)
    {
        assert!(
            seen.contains(&id),
            "paging forwards skipped message {id}: every message the walk back found has \
             to come back, and this one was inside the range this walk covered"
        );
    }
}

/// What walking one conversation found.
///
/// The identifiers are kept rather than a count of them, because the two walks
/// are only interesting together: a message the walk towards the past saw and
/// the walk back did not is one that was skipped on the way, and that is the
/// mistake a wrong shift makes.
struct Chain {
    /// How many pages this walk fetched.
    pages: usize,

    /// Every message the walk saw, in the order it saw them.
    seen: Vec<i64>,

    /// Whether the walk reached the end of the conversation it was heading for.
    reached_the_end: bool,
}

/// Asserts that a walk saw each message once.
fn assert_no_repeats(seen: &[i64]) {
    let unique: HashSet<i64> = seen.iter().copied().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "two pages of one conversation must not return the same message"
    );
}

/// Pages backwards from `latest`, checking every page against the one before it.
///
/// The checks are all about the anchor: a page holds what is in front of it and
/// not the message it was counted from, the bound only ever moves away from
/// where the reader started, and no message comes back twice. Those are the
/// properties the window's prepend and its deduplication rest on.
///
/// Leaves `cursor` describing the whole region the walk covered, and hands back
/// the last page it fetched: that page is where a reader who had scrolled to the
/// top of the conversation would be, and it is the only place a walk back
/// towards the present can start from.
async fn walk_backwards(
    proto: &ProtoClient,
    chat_id: i64,
    cursor: &mut HistoryCursor,
    latest: &[Message],
) -> (Chain, Vec<Message>) {
    cursor.reset_to(latest);

    let mut seen: Vec<i64> = latest.iter().map(|message| message.id).collect();
    let mut pages = 0_usize;
    let mut reached_the_end = false;
    let mut last_page = latest.to_vec();

    while pages < PAGE_BUDGET {
        let anchor = cursor
            .oldest_loaded_id()
            .expect("the newest page left a bound to count from");

        let page = proto
            .fetch_older(cursor, PAGE)
            .await
            .expect("an older page is fetched");

        if page.is_empty() {
            assert!(
                cursor.exhausted_older(),
                "an empty page is the plainest end there is, so it must settle the direction"
            );
            reached_the_end = true;
            break;
        }

        assert_page_beside(&page, chat_id, anchor, Side::Older);

        seen.extend(page.iter().map(|message| message.id));

        let moved = cursor.oldest_loaded_id();
        assert!(
            moved < Some(anchor),
            "paging backwards has to move the oldest bound away from the reader: \
             {anchor} -> {moved:?}"
        );

        pages += 1;
        last_page = page;

        if cursor.exhausted_older() {
            reached_the_end = true;
            break;
        }
    }

    assert_no_repeats(&seen);

    (
        Chain {
            pages,
            seen,
            reached_the_end,
        },
        last_page,
    )
}

/// Pages forwards from where `cursor` stands, checking every page against the
/// one before it and against what a backward walk had already seen.
///
/// This is the direction nothing else in this file settles. It is the only
/// paging fetch that sends a negative `add_offset`, and a shift that is wrong
/// does not fail — Telegram answers with the messages in front of the anchor
/// instead, which is a page that looks entirely ordinary right up until a reader
/// scrolls down and finds they have gone the wrong way. The per-page checks
/// catch that, because such a page holds nothing behind the anchor.
///
/// Completeness is checked against the backward walk, by
/// [`assert_nothing_skipped`]: every message that walk saw beyond where this one
/// starts has to come back. A shift that is too wide skips messages, and a walk
/// that stopped short of them would otherwise look like a conversation that had
/// simply run out.
async fn walk_forwards(
    proto: &ProtoClient,
    chat_id: i64,
    cursor: &mut HistoryCursor,
    already_seen: &[i64],
) -> Chain {
    let start = cursor
        .newest_loaded_id()
        .expect("the page this walk starts from left a bound to count from");

    let mut seen: Vec<i64> = Vec::new();
    let mut pages = 0_usize;
    let mut reached_the_end = false;

    while pages < PAGE_BUDGET {
        let anchor = cursor
            .newest_loaded_id()
            .expect("every page this walk fetched left a bound to count from");

        let page = proto
            .fetch_newer(cursor, PAGE)
            .await
            .expect("a newer page is fetched");

        if page.is_empty() {
            assert!(
                cursor.exhausted_newer(),
                "an empty page is the plainest end there is, so it must settle the direction"
            );
            reached_the_end = true;
            break;
        }

        assert_page_beside(&page, chat_id, anchor, Side::Newer);

        seen.extend(page.iter().map(|message| message.id));

        let moved = cursor.newest_loaded_id();
        assert!(
            moved.is_some_and(|newest| newest > anchor),
            "paging forwards has to move the newest bound towards the reader: \
             {anchor} -> {moved:?}"
        );

        pages += 1;

        if cursor.exhausted_newer() {
            reached_the_end = true;
            break;
        }
    }

    assert_no_repeats(&seen);
    assert_nothing_skipped(&seen, already_seen, start);

    Chain {
        pages,
        seen,
        reached_the_end,
    }
}

/// Fetches a page centred on `target`, and checks that it is centred on it.
async fn page_around(proto: &ProtoClient, chat_id: i64, target: i64) -> Vec<Message> {
    let around = proto
        .fetch_around(chat_id, target, PAGE)
        .await
        .expect("a page around a message is fetched");

    assert_ascending(&around);
    assert!(
        around.iter().any(|message| message.id == target),
        "a page centred on message {target} has to contain it"
    );

    around
}

/// Fetches a page around where a conversation's unread messages are estimated to
/// start, and reports what came back.
///
/// This is the other thing a page *around* a message is for, and the estimate it
/// is given is the one the client works from. What a datacenter can confirm is
/// not that the page begins at the first unread — nothing says which message a
/// reader stopped at — but that the arithmetic produces a usable anchor:
/// Telegram accepts it, and a page of that conversation comes back like any
/// other. The unit tests carry the cases that can be settled exactly.
///
/// `None` when the conversation has nothing unread to jump to.
async fn page_around_the_unread(proto: &ProtoClient, chat: &Chat) -> Option<Vec<Message>> {
    let target = unread_target(chat.last_message_id, chat.unread_count)?;

    let around = proto
        .fetch_around(chat.id, target, PAGE)
        .await
        .expect("a page around the unread estimate is fetched");

    assert_ascending(&around);
    assert!(
        around.iter().all(|message| message.chat_id == chat.id),
        "a page is fetched for one conversation, and every message in it names that one"
    );

    eprintln!(
        "conversation {} has {} unread message(s), which puts the first of them at \
         {target}; a page around it held {} message(s), and {}",
        chat.id,
        chat.unread_count,
        around.len(),
        if around.iter().any(|message| message.id == target) {
            "the estimate is one of them"
        } else {
            "the estimate is not, so the numbering has gaps there"
        },
    );

    Some(around)
}

/// Pages through a conversation, in both directions, and checks what the pages
/// say about themselves.
///
/// This is where the paging arguments are proved against something that decides
/// what they mean. `offset_id` and `add_offset` are the one part of the history
/// work that no fixture can settle — the wire's meaning of an anchor and a shift
/// is Telegram's to define — so the assertions here are about what came back
/// rather than about what was sent: oldest first, nothing repeated, nothing
/// skipped, and an anchor that only ever moves away from where the reader
/// started.
///
/// The walk goes back and then comes forwards again, which is the only way to say
/// anything about the negative `add_offset` that a "newer" page is: a shift
/// with the wrong sign does not fail, it answers with the messages behind the
/// reader instead, and the only thing that notices is a walk checked against
/// what the outward one found.
///
/// One test rather than three, because each would need its own login and
/// Telegram throttles the code request hard. What it can prove is bounded by the
/// account: a conversation with no history is skipped, and one shorter than the
/// page budget is walked to its end. The counts are printed, so a run that
/// examined one message says so rather than looking like a pass.
#[tokio::test]
async fn history_pages_through_a_conversation_without_gaps_or_repeats() {
    let Some(dc) = TestDc::from_env() else {
        eprintln!("skipped: set TELEVIM_TEST_DC=1 to run against a real datacenter");
        return;
    };
    if dc.login_credentials().is_none() {
        eprintln!("skipped: set TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE");
        return;
    }

    let (_dir, path) = session_path();
    let client = build_client(&dc, &path).await;
    if !log_in(&client, &dc).await {
        return;
    }

    let proto = ProtoClient::new(client);

    let Some(chat) = conversation_with_history(&proto).await else {
        return;
    };

    let latest = proto
        .fetch_latest(chat.id, PAGE)
        .await
        .expect("the newest page is fetched");

    if latest.is_empty() {
        eprintln!(
            "skipped: conversation {} previews a message but returned none",
            chat.id
        );
        return;
    }

    assert_ascending(&latest);
    assert!(
        latest.iter().all(|message| message.chat_id == chat.id),
        "a page is fetched for one conversation, and every message in it names that one"
    );

    let mut cursor = HistoryCursor::new(chat.id);
    let (back, where_the_reader_is) = walk_backwards(&proto, chat.id, &mut cursor, &latest).await;

    // Now back the other way, from where a reader who had scrolled to the top of
    // the conversation would be standing.
    let forward = if back.pages == 0 {
        eprintln!(
            "skipped: conversation {} is shorter than one page, so there is nothing to \
             walk forwards from",
            chat.id
        );
        None
    } else {
        // The cursor describes the whole region the walk covered, and a page
        // counted from the newest of *that* is the end of the conversation
        // again. What a reader at the top is looking at is the last page.
        cursor.reset_to(&where_the_reader_is);
        Some(walk_forwards(&proto, chat.id, &mut cursor, &back.seen).await)
    };

    let newest = latest.last().expect("the newest page is not empty").id;
    if let Some(forward) = &forward {
        assert!(
            forward.pages > 0,
            "a walk with messages to recover must fetch at least one page"
        );
    }

    // A page that replaces the window is surrounded by the unknown on both
    // sides, whatever the cursor said before it: the only thing that survived
    // the jump is where it landed.
    let target = latest[latest.len() / 2].id;
    let around = page_around(&proto, chat.id, target).await;

    let mut around_cursor = HistoryCursor::new(chat.id);
    around_cursor.reset_to(&around);
    assert!(
        !around_cursor.exhausted_older() && !around_cursor.exhausted_newer(),
        "both directions open again after a jump"
    );
    assert_eq!(
        around_cursor.oldest_loaded_id(),
        around.first().map(|m| m.id)
    );
    assert_eq!(
        around_cursor.newest_loaded_id(),
        around.last().map(|m| m.id)
    );

    // The other thing a page around a message is for: taking a reader to the
    // first of their unread messages, which the client places by arithmetic.
    if page_around_the_unread(&proto, &chat).await.is_none() {
        eprintln!(
            "skipped: conversation {} has nothing unread to jump to",
            chat.id
        );
    }

    // Whether the round trip closed is reported rather than asserted: a
    // conversation that ran out of pages before getting back to where it started
    // has still been proved not to skip or repeat anything within the range it
    // did cover.
    eprintln!(
        "paged {} page(s) back through conversation {} ({} message(s), {}); \
         {} page(s) forward again ({} message(s), {}, back to the newest: {}); \
         a page around message {target} held {}",
        back.pages,
        chat.id,
        back.seen.len(),
        if back.reached_the_end {
            "walked back to the start"
        } else {
            "did not walk back to the start"
        },
        forward.as_ref().map_or(0, |walk| walk.pages),
        forward.as_ref().map_or(0, |walk| walk.seen.len()),
        match &forward {
            Some(walk) if walk.reached_the_end => "walked forward to the end",
            Some(_) => "did not walk forward to the end",
            None => "not walked forwards",
        },
        forward
            .as_ref()
            .is_some_and(|walk| walk.seen.contains(&newest)),
        around.len(),
    );
}

// ---- what the checks are worth --------------------------------------------
//
// Everything above needs a datacenter, so without one it proves nothing at all —
// which is the whole reason these tests report what they examined. The
// assertions inside the walks are the other half: they are what turns a run
// against Telegram into a verdict, and a check that cannot fail is not one.
//
// So they are exercised here, against pages built to be wrong in each of the ways
// a wrong `offset_id` or `add_offset` would make them wrong.

/// A message in a conversation, as a page would carry it.
fn message(chat_id: i64, id: i64) -> Message {
    Message {
        id,
        chat_id,
        text: "text".into(),
        timestamp: 1_700_000_000,
        status: domain::message::MessageStatus::Received,
        is_outgoing: false,
        reply_to: None,
    }
}

/// A page of messages in one conversation, oldest first.
fn page(chat_id: i64, ids: &[i64]) -> Vec<Message> {
    ids.iter().map(|id| message(chat_id, *id)).collect()
}

/// A page beside its anchor is accepted, and it does not repeat the anchor: the
/// wire counts from a message exclusively, so a page that came back with it
/// would be a page the cursor has already accounted for.
#[test]
fn a_page_beside_its_anchor_is_accepted_in_both_directions() {
    assert_page_beside(&page(7, &[3, 4, 5]), 7, 6, Side::Older);
    assert_page_beside(&page(7, &[7, 8, 9]), 7, 6, Side::Newer);
}

/// The failure this whole walk exists to catch: a negative shift that Telegram
/// answers by sending the messages *in front of* the anchor. Every one of them
/// is on the wrong side, and an ordinary-looking page otherwise.
#[test]
#[should_panic(expected = "a newer page must hold only messages newer than its anchor")]
fn a_newer_page_that_came_back_with_the_wrong_side_is_refused() {
    assert_page_beside(&page(7, &[4, 5, 6]), 7, 6, Side::Newer);
}

#[test]
#[should_panic(expected = "an older page must hold only messages older than its anchor")]
fn an_older_page_that_reached_past_its_anchor_is_refused() {
    assert_page_beside(&page(7, &[7, 8]), 7, 6, Side::Older);
}

/// A page is one conversation's, whatever else is wrong with it.
#[test]
#[should_panic(expected = "every message names that one")]
fn a_page_naming_another_conversation_is_refused() {
    assert_page_beside(&page(9, &[8, 10]), 7, 6, Side::Newer);
}

#[test]
#[should_panic(expected = "a page must come out oldest first")]
fn a_page_that_is_not_oldest_first_is_refused() {
    assert_page_beside(&page(7, &[10, 8, 9]), 7, 6, Side::Newer);
}

#[test]
#[should_panic(expected = "must not return the same message")]
fn a_walk_that_saw_a_message_twice_is_refused() {
    assert_no_repeats(&[8, 9, 9, 10]);
}

/// The failure a shift that is too wide makes: a hole inside the range the walk
/// covered. The messages on either side of it came back, so nothing looks wrong
/// except what is missing.
#[test]
#[should_panic(expected = "paging forwards skipped message 9")]
fn a_walk_that_skipped_a_message_it_had_room_to_reach_is_refused() {
    assert_nothing_skipped(&[8, 10, 11], &[8, 9, 10, 11], 6);
}

/// The other side of that bound: a walk that ran out of pages before reaching
/// messages the outward walk had seen has not skipped anything, and saying so
/// would fail a run for being inconclusive.
#[test]
fn a_walk_that_ran_out_of_pages_is_not_accused_of_skipping() {
    assert_nothing_skipped(&[8, 9], &[8, 9, 10, 11, 12, 13], 6);
}

/// And the messages at or behind where the walk started are none of its
/// business — they were already loaded.
#[test]
fn a_walk_is_not_asked_for_what_was_loaded_before_it_started() {
    assert_nothing_skipped(&[8, 9], &[1, 2, 3, 8, 9], 6);
}
