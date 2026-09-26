//! The live update feed, described in this crate's own vocabulary.
//!
//! `grammers` stops at this module's private functions. [`MessageInfo`] and
//! [`UpdateKind`] are built from numbers and strings, so `proto` can turn an
//! event into a `domain` type without ever naming a `grammers` type — which is
//! what `make boundary` asserts for the whole crate.
//!
//! # Where the feed comes from
//!
//! `grammers` creates the update channel together with its connection pool and
//! starts feeding it as soon as a connection exists, which after any request is
//! always. Nothing reads that channel until
//! [`stream_updates`](grammers_client::Client::stream_updates) is handed it, and
//! it is unbounded, so a client that is built, authorised and then left without
//! a subscriber would buffer every update Telegram sends for as long as it
//! runs. That is exactly the growth this crate's memory budget cannot absorb.
//!
//! So the receiving end is not parked. `UpdateRelay` moves the pool's channel
//! into one this crate owns and *discards* everything until the first
//! subscriber takes it. Discarding loses nothing: the session's update state is
//! only advanced by the message box inside the stream, so a client that has not
//! subscribed has not moved its position, and `catch_up` replays from that
//! position when it finally does.
//!
//! # Why the mapping is split in two
//!
//! A `grammers::Update` can only be built from a raw response and a peer map,
//! so nothing that consumes one can be tested without a datacenter. That makes
//! `update_to_kind` and everything it calls untestable here, so they are kept
//! as thin as possible, and every decision inside them is pulled out into a
//! free function over primitives: `is_private_conversation`,
//! `peer_kind_from_id`, `message_info` and `deleted_ids`. Those are the parts
//! that can be wrong, and they run on every CI job.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use grammers_client::client::updates::UpdateStream;
use grammers_client::session::defs::PeerKind;
use grammers_client::session::updates::UpdatesLike;
use grammers_client::types::update::Message;
use grammers_client::{InvocationError, Update, UpdatesConfiguration};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::client::Client;
use crate::dialogs::DialogKind;
use crate::error::{FrameworkError, RequestError};
use crate::session::StoreSession;

/// How many discarded updates to let pass between debug logs.
///
/// A quiet account can produce a steady trickle of events televim has no use
/// for, so logging each one would drown the log it is supposed to inform.
const DROP_LOG_INTERVAL: u64 = 100;

/// A message as this crate describes it.
///
/// The fields are primitives and strings so that a `grammers` type never
/// escapes the crate. `chat_peer_id` is the *bare* identifier of the
/// conversation the message belongs to, matching
/// [`DialogInfo::peer_id`](crate::DialogInfo::peer_id), so an event can be
/// matched against the chat list without a lookup table.
#[derive(Debug, Clone)]
pub struct MessageInfo {
    /// Identifier of the message, unique within its conversation.
    pub id: i64,

    /// Bare identifier of the conversation the message belongs to.
    pub chat_peer_id: i64,

    /// The message's text, empty when it carries none.
    pub text: String,

    /// Unix timestamp in seconds, zero when Telegram sent no date.
    pub timestamp: i64,

    /// Whether the logged-in account sent it.
    pub is_outgoing: bool,
}

/// Something that happened to a conversation televim displays.
///
/// The variants are deliberately few. Anything Telegram sends that is not a
/// message in a private conversation is discarded inside
/// [`UpdateSubscription::next`], so a caller never has to ask what an event
/// means for a group or a bot it does not render.
#[derive(Debug, Clone)]
pub enum UpdateKind {
    /// A message arrived.
    NewMessage(MessageInfo),

    /// A message's text changed.
    MessageEdited(MessageInfo),

    /// Messages were deleted.
    ///
    /// Telegram does not say which conversation these belonged to. A deletion
    /// from a channel names the channel, and those are discarded; the other
    /// form covers private chats and small groups, whose messages share one
    /// account-wide numbering, so it carries identifiers and nothing else. A
    /// caller holding several conversations has to look for the identifiers in
    /// each of them.
    MessagesDeleted {
        /// Identifiers of the deleted messages.
        message_ids: Vec<i64>,
    },
}

