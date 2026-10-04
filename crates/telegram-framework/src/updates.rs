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
//! free function over primitives: `is_displayed_conversation`,
//! `peer_kind_from_id`, `message_info` and `deleted_ids`. Those are the parts
//! that can be wrong, and they run on every CI job.
//!
//! One lookup cannot be pulled out that way. Whether a peer is a bot is not in
//! its identifier — the flag lives on the account object — so it can only come
//! from the peer map the update arrived with, and that map is a `grammers`
//! value. `bot_flag` is therefore as thin as it can be: it reads the flag and
//! nothing else, and `is_displayed_conversation` decides what it means.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use grammers_client::InvocationError;
use grammers_client::client::{UpdateStream, UpdatesConfiguration};
use grammers_client::peer::Peer;
use grammers_client::session::types::PeerKind;
use grammers_client::session::updates::UpdatesLike;
use grammers_client::update::{Message, Update};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::client::Client;
use crate::dialogs::DialogKind;
use crate::error::{FrameworkError, RequestError};
use crate::media::{MediaKind, classify_typed};
use crate::session::StoreSession;
use crate::tl;

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

    /// Identifier of the message this one replies to, if it is a reply.
    ///
    /// Telegram numbers a reply's target within the same conversation, so this
    /// names a message by the same identifier space [`MessageInfo::id`] does.
    pub reply_to_msg_id: Option<i32>,

    /// What the message carries, if anything.
    ///
    /// This is the framework's *reading of the wire*, not a type the rest of
    /// the workspace shares: it names the kind and nothing else — where the
    /// bytes are is re-derived from the message itself when one is fetched, so
    /// no locator is carried here.
    ///
    /// A kind this build does not model is reported as [`MediaKind::File`]
    /// rather than as nothing, because a message that carries something must
    /// not read as one that does not. Only a message carrying no media at all
    /// is `None`.
    pub media: Option<MediaKind>,
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
    /// The event names no conversation, and cannot: the update it comes from —
    /// `updateDeleteMessages` — carries only the identifiers, a `pts` and a
    /// `pts_count`. There is no peer field to scope it by. Deletions from a
    /// channel arrive as a different update that does name its channel, and
    /// those are discarded because a channel is not displayed.
    ///
    /// The identifiers are therefore from the one sequence that private chats
    /// and small groups share, so a caller holding several conversations has to
    /// look for each identifier in all of them.
    MessagesDeleted {
        /// Identifiers of the deleted messages.
        message_ids: Vec<i64>,
    },

    /// The reader's own messages in a conversation have been read, up to a point.
    ///
    /// A watermark rather than a flag per message, because that is how Telegram
    /// says it: `updateReadHistoryOutbox` carries one number meaning "outgoing
    /// messages up to here have been read", and its `pts` is a sequence number
    /// for the gap-tracking this crate does not use.
    ///
    /// Read from the raw update rather than from a named `grammers` variant,
    /// because grammers has none and puts this and every other update it does
    /// not model into `Update::Raw`. Best-effort by nature: Telegram delivers
    /// each update to one randomly chosen active session, and a queue under load
    /// can drop one, so a receipt that never arrives is a receipt that was never
    /// sent rather than one that was lost here.
    ReadReceipt {
        /// The conversation whose messages were read.
        chat_peer_id: i64,

        /// Every outgoing message in it up to this identifier has been read.
        max_id: i64,
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
    /// session. Whether that has happened is reported by
    /// [`Client::has_fetched_dialogs`], and subscribing anyway is logged at
    /// `debug` — it is not an error, but on a client that has just logged in it
    /// means the gap is resolved against a cache holding no peers.
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
    /// let mut updates = client.subscribe_updates().await?;
    ///
    /// while let Some(event) = updates.next().await {
    ///     match event? {
    ///         UpdateKind::NewMessage(message) => println!("{}", message.text),
    ///         UpdateKind::MessageEdited(message) => println!("edited: {}", message.text),
    ///         UpdateKind::MessagesDeleted { message_ids } => println!("{message_ids:?}"),
    ///         UpdateKind::ReadReceipt { chat_peer_id, max_id } => {
    ///             println!("{chat_peer_id} read up to {max_id}")
    ///         }
    ///     }
    /// }
    ///
    /// // The position the feed reached is recorded here rather than on drop,
    /// // // so that the next launch does not replay what was just read.
    /// updates.finish().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn subscribe_updates(&self) -> Result<UpdateSubscription, FrameworkError> {
        if !self.has_fetched_dialogs() {
            tracing::debug!(
                "subscribing to updates before the chat list was fetched; a gap \
                 would be resolved against a session holding no peers yet"
            );
        }

        let receiver = self.updates().take()?;

        let stream = self
            .inner()
            .stream_updates(
                receiver,
                UpdatesConfiguration {
                    // Replays whatever arrived while the client was offline, out of
                    // the update state the session store holds. The queue limit is
                    // left at grammers' default, which bounds the buffer.
                    catch_up: true,
                    ..UpdatesConfiguration::default()
                },
            )
            .await
            // The only thing that can fail in here is a read of the session, and
            // this crate's session cannot fail — so this arm is unreachable in
            // practice, and is mapped rather than dropped so that a session that
            // somehow did fail is not reported as a network fault.
            .map_err(|error| FrameworkError::from(RequestError::Session(error.to_string())))?;

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
    /// Records how far the feed got, and writes the session back.
    ///
    /// This has to be called for the position to be kept, and it is explicit
    /// rather than automatic because `grammers` no longer writes the position
    /// when the stream is dropped: it asks for it to be synchronised, and the
    /// call is `async` while a destructor cannot await. The mirror is therefore
    /// only as current as the last call here, and a feed that is dropped without
    /// one leaves the session pointing at an older position — which the next
    /// launch resolves by replaying updates the reader has already seen.
    ///
    /// A caller that reads the feed to its end should call this. It consumes the
    /// subscription, because there is nothing left to read afterwards.
    pub async fn finish(self) -> Result<(), FrameworkError> {
        self.stream
            .sync_update_state()
            .await
            .map_err(|error| FrameworkError::from(RequestError::Session(error.to_string())))?;
        self.persist_position();
        Ok(())
    }

    /// Writes the session back if anything in it changed.
    ///
    /// Split out of [`Drop`] so that [`Self::finish`] and the destructor agree on
    /// what is written, and do not disagree about whether it was.
    fn persist_position(&self) {
        match self.session.persist_if_dirty() {
            Ok(false) => {}
            Ok(true) => tracing::debug!("wrote the session back when the update feed ended"),
            Err(error) => tracing::warn!(
                %error,
                "the update position changed but could not be saved; the next launch may replay updates"
            ),
        }
    }
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
    /// Writes back whatever the session already holds.
    ///
    /// A destructor cannot await, and `grammers` no longer records the position
    /// for itself when the stream is dropped — it has to be asked, and asking is
    /// `async`. So the position is synchronised by [`Self::finish`], and this
    /// only writes back what the mirror already carries. A subscription dropped
    /// without having been finished therefore persists a position that may be
    /// behind the one the feed actually reached.
    ///
    /// It is still worth doing: a peer cached by a request that read history, or
    /// a datacenter migrated mid-session, is in the mirror and would otherwise be
    /// lost, and the write is a no-op when nothing changed.
    fn drop(&mut self) {
        self.persist_position();
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

    /// The relaying task, stopped when the client is dropped.
    task: JoinHandle<()>,
}

impl UpdateRelay {
    /// Starts relaying `source`, and returns the end a subscriber will take.
    ///
    /// Must be called from within a `tokio` runtime: the relay runs on it.
    pub(crate) fn start(source: UnboundedReceiver<UpdatesLike>) -> Self {
        let (sink, receiver) = mpsc::unbounded_channel();
        let open = Arc::new(AtomicBool::new(false));

        let task = tokio::spawn(relay(source, sink, Arc::clone(&open)));

        Self {
            receiver: Mutex::new(Some(receiver)),
            open,
            task,
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

impl Drop for UpdateRelay {
    /// Stops relaying, and with it the pool's delivery.
    ///
    /// The task owns the pool's receiving end, so aborting it drops that end
    /// and the pool stops feeding a channel nothing will read. A subscriber
    /// still holding the other side sees the feed end, which is what a client
    /// that no longer exists should look like — rather than a feed that stays
    /// open for as long as the process runs.
    fn drop(&mut self) {
        self.task.abort();
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
        // Everything this crate does not model arrives here, and most of it is
        // still not displayed. Matching the raw enum once, in `read_receipt`, is
        // what keeps that judgement in one place — the same reasoning
        // `dialog_to_info` uses to rule folders out.
        Update::Raw(raw) => read_receipt(raw),
        // Callback and inline queries only exist for bots.
        _ => None,
    }
}

/// Describes a read acknowledgement, or `None` if this update is not one.
///
/// The raw update is matched on its own variant rather than through a `grammers`
/// wrapper because grammers has none for it: `updateReadHistoryOutbox` reaches
/// the catch-all at the end of `Update::from_raw` and arrives as
/// `Update::Raw`. Its `pts` and `pts_count` are the gap-tracking numbers this
/// crate does not use — the update position is already in the session — so only
/// the peer and the watermark are taken.
fn read_receipt(raw: &grammers_client::update::Raw) -> Option<UpdateKind> {
    let tl::enums::Update::ReadHistoryOutbox(read) = &raw.raw else {
        return None;
    };
    let chat_peer_id = read_peer(&read.peer)?;

    Some(UpdateKind::ReadReceipt {
        chat_peer_id,
        max_id: i64::from(read.max_id),
    })
}

/// The conversation a read acknowledgement is about, if it is one this client
/// displays.
///
/// A user, and only a user: a group and a channel are not conversations televim
/// renders, so nothing read inside one is shown and nothing is recorded for it.
/// The bot flag is not here to be had — it lives on the account object, which a
/// raw update does not carry — and it does not need to be: a bot's conversation
/// is dropped by the chat list's own filter, and its read state with it.
fn read_peer(peer: &tl::enums::Peer) -> Option<i64> {
    match peer {
        tl::enums::Peer::User(user) => Some(user.user_id),
        tl::enums::Peer::Chat(_) | tl::enums::Peer::Channel(_) => None,
    }
}

/// Describes a message, if it belongs to a conversation televim displays.
///
/// The peer is taken from the identifier rather than from the peer map, because
/// the map only holds the peers Telegram has disclosed so far and an identifier
/// is enough to tell a user from a group. The bot flag is the one thing an
/// identifier cannot answer, so it comes from the map — see [`bot_flag`].
fn private_message(message: &Message) -> Option<MessageInfo> {
    let peer_id = message.peer_id();

    if !is_displayed_conversation(peer_kind_from_id(peer_id.kind()), bot_flag(message)) {
        return None;
    }

    // A peer with no bare identifier is the account itself, and there is no
    // number to substitute for one — see the same note in `dialogs.rs`. Skipping
    // it is unreachable for a conversation Telegram named, and dropping it beats
    // filing it under an identifier that addresses nothing.
    let chat_peer_id = peer_id.bare_id()?;

    Some(message_info(
        message.id(),
        chat_peer_id,
        message.text(),
        message.date().timestamp(),
        message.outgoing(),
        message.reply_to_message_id(),
        classify_typed(message.media().as_ref()),
    ))
}

/// Whether the conversation's peer is a bot, if the update disclosed it.
///
/// The flag lives on the account object, not on the peer identifier, so it can
/// only be read out of the peer map the update arrived with. `None` means that
/// map did not hold the peer — a cache miss, not an answer.
fn bot_flag(message: &Message) -> Option<bool> {
    match message.peer()? {
        Peer::User(user) => Some(user.is_bot()),
        // A group and a channel are not users and carry no bot flag; they are
        // already excluded by their kind.
        Peer::Group(_) | Peer::Channel(_) => None,
    }
}

/// Whether televim displays a conversation of this kind.
///
/// Only people. A group and a channel are not conversations this client
/// renders, and neither is a bot: televim is a client for talking to people,
/// and a bot's traffic would otherwise reach `domain` as an ordinary message
/// and raise an unread count for a conversation that is never in the list.
///
/// `is_bot` is `None` when the peer was not in the update's peer map. That is
/// treated as a person rather than as a bot, deliberately: dropping a real
/// message because a cache missed would lose it for good, while a bot that
/// slips through is dropped by the chat list's own filter and costs one
/// `apply_update` that reports no change.
fn is_displayed_conversation(kind: DialogKind, is_bot: Option<bool>) -> bool {
    matches!(kind, DialogKind::PrivateUser) && is_bot != Some(true)
}

/// Maps the kind Telegram encodes in a peer identifier onto this crate's own.
///
/// An identifier does not carry the bot flag — that only exists on the account
/// object — so a person and a bot are indistinguishable here. Both are users,
/// which is what [`is_displayed_conversation`] is told; whether the user is a
/// bot is a separate question, answered from the peer map.
fn peer_kind_from_id(kind: PeerKind) -> DialogKind {
    match kind {
        PeerKind::User => DialogKind::PrivateUser,
        PeerKind::Chat => DialogKind::Group,
        PeerKind::Channel => DialogKind::Channel,
    }
}

/// Builds a [`MessageInfo`] from the pieces `grammers` exposes.
///
/// The identifier is widened here: Telegram numbers messages within a
/// conversation using an `i32`, and the rest of the workspace counts in `i64`.
///
/// Reused by the history fetch, so that a message read out of a conversation
/// and the same message arriving over the feed are described identically —
/// which is what lets the two be deduplicated against each other. That is why
/// `media` is passed in already classified: each path reads it off its own
/// message type, and this is where the two answers are forced to agree.
pub(crate) fn message_info(
    id: i32,
    chat_peer_id: i64,
    text: &str,
    timestamp: i64,
    is_outgoing: bool,
    reply_to_msg_id: Option<i32>,
    media: Option<MediaKind>,
) -> MessageInfo {
    MessageInfo {
        id: i64::from(id),
        chat_peer_id,
        text: text.to_owned(),
        timestamp,
        is_outgoing,
        reply_to_msg_id,
        media,
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
    fn every_real_user_keeps_its_identifier() {
        use grammers_client::session::types::PeerId;

        // `private_message` and `dialog_to_info` skip a peer that has no bare
        // identifier, because there is no number to substitute for one. That
        // skip is only sound while every user Telegram can name still reports
        // its own — if it ever stopped, the skip would quietly swallow real
        // conversations rather than fail, which is the failure mode this guards.
        for id in [1_i64, 42, 0xffff_ffff, 0x00ff_ffff_ffff] {
            let peer = PeerId::user(id).expect("in the user range");
            assert_eq!(
                peer.bare_id(),
                Some(id),
                "user {id} lost its identifier, so it would be skipped"
            );
        }

        assert!(PeerId::user(0).is_none(), "zero is not a user identifier");
    }

    #[test]
    fn the_account_itself_is_the_only_peer_without_an_identifier() {
        use grammers_client::session::types::PeerId;

        // Which is what makes the skip above unreachable: the sentinel stands
        // for the account, and a conversation arriving from Telegram always
        // names the account's real identifier instead.
        assert!(
            PeerId::self_user().bare_id().is_none(),
            "the account's own sentinel has no bare identifier"
        );
        assert_eq!(
            PeerKind::User,
            PeerId::self_user().kind(),
            "so it is classified as an ordinary private conversation"
        );
    }

    #[test]
    fn only_conversations_with_people_are_displayed() {
        assert!(is_displayed_conversation(
            DialogKind::PrivateUser,
            Some(false)
        ));
        assert!(
            is_displayed_conversation(DialogKind::PrivateUser, None),
            "a peer the update did not disclose is kept; a cache miss is not \
             evidence of a bot, and dropping a real message would lose it"
        );

        assert!(
            !is_displayed_conversation(DialogKind::PrivateUser, Some(true)),
            "a bot is a user to telegram, and its conversation is not one televim renders"
        );
        assert!(!is_displayed_conversation(DialogKind::Bot, Some(true)));
        assert!(
            !is_displayed_conversation(DialogKind::Bot, None),
            "a peer already classified as a bot is dropped even when the flag is missing"
        );
        assert!(!is_displayed_conversation(DialogKind::Group, None));
        assert!(!is_displayed_conversation(DialogKind::Channel, None));
    }

    #[test]
    fn a_peer_identifier_maps_onto_a_conversation_kind() {
        // The account's own peer reports `User`, so Saved Messages is classified
        // as a private conversation by the same arm as everyone else's.
        let cases = [
            (PeerKind::User, DialogKind::PrivateUser),
            (PeerKind::Chat, DialogKind::Group),
            (PeerKind::Channel, DialogKind::Channel),
        ];

        for (source, expected) in cases {
            assert_eq!(
                peer_kind_from_id(source),
                expected,
                "{source:?} was mistranslated"
            );
        }
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

    /// The read acknowledgement is the one update this crate reads out of the raw
    /// bucket, so the mapping off Telegram's own fields is worth pinning: the
    /// peer it names and the watermark it carries, and nothing else.
    #[test]
    fn a_read_acknowledgement_keeps_its_conversation_and_its_watermark() {
        let receipt = read_receipt(&raw_outbox(user(42), 7));

        assert!(
            matches!(
                receipt,
                Some(UpdateKind::ReadReceipt {
                    chat_peer_id: 42,
                    max_id: 7
                })
            ),
            "the peer and the watermark, with the pts numbers left behind: {receipt:?}"
        );
    }

    /// A group and a channel are not conversations this client renders, so a read
    /// inside one is dropped whole rather than filed under an identifier.
    #[test]
    fn a_read_acknowledgement_for_a_conversation_that_is_not_displayed_is_dropped() {
        let group = raw_outbox(
            tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 42 }),
            7,
        );
        let channel = raw_outbox(
            tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 42 }),
            7,
        );

        assert!(read_receipt(&group).is_none(), "a group is not displayed");
        assert!(read_receipt(&channel).is_none(), "nor is a channel");
    }

    /// `Update::Raw` is the catch-all for everything `grammers` does not model, so
    /// reading one update out of it must not mistake another for it — and the
    /// update that looks most like this one is the *inbox* watermark, which is
    /// this account's own reading and not the peer's.
    #[test]
    fn another_raw_update_is_not_a_read_acknowledgement() {
        let inbox = tl::enums::Update::ReadHistoryInbox(tl::types::UpdateReadHistoryInbox {
            folder_id: None,
            peer: user(42),
            top_msg_id: None,
            max_id: 7,
            still_unread_count: 0,
            pts: 1,
            pts_count: 1,
        });

        assert!(read_receipt(&raw_update(inbox)).is_none());
    }

    /// A watermark is an `i32` on the wire and an `i64` everywhere else, widened
    /// without changing its sign or its order — the rule deletions follow.
    #[test]
    fn a_read_watermark_is_widened_without_changing_sign_or_order() {
        assert!(
            matches!(
                read_receipt(&raw_outbox(user(1), i32::MAX)),
                Some(UpdateKind::ReadReceipt { max_id, .. }) if max_id == i64::from(i32::MAX)
            ),
            "and the domain counts in the same units the rest of the workspace does"
        );
    }

    fn user(id: i64) -> tl::enums::Peer {
        tl::enums::Peer::User(tl::types::PeerUser { user_id: id })
    }

    /// A read acknowledgement as `grammers` hands it over: in the catch-all
    /// bucket, because it models no named update for it.
    fn raw_outbox(peer: tl::enums::Peer, max_id: i32) -> grammers_client::update::Raw {
        raw_update(tl::enums::Update::ReadHistoryOutbox(
            tl::types::UpdateReadHistoryOutbox {
                peer,
                max_id,
                pts: 1,
                pts_count: 1,
            },
        ))
    }

    fn raw_update(update: tl::enums::Update) -> grammers_client::update::Raw {
        use grammers_client::session::updates::State;

        grammers_client::update::Raw {
            raw: update,
            // The mapping reads nothing of it: the update position is in the
            // session, and this state is `grammers`' own bookkeeping.
            state: State {
                date: 0,
                seq: 0,
                message_box: None,
            },
        }
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
        let info = message_info(
            7,
            42,
            "hello",
            1_700_000_000,
            true,
            Some(5),
            Some(MediaKind::Photo),
        );

        assert_eq!(info.id, 7);
        assert_eq!(info.chat_peer_id, 42);
        assert_eq!(info.text, "hello");
        assert_eq!(info.timestamp, 1_700_000_000);
        assert!(info.is_outgoing);
        assert_eq!(
            info.reply_to_msg_id,
            Some(5),
            "a reply names the message it answers, or the reply context is lost"
        );
        assert_eq!(info.media, Some(MediaKind::Photo));
    }

    #[test]
    fn a_message_without_text_is_still_described() {
        let info = message_info(7, 42, "", 0, false, None, Some(MediaKind::Photo));

        assert!(info.text.is_empty(), "a photo and a sticker have no text");
        assert_eq!(
            info.media,
            Some(MediaKind::Photo),
            "no text is not no media: a photo arrives with nothing written on it"
        );
        assert_eq!(
            info.timestamp, 0,
            "zero is grammers' 'no date', and the domain counts from the same epoch"
        );
        assert!(!info.is_outgoing);
        assert_eq!(
            info.reply_to_msg_id, None,
            "a message that answers nothing carries no reply target"
        );
    }
}
