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
//! Two things this seam wants proven cannot be provoked with the single account
//! these tests have. They are recorded here rather than left as a silent hole:
//!
//! - **An update arriving for real.** Nothing sends to the test account while a
//!   run is in progress, so the feed is usually silent and the drain below
//!   checks zero updates. Provoking one needs a second account, and a send API
//!   the framework does not expose yet.
//! - **The offline gap.** `catch_up` replays what arrived while the client was
//!   not running, which again needs something to send to it in the meantime.
//!
//! Both are covered as far as one account allows. The fetched list's ordering
//! is asserted directly, which is deterministic, and the number of updates
//! actually checked — along with the number the framework discarded — is
//! printed, so a run that proved little says so instead of looking like a pass.
//!
//! The whole file is compiled only under the crate's `live` feature, which is
//! what turns on the framework's client and the `proto` wrapper it is built
//! from.

#![cfg(feature = "live")]

use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use domain::chat::Chat;
use domain::updates::{ChatList, UpdateEvent};
use proto::{ProtoClient, ProtoError, UpdateStream};
use telegram_framework::session::FileStore;
use telegram_framework::{Client, ClientBuilder, FrameworkError, SignInResult};

/// How long to wait for the feed before accepting that nothing is arriving.
///
/// The feed is quiet whenever nobody is sending to the account, and it does not
/// say so: it simply produces no event. A test therefore has to bound its wait
/// rather than await forever.
const FEED_WINDOW: Duration = Duration::from_secs(10);

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