impl Client {
    /// Subscribes to the updates Telegram sends for this account.
    ///
    /// The feed is filtered to private conversations: groups, channels, bots
    /// and events that are not about messages never reach the caller. Doing
    /// that here rather than downstream is what keeps `domain` and `tui` free
    /// of Telegram's peer taxonomy.
    ///
    /// # Take once
    ///
    /// An account's updates are a single ordered sequence, so a client has
    /// exactly one feed. This can be called once; a second call returns
    /// [`FrameworkError::UpdatesAlreadySubscribed`]. A caller that needs to fan
    /// events out should subscribe once and hand the one
    /// [`UpdateSubscription`] to whatever wants them.
    ///
    /// # Fetch first
    ///
    /// [`Client::fetch_dialogs`] should have been called at least once since
    /// the last login, because iterating dialogs is what makes Telegram
    /// disclose a peer's `access_hash` and a channel's persistent timestamp,
    /// and resolving what was missed while offline reads both back out of the
    /// session. A client resumed from a warm session store is already warmed;
    /// a freshly logged-in one is not.
    ///
    /// # Draining
    ///
    /// `grammers` only moves updates out of its queue while the stream is being
    /// polled, so a caller that stops polling lets the queue behind it grow. The
    /// subscription is therefore meant to be driven continuously — by the task
    /// that owns it, for as long as updates are wanted.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UpdatesAlreadySubscribed`] if the feed was
    /// already taken.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::{ClientBuilder, UpdateKind};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// let chats = client.fetch_dialogs().await?;
    /// println!("{} conversations", chats.len());
    ///
    /// let mut updates = client.subscribe_updates()?;
    ///
    /// while let Some(event) = updates.next().await {
    ///     match event? {
    ///         UpdateKind::NewMessage(message) => println!("{}", message.text),
    ///         UpdateKind::MessageEdited(message) => println!("edited: {}", message.text),
    ///         UpdateKind::MessagesDeleted { message_ids } => println!("{message_ids:?}"),
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn subscribe_updates(&self) -> Result<UpdateSubscription, FrameworkError> {
        let receiver = self.updates().take()?;

        let stream = self.inner().stream_updates(
            receiver,
            UpdatesConfiguration {
                // Replays whatever arrived while the client was offline, out of
                // the update state the session store holds. The queue limit is
                // left at grammers' default, which bounds the buffer.
                catch_up: true,
                ..UpdatesConfiguration::default()
            },
        );

        Ok(UpdateSubscription {
            stream,
            session: self.session_handle(),
            dropped: 0,
        })
    }
}

/// A live feed of the updates Telegram sends for one account.
///
/// Obtained from [`Client::subscribe_updates`], which can only be called once.
/// The subscription owns the `grammers` stream: dropping it stops delivery and
/// writes the session back, so it has to be held for as long as updates are
/// wanted.
pub struct UpdateSubscription {
    stream: UpdateStream,
    session: Arc<StoreSession>,
    dropped: u64,
}

impl UpdateSubscription {
    /// Awaits the next update televim displays.
    ///
    /// `None` means the feed has ended, which only happens once the client is
    /// shutting down; a caller that sees it should stop polling rather than
    /// retry. An `Err` is a request that failed while resolving a gap in the
    /// sequence — the feed is still usable afterwards.
    ///
    /// Updates televim does not display are skipped here rather than returned,
    /// so this can await for a long time without producing anything. They are
    /// counted; [`UpdateSubscription::dropped`] reports the total.
    pub async fn next(&mut self) -> Option<Result<UpdateKind, FrameworkError>> {
        loop {
            let update = match self.stream.next().await {
                Ok(update) => update,
                // The pool's sender is gone, so nothing else can arrive. That
                // is the end of the feed rather than a failure.
                Err(InvocationError::Dropped) => return None,
                Err(error) => {
                    return Some(Err(FrameworkError::from(RequestError::from_invocation(
                        &error,
                    ))));
                }
            };

            if let Some(kind) = update_to_kind(&update) {
                return Some(Ok(kind));
            }

            self.note_drop();
        }
    }

    /// How many updates have been discarded so far.
    ///
    /// Everything Telegram sends that is not a message in a private
    /// conversation lands here, so a count that climbs while the account is
    /// quiet is the filter doing its job.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Counts a discarded update, logging the first and every hundredth.
    fn note_drop(&mut self) {
        self.dropped += 1;

        if self.dropped == 1 || self.dropped.is_multiple_of(DROP_LOG_INTERVAL) {
            tracing::debug!(
                dropped = self.dropped,
                "dropped an update that is not a private conversation message"
            );
        }
    }
}

impl fmt::Debug for UpdateSubscription {
    /// Renders the feed without exposing the stream it owns.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateSubscription")
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

impl Drop for UpdateSubscription {
    /// Writes the session back once delivery has stopped.
    ///
    /// `grammers` records how far the stream got in the session object when the
    /// stream itself is dropped, and that happens after this body runs. The
    /// state is therefore synchronised here, explicitly, and only then
    /// persisted — otherwise the last position would be lost, and the next
    /// launch would replay updates the caller has already seen.
    fn drop(&mut self) {
        self.stream.sync_update_state();

        match self.session.persist_if_dirty() {
            Ok(false) => {}
            Ok(true) => tracing::debug!("wrote the session back when the update feed ended"),
            Err(error) => tracing::warn!(
                %error,
                "the update position changed but could not be saved; the next launch may replay updates"
            ),
        }
    }
}

/// The update pipeline a [`Client`] owns until something subscribes.
///
/// See the module documentation for why the pool's channel cannot simply be
/// held: it is unbounded, and the pool feeds it whether or not anyone reads.
pub(crate) struct UpdateRelay {
    /// The relaying end, taken by the first subscriber.
    receiver: Mutex<Option<UnboundedReceiver<UpdatesLike>>>,

    /// Whether a subscriber has taken it, which is what opens the relay.
    open: Arc<AtomicBool>,
}

impl UpdateRelay {
    /// Starts relaying `source`, and returns the end a subscriber will take.
    ///
    /// Must be called from within a `tokio` runtime: the relay runs on it.
    pub(crate) fn start(source: UnboundedReceiver<UpdatesLike>) -> Self {
        let (sink, receiver) = mpsc::unbounded_channel();
        let open = Arc::new(AtomicBool::new(false));

        tokio::spawn(relay(source, sink, Arc::clone(&open)));

        Self {
            receiver: Mutex::new(Some(receiver)),
            open,
        }
    }

    /// Hands the receiving end over and opens the relay.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UpdatesAlreadySubscribed`] if it was already
    /// taken.
    pub(crate) fn take(&self) -> Result<UnboundedReceiver<UpdatesLike>, FrameworkError> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(FrameworkError::UpdatesAlreadySubscribed)?;

        // Opened only once the receiving end is out of the mutex, so nothing
        // can be handed over while the relay is still discarding. The flag
        // carries no other data — the mutex is what makes the handover visible
        // — so relaxed ordering is enough.
        self.open.store(true, Ordering::Relaxed);

        Ok(receiver)
    }
}

/// Moves updates from the pool's channel to the subscriber's, discarding them
/// until there is one.
async fn relay(
    mut source: UnboundedReceiver<UpdatesLike>,
    sink: UnboundedSender<UpdatesLike>,
    open: Arc<AtomicBool>,
) {
    let mut discarded: u64 = 0;

    while let Some(update) = source.recv().await {
        if !open.load(Ordering::Relaxed) {
            discarded += 1;

            if discarded == 1 || discarded.is_multiple_of(DROP_LOG_INTERVAL) {
                tracing::debug!(
                    discarded,
                    "discarded updates because nothing is subscribed to the feed yet"
                );
            }
            continue;
        }

        if sink.send(update).is_err() {
            // The subscriber is gone and the receiving end is closed. Returning
            // drops `source`, which is what stops the pool from delivering.
            break;
        }
    }
}

/// Describes an update, or `None` if televim does not display it.
fn update_to_kind(update: &Update) -> Option<UpdateKind> {
    match update {
        Update::NewMessage(message) => private_message(message).map(UpdateKind::NewMessage),
        Update::MessageEdited(message) => private_message(message).map(UpdateKind::MessageEdited),
        Update::MessageDeleted(deletion) => deleted_ids(deletion.channel_id(), deletion.messages())
            .map(|message_ids| UpdateKind::MessagesDeleted { message_ids }),
        // Callback and inline queries only exist for bots, and `Raw` is the
        // escape hatch for whatever this crate does not model.
        _ => None,
    }
}

/// Describes a message, if it belongs to a conversation televim displays.
///
/// The peer is taken from the identifier rather than from the peer map,
/// because the map only holds the peers Telegram has disclosed so far and an
/// identifier is enough to tell a user from a group.
fn private_message(message: &Message) -> Option<MessageInfo> {
    let peer_id = message.peer_id();

    if !is_private_conversation(peer_kind_from_id(peer_id.kind())) {
        return None;
    }

    Some(message_info(
        message.id(),
        peer_id.bare_id(),
        message.text(),
        message.date().timestamp(),
        message.outgoing(),
    ))
}

/// Whether televim displays a conversation of this kind.
///
/// Bots are kept alongside people: a bot is a user as far as Telegram's peer
/// identifiers go, and a conversation with one is still one-to-one. Only groups
/// and channels are dropped.
fn is_private_conversation(kind: DialogKind) -> bool {
    matches!(kind, DialogKind::PrivateUser | DialogKind::Bot)
}

/// Maps the kind Telegram encodes in a peer identifier onto this crate's own.
///
/// An identifier does not carry the bot flag — that only exists on the account
/// object — so a person and a bot are indistinguishable here. Both are private
/// conversations, which is all [`is_private_conversation`] needs to know.
fn peer_kind_from_id(kind: PeerKind) -> DialogKind {
    match kind {
        PeerKind::User | PeerKind::UserSelf => DialogKind::PrivateUser,
        PeerKind::Chat => DialogKind::Group,
        PeerKind::Channel => DialogKind::Channel,
    }
}

/// Builds a [`MessageInfo`] from the pieces `grammers` exposes.
///
/// The identifier is widened here: Telegram numbers messages within a
/// conversation using an `i32`, and the rest of the workspace counts in `i64`.
fn message_info(
    id: i32,
    chat_peer_id: i64,
    text: &str,
    timestamp: i64,
    is_outgoing: bool,
) -> MessageInfo {
    MessageInfo {
        id: i64::from(id),
        chat_peer_id,
        text: text.to_owned(),
        timestamp,
        is_outgoing,
    }
}

/// The identifiers a deletion carries, or `None` if it is not televim's.
///
/// `channel` is the identifier Telegram names when a message was deleted from a
/// channel. Channels are not displayed, so such a deletion is dropped whole.
/// The other form covers private chats and small groups, whose messages share
/// one account-wide numbering, and carries identifiers with no conversation
/// attached.
fn deleted_ids(channel: Option<i64>, messages: &[i32]) -> Option<Vec<i64>> {
    if channel.is_some() {
        return None;
    }

    Some(messages.iter().copied().map(i64::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The feed is meant to be driven by a task that owns it, and the client it
    // came from is shared, so both have to cross a thread boundary. `Sync` is
    // deliberately not asserted for the subscription: it is a single-consumer
    // stream, and a second reference to it would mean a second reader.
    static_assertions::assert_impl_all!(Client: Send, Sync);
    static_assertions::assert_impl_all!(UpdateSubscription: Send);

    #[test]
    fn people_and_bots_are_displayed_but_groups_and_channels_are_not() {
        assert!(is_private_conversation(DialogKind::PrivateUser));
        assert!(
            is_private_conversation(DialogKind::Bot),
            "a bot is a user to telegram, and the conversation is still one-to-one"
        );
        assert!(!is_private_conversation(DialogKind::Group));
        assert!(!is_private_conversation(DialogKind::Channel));
    }

    #[test]
    fn a_deletion_from_a_channel_is_dropped_whole() {
        assert_eq!(
            deleted_ids(Some(42), &[1, 2, 3]),
            None,
            "a channel is not displayed, so neither is its traffic"
        );
    }

    #[test]
    fn an_account_wide_deletion_keeps_its_identifiers() {
        assert_eq!(
            deleted_ids(None, &[7, 8]),
            Some(vec![7, 8]),
            "these have no conversation attached, so the caller needs all of them"
        );
        assert_eq!(deleted_ids(None, &[]), Some(Vec::new()));
    }

    #[test]
    fn identifiers_are_widened_without_changing_sign_or_order() {
        assert_eq!(
            deleted_ids(None, &[i32::MIN, -1, 0, 1, i32::MAX]),
            Some(vec![-2_147_483_648, -1, 0, 1, 2_147_483_647])
        );
    }

    #[test]
    fn a_message_is_described_field_for_field() {
        let info = message_info(7, 42, "hello", 1_700_000_000, true);

        assert_eq!(info.id, 7);
        assert_eq!(info.chat_peer_id, 42);
        assert_eq!(info.text, "hello");
        assert_eq!(info.timestamp, 1_700_000_000);
        assert!(info.is_outgoing);
    }

    #[test]
    fn a_message_without_text_is_still_described() {
        let info = message_info(7, 42, "", 0, false);

        assert!(info.text.is_empty(), "a photo and a sticker have no text");
        assert_eq!(
            info.timestamp, 0,
            "zero is grammers' 'no date', and the domain counts from the same epoch"
        );
        assert!(!info.is_outgoing);
    }
}
