//! Read messages on disk: the history file beside the configuration.
//!
//! The messages of each private conversation, and the chat list they were read
//! from, so a launch can paint the list and a conversation before the wire has
//! answered. It lives here for the drafts
//! file's reason: `domain` and `tui` own no filesystem, and this is the crate
//! that owns the configuration path. What crosses out of it is plain
//! [`domain::message::Message`] and [`domain::chat::Chat`] values, never the
//! file's own rows.
//!
//! **Bodies, not bytes.** A row is a message's text and the *kind* of
//! attachment it carries — the `[image]` token, not the image. Media bytes
//! belong to a media cache of their own, and a history file that held them
//! would be that cache under another name and a budget nobody measured.
//!
//! **Sealed records.** Each message is an authenticated envelope in a
//! per-account SQLite database restricted to its owner and tagged with the
//! account. The address a record is filed under is carried inside its own
//! plaintext and checked on read, so a row cannot be moved or replayed under
//! another address.
//!
//! Every operation is best-effort and launch-safe, the drafts file's
//! discipline: a missing file is an empty cache, an unreadable one is warned
//! about and forgotten (the next save overwrites it), and nothing here aborts
//! the run or fails a logout — diagnostics go to `tracing`, never the
//! terminal, which this program is drawing on.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::{Result, anyhow};
use domain::chat::{Chat, ChatKind};
use domain::history::{PINNED_IDS, REACTION_ENTRIES};
use domain::message::{MediaKind, Message, MessageStatus, Reaction, ReactionCount};
use domain::updates::UpdateEvent;
use hmac::{Hmac, Mac};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use telegram_framework::{FileKey, KeyProvider, SessionError, sealed};

/// How many messages of one conversation the in-memory window keeps.
///
/// The screen shows one window of 200 messages, and a cache hit fills it with
/// no wire follow-up. The archive keeps every message it is given: this bounds
/// the memory copy, not what is stored. Named apart from a
/// `CONVERSATION_WINDOW` because the two are different promises — one is a
/// window on the screen, the other is what is on disk — and only the screen
/// needs them equal.
pub(crate) const HISTORY_CACHE_DEPTH: usize = 200;

/// How many conversations' read marks the loop keeps, as a working set.
///
/// Read marks are names, not stored messages: thirty-two is more conversations
/// than a reader moves between in a sitting. Messages of every conversation
/// are kept in the archive, with no bound here.
pub(crate) const HISTORY_CACHE_PEERS: usize = 32;

/// The encoded text budget the worst-case measurement is filled to.
///
/// No longer a bound on what is stored: nothing in the archive is evicted, and
/// the configured limit in `Config` is what stops a download. It remains the
/// number the worst-case write test fills to, so a change in the cost of a
/// message is measured rather than silently absorbed. See [`encoded_len`] for
/// how a byte is counted.
#[allow(dead_code)]
pub(crate) const HISTORY_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// How many conversations of the chat list the file keeps.
///
/// The list as the wire last showed it is what a launch draws before the wire
/// has answered, so the file holds the head of it: pinned first, then newest,
/// the order it is kept in. Five hundred is more conversations than a screen
/// scrolls through before the fetched list replaces them, and a cap at all is
/// what gives the file a worst case — the fetched list itself has none.
pub(crate) const HISTORY_CACHE_CHATS: usize = 500;

/// The per-account archive beside the configuration.
///
/// The path handed in is the legacy `*.history.json` of [`crate::accounts`];
/// the archive is its `.db` sibling, computed once here and threaded to the
/// loop — passed rather than derived at each use, for the reason
/// [`DraftFile`](crate::draft_store::DraftFile) is: the load, the save and the
/// logout clear cannot disagree about which file they mean. The legacy path is
/// kept only so a launch can warn about it once; it is never read or deleted.
#[derive(Debug, Clone)]
pub(crate) struct HistoryFile {
    path: PathBuf,
    legacy: PathBuf,

    /// The account key source, shared by every clone. It opens the sealed
    /// `archive_key` in `meta`; records, state and chips are sealed under that
    /// key instead, so a passphrase change never has to touch a row. Held
    /// behind a mutex so a write and a re-seal cannot interleave: the swap to
    /// a new provider and the meta rows it seals must be one observable step.
    provider: Arc<Mutex<Arc<dyn KeyProvider>>>,

    /// Set when a present archive could not be read. A write then has nothing
    /// it may replace: an ingest would stack new rows on a file whose old rows
    /// are unknown, and a full rewrite would drop them. Only sign-out, which
    /// removes the archive, clears it.
    poisoned: Arc<AtomicBool>,
}

/// What [`HistoryFile::load`] found: the stored account tags and the cache.
///
/// `account_id` is the signed-in user id the file was written under, and
/// `account` the configured phone of files written before that tag existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedHistory {
    pub account: Option<String>,
    pub account_id: Option<i64>,
    pub cache: HistoryCache,
}

/// The account a history file is tagged with, as the loop holds it.
///
/// An id is the tag; the phone is the legacy tag of files written before the
/// id was known, read but written only while no id is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryTag {
    pub account_id: Option<i64>,
    pub phone: Option<String>,
}

impl HistoryTag {
    /// Whether the cache this tag names may stay once `Ready` names the
    /// signed-in account: an id must equal the live id; a legacy phone must
    /// equal the live phone, and passes when the live one is unknown, as does
    /// an untagged file. Absence is never a mismatch.
    pub(crate) fn holds_for(&self, user_id: i64, phone: Option<&str>) -> bool {
        match (self.account_id, self.phone.as_deref(), phone) {
            (Some(id), _, _) => id == user_id,
            (None, Some(stored), Some(live)) => stored == live,
            _ => true,
        }
    }

    /// Whether a write tagged `self` still belongs to the cache the live tag
    /// names, so a failure of that write may hand its pages back to it: an id
    /// must equal the live id, and a write with an id never belongs to a cache
    /// without one; a phone must equal the live phone, and passes when either
    /// is unknown, as does an untagged write. Absence is never a mismatch.
    pub(crate) fn tags_the_same_account_as(&self, live: &HistoryTag) -> bool {
        match (self.account_id, live.account_id) {
            (Some(stored), Some(id)) => stored == id,
            (Some(_), None) => false,
            (None, _) => self
                .phone
                .as_deref()
                .is_none_or(|stored| live.phone.as_deref().is_none_or(|label| stored == label)),
        }
    }
}

impl LoadedHistory {
    /// Nothing stored, and nothing stored under: a missing file, an
    /// unreadable one, or any other read failure.
    fn empty() -> Self {
        Self {
            account: None,
            account_id: None,
            cache: HistoryCache::default(),
        }
    }
}

/// The cached messages, per private peer, oldest first.
///
/// Keyed by the bare peer id, the way the drafts are: the product shows only
/// private conversations, and deciding which peers those are is the caller's
/// business — the chat list has already vetted them by the time a window is
/// open. Every peer holds at least one message and none of them is a local
/// placeholder; an empty peer is no entry rather than an empty one. Nothing
/// held here is dropped to make room — a full archive stops older downloads
/// instead.
///
/// A `BTreeMap` for the drafts file's reason: the same cache always
/// serialises to the same bytes.
#[derive(Debug, Clone, Default)]
pub(crate) struct HistoryCache {
    peers: BTreeMap<i64, Vec<CachedMessage>>,

    /// What this session's update feed has said about the cached runs.
    feed: FeedMarks,

    /// The reader's own names for people, keyed by peer id.
    ///
    /// In the same account-tagged file as the chats, so sign-out takes them
    /// with the history they describe.
    aliases: BTreeMap<i64, String>,

    /// Reaction strips per peer, keyed by message id, oldest entry first;
    /// at most [`REACTION_ENTRIES`] per peer.
    reactions: BTreeMap<i64, Vec<(i64, Vec<CachedReaction>)>>,

    /// Pinned message ids per peer, oldest pin first; at most
    /// [`PINNED_IDS`] per peer.
    pinned: BTreeMap<i64, Vec<i64>>,

    /// The chat list as the wire last showed it, head first, at most
    /// [`HISTORY_CACHE_CHATS`] long.
    ///
    /// In the history file rather than a file of its own: one account tag
    /// guards both, and one removal on sign-out takes both — a list kept
    /// apart would be a second place for another account's conversations to
    /// survive.
    chats: Vec<CachedChat>,

    /// Conversations the reader deleted, carried to the next save so an ingest
    /// write can drop their rows. A peer with rows in the window is never in
    /// this set: a deleted conversation that gets a new message is alive again.
    removed: BTreeSet<i64>,

    /// Identifiers of messages the reader deleted, carried to the next save.
    ///
    /// A deletion names no chat, but private message identifiers are one
    /// sequence, so these are applied to every chat at save time. Without them
    /// a save would upsert the surviving window over a row the reader had
    /// removed, and the message would come back on the next launch.
    deleted_messages: BTreeSet<i64>,

    /// New words of messages edited in each chat, carried to the next save.
    ///
    /// The text travels with the id so an edit of a row the window no longer
    /// holds can still be sealed on its own stored envelope.
    updated_messages: BTreeMap<i64, BTreeMap<i64, String>>,
}

/// Two caches are equal when they would write the same file: the feed marks
/// and the touches belong to the session that made them, are never written,
/// and a cache loaded from disk has neither.
impl PartialEq for HistoryCache {
    fn eq(&self, other: &Self) -> bool {
        self.peers == other.peers
            && self.chats == other.chats
            && self.aliases == other.aliases
            && self.reactions == other.reactions
            && self.pinned == other.pinned
    }
}

impl Eq for HistoryCache {}

/// What the update feed has said about each peer's cached run since it was
/// last whole, which is what decides whether a new message may join the run.
///
/// **Memory only.** A mark is a claim about a feed — that nothing it carried
/// for the peer has been missed — and the next launch has a different feed.
#[derive(Debug, Clone, Default)]
struct FeedMarks {
    /// The peers whose cached run is known to end at their newest message: a
    /// newest page has landed for each since the feed was last interrupted,
    /// reaching at least as far as anything [`FeedMarks::seen`] holds.
    current: BTreeSet<i64>,

    /// The newest identifier each peer has been seen to reach outside a page
    /// — an arrival over the feed, or a send the server numbered — whether or
    /// not it was cached.
    ///
    /// What lets a page be told apart from one the feed has overtaken: a page
    /// fetched just before a message arrived does not carry it, and a run it
    /// lands as would end short of the newest while looking current. Kept
    /// across an interruption, because a message seen is a message that
    /// exists whichever feed carried it.
    seen: BTreeMap<i64, i64>,
}

/// Bytes `text` takes inside a JSON string, quotes excluded: what `serde_json`
/// writes. `"` and `\` cost 2, as do the short escapes `\n \r \t` and the
/// backspace and form feed; every other control character below U+0020 costs 6
/// (`\u00XX`); anything else costs its UTF-8 length. Escaping only ever
/// inflates, so counting this and not `str::len` bounds the serialised copy.
fn encoded_len(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            c if c < '\u{20}' => 6,
            c => c.len_utf8(),
        })
        .sum()
}

impl HistoryCache {
    /// The messages cached for `peer`, oldest first, as the domain's own type.
    ///
    /// Empty for a peer with nothing cached.
    #[must_use]
    pub(crate) fn get(&self, peer: i64) -> Vec<Message> {
        self.peers.get(&peer).map_or_else(Vec::new, |rows| {
            rows.iter().map(|row| row.to_message(peer)).collect()
        })
    }

    /// The cached chat list, head first, as the domain's own type.
    ///
    /// Empty when nothing has been cached, which is a cold launch. Every chat
    /// comes back without a presence: a peer's status is stale the moment it
    /// is read back, and the feed says the current one soon enough.
    #[must_use]
    pub(crate) fn chats(&self) -> Vec<Chat> {
        self.chats.iter().map(CachedChat::to_chat).collect()
    }

    /// Makes the cached chat list `chats`, and says whether it changed.
    ///
    /// Only private conversations, and only the first
    /// [`HISTORY_CACHE_CHATS`] of them: what the screen shows, to the file's
    /// bound. Compared before anything is copied, because the loop asks this
    /// every pass and the answer is almost always no.
    pub(crate) fn set_chats(&mut self, chats: &[Chat]) -> bool {
        let listed = chats
            .iter()
            .filter(|chat| chat.is_private())
            .take(HISTORY_CACHE_CHATS);
        if self.chats.len() == listed.clone().count()
            && self
                .chats
                .iter()
                .zip(listed.clone())
                .all(|(cached, chat)| cached.matches(chat))
        {
            return false;
        }
        self.chats = listed.map(CachedChat::from_chat).collect();
        true
    }

    /// Replaces what is cached for `peer` with `messages`, oldest first.
    ///
    /// Not a merge: whatever `peer` held before is gone. Three things are
    /// dropped on the way in. Placeholders — an identifier at or below zero is
    /// a send the server has not numbered, which a relaunch cannot confirm or
    /// retry, so caching one would resurrect a message that may never have
    /// gone. Messages naming another conversation, because a message names its
    /// own and the key must not disagree with it. And everything older than
    /// the newest [`HISTORY_CACHE_DEPTH`], so the oldest go first, the way the
    /// window drops its far end.
    pub(crate) fn put<'a>(&mut self, peer: i64, messages: impl IntoIterator<Item = &'a Message>) {
        let mut rows: Vec<CachedMessage> = messages
            .into_iter()
            .filter(|message| message.id > 0 && message.chat_id == peer)
            .map(CachedMessage::from_message)
            .collect();
        let excess = rows.len().saturating_sub(HISTORY_CACHE_DEPTH);
        rows.drain(..excess);

        if rows.is_empty() {
            self.remove(peer);
        } else {
            self.peers.insert(peer, rows);
        }
    }

    /// Forgets everything cached for `peer`, and names it to the next save so
    /// the rows in the archive go with it.
    pub(crate) fn remove(&mut self, peer: i64) {
        self.peers.remove(&peer);
        self.aliases.remove(&peer);
        self.reactions.remove(&peer);
        self.pinned.remove(&peer);
        self.removed.insert(peer);
    }

    /// The reader's names, as a borrowed map.
    #[must_use]
    pub(crate) fn aliases(&self) -> &BTreeMap<i64, String> {
        &self.aliases
    }

    /// Stores or clears one local name, saying whether the file changed.
    pub(crate) fn set_alias(&mut self, peer: i64, name: Option<&str>) -> bool {
        match name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => self
                .aliases
                .insert(peer, name.to_owned())
                .is_none_or(|old| old != name),
            None => self.aliases.remove(&peer).is_some(),
        }
    }

    /// The JSON-encoded bytes of message text held, over every cached peer.
    ///
    /// Kept as the measure the worst-case tests fill to; see
    /// [`HISTORY_CACHE_BYTES`]. The on-disk fullness answer is
    /// [`HistoryFile::text_bytes`], which reads the archive rather than this
    /// in-memory copy. Summed on demand so there is no counter to drift from
    /// the rows.
    #[allow(dead_code)]
    fn encoded_text_bytes(&self) -> usize {
        self.peers
            .values()
            .flatten()
            .map(|row| encoded_len(&row.text))
            .sum()
    }

    /// Folds a page the wire answered with into what is cached for `peer`,
    /// and says whether anything changed.
    ///
    /// What is cached for a peer is one unbroken run of numbered messages,
    /// oldest first, at most one per id, ending at the newest message the wire
    /// has shown — so a cache hit is a window that needs no gap filled. Every
    /// rule below keeps that true; a page that cannot be folded in without
    /// breaking it is ignored, because a gap the cache cannot see is one it
    /// would paint over.
    ///
    /// **What a page speaks for.** A page is the whole of the stretch of
    /// history it was fetched from, so every cached row inside that stretch
    /// that the page does not carry has been deleted, and every one it does
    /// carry is replaced by the page's copy, which may be an edit:
    ///
    /// * [`PageKind::Latest`] speaks for everything from its oldest message
    ///   up — anything newer than its newest is gone too — and, when it was
    ///   shorter than asked for, for the whole conversation.
    /// * [`PageKind::Older`] speaks for its oldest message up to, not
    ///   including, the anchor it was counted from; when short, for everything
    ///   below the anchor.
    /// * [`PageKind::Newer`] the same the other way: just above its anchor up
    ///   to its newest message, or everything above the anchor when short.
    /// * [`PageKind::Around`] speaks for its oldest to its newest message,
    ///   and nothing past them: whether it was short says nothing about which
    ///   end ran out.
    ///
    /// **When it may be folded in.** Only when that stretch overlaps the cached
    /// run or, for an anchored page, starts from an anchor inside it — the
    /// anchors are exclusive, so a page counted from the cached oldest message
    /// is contiguous with the run without sharing a row with it. Ids alone
    /// cannot prove two stretches touch: a conversation's ids are not dense.
    ///
    /// * A latest page that does not overlap the run *replaces* it: the newest
    ///   end moved on past a gap nobody fetched, and the newest end is what
    ///   the cache is for. With nothing cached, a latest page is the cache.
    /// * Any other page that does not reach the run is ignored, as is any
    ///   other page while nothing is cached: the cache holds the newest end
    ///   only, and a page from somewhere else cannot be shown to join it.
    ///
    /// Then the newest [`HISTORY_CACHE_DEPTH`] are kept, so an older page past
    /// the cap falls off the far end as the window's would.
    ///
    /// Placeholders and rows naming another conversation are dropped first,
    /// for [`HistoryCache::put`]'s reasons, and a page with nothing left — an
    /// empty one included — changes nothing: an empty answer is as likely to
    /// be a fetch that short-circuited as a conversation that was cleared, and
    /// wiping the cache on the strength of it would throw away a launch's
    /// head start for nothing.
    ///
    /// **What a page says about the feed.** A latest page that lands is what
    /// makes `peer`'s run current for [`HistoryCache::arrive`] — unless the
    /// feed has already shown a message newer than the run now ends at, in
    /// which case the page was fetched before that message and any page that
    /// leaves the run there takes the mark away: the message is real, the
    /// run no longer holds it, and a later arrival appended past it would hide
    /// the hole.
    pub(crate) fn merge(&mut self, peer: i64, page: &[Message], kind: PageKind) -> bool {
        let mut fresh: Vec<CachedMessage> = page
            .iter()
            .filter(|message| message.id > 0 && message.chat_id == peer)
            .map(CachedMessage::from_message)
            .collect();
        fresh.sort_by_key(|row| row.id);
        fresh.dedup_by_key(|row| row.id);
        let (Some(first), Some(last)) = (fresh.first(), fresh.last()) else {
            return false;
        };
        let (first, last) = (first.id, last.id);

        // The ids the page speaks for, inclusive, and how far it reaches: the
        // stretch plus the anchor it was counted from, which is what lets a
        // page that shares no row with the run still be shown to join it.
        let (low, high, reach) = match kind {
            PageKind::Latest { whole } => (
                if whole { i64::MIN } else { first },
                i64::MAX,
                (first, i64::MAX),
            ),
            PageKind::Older {
                before,
                reached_start,
            } => (
                if reached_start { i64::MIN } else { first },
                before.saturating_sub(1),
                (first, before),
            ),
            PageKind::Newer { after, reached_end } => (
                after.saturating_add(1),
                if reached_end { i64::MAX } else { last },
                (after, last),
            ),
            PageKind::Around => (first, last, (first, last)),
        };

        let cached = self.peers.get(&peer);
        let joins = cached.and_then(|rows| Some((rows.first()?.id, rows.last()?.id)));
        let mut merged = match (cached, joins) {
            (Some(rows), Some((oldest, newest))) if reach.0 <= newest && reach.1 >= oldest => {
                // The page first, so that where an id is in both — which only
                // a page carrying rows outside its own stretch could cause —
                // the stable sort and the dedupe keep the wire's copy.
                let kept = rows.iter().filter(|row| row.id < low || row.id > high);
                let mut merged = fresh;
                merged.extend(kept.cloned());
                merged.sort_by_key(|row| row.id);
                merged.dedup_by_key(|row| row.id);
                merged
            }
            _ if matches!(kind, PageKind::Latest { .. }) => fresh,
            _ => return false,
        };
        let excess = merged.len().saturating_sub(HISTORY_CACHE_DEPTH);
        merged.drain(..excess);

        // Decided before the no-change answer below: a latest page identical
        // to the run still shows the run is current.
        if let Some(newest) = merged.last().map(|row| row.id) {
            if self.feed.seen.get(&peer).is_some_and(|&seen| newest < seen) {
                self.feed.current.remove(&peer);
            } else if matches!(kind, PageKind::Latest { .. }) {
                self.feed.current.insert(peer);
            }
        }

        let unchanged = cached == Some(&merged);
        if unchanged {
            return false;
        }
        self.peers.insert(peer, merged);
        true
    }

    /// Folds an event from the update feed into what is cached, and says
    /// whether anything changed.
    ///
    /// Each rule keeps [`HistoryCache::merge`]'s promise — one unbroken run
    /// per peer, ending at its newest message:
    ///
    /// * An edit replaces the text of a cached message where it stands, and
    ///   is ignored for one that is not cached: it moves nothing.
    /// * A deletion removes the messages from **every** peer, because it
    ///   names none — [`UpdateEvent::MessagesDeleted`] says why it cannot —
    ///   and the identifiers of private conversations are one sequence, so
    ///   they cannot match another peer's messages. A run with messages taken
    ///   out of it has no hole the conversation does not have.
    /// * A new message goes through [`HistoryCache::arrive`], which adds it
    ///   only to a run known to be current.
    ///
    /// The rest change nothing a row holds: a read receipt is a watermark on
    /// the conversation, and a row's status is its direction alone; typing
    /// and presence are not messages at all.
    pub(crate) fn apply_update(&mut self, event: &UpdateEvent) -> bool {
        match event {
            UpdateEvent::NewMessage(message) => self.arrive(message),
            UpdateEvent::MessageEdited {
                chat_id,
                message_id,
                new_text,
            } => self.edit(*chat_id, *message_id, new_text),
            UpdateEvent::MessagesDeleted { message_ids } => self.delete(message_ids),
            UpdateEvent::ReactionsChanged {
                chat_id,
                message_id,
                reactions,
            } => self.set_reactions(*chat_id, *message_id, reactions),
            UpdateEvent::PinChanged {
                chat_id,
                message_ids,
                pinned,
            } => self.set_pins(*chat_id, message_ids, *pinned),
            // A read receipt is a watermark on the conversation, and a row's
            // status is its direction alone; typing and presence are not
            // messages at all.
            UpdateEvent::ReadReceipt { .. }
            | UpdateEvent::InboxRead { .. }
            | UpdateEvent::PeerTyping { .. }
            | UpdateEvent::PeerStatus { .. } => false,
        }
    }

    /// Replaces the reaction strip of one message, keeping the bounded
    /// per-peer side table in the order the updates arrived.
    fn set_reactions(&mut self, peer: i64, message_id: i64, reactions: &[ReactionCount]) -> bool {
        let fresh: Vec<CachedReaction> = reactions.iter().map(CachedReaction::from).collect();
        let rows = self.reactions.entry(peer).or_default();
        let held = rows.iter().position(|(id, _)| *id == message_id);
        let changed = match held {
            Some(at) if rows[at].1 == fresh => false,
            Some(at) if fresh.is_empty() => {
                rows.remove(at);
                true
            }
            Some(at) => {
                rows[at].1 = fresh;
                true
            }
            None if fresh.is_empty() => false,
            None => {
                rows.push((message_id, fresh));
                true
            }
        };
        if rows.len() > REACTION_ENTRIES {
            rows.drain(..rows.len() - REACTION_ENTRIES);
        }
        changed
    }

    /// Adds or removes pinned ids for `peer`, keeping the bounded set.
    fn set_pins(&mut self, peer: i64, message_ids: &[i64], pinned: bool) -> bool {
        let rows = self.pinned.entry(peer).or_default();
        let mut changed = false;
        for &id in message_ids {
            if pinned {
                if !rows.contains(&id) {
                    rows.push(id);
                    changed = true;
                }
            } else if let Some(at) = rows.iter().position(|held| *held == id) {
                rows.remove(at);
                changed = true;
            }
        }
        if rows.len() > PINNED_IDS {
            rows.drain(..rows.len() - PINNED_IDS);
        }
        changed
    }

    /// Adds a message that has just come into being — an arrival over the
    /// feed, or a send the server has numbered — and says whether anything
    /// changed.
    ///
    /// **Only to a current run.** A run loaded from disk, or one the feed has
    /// been interrupted behind, may end well short of the conversation's
    /// newest message, and appending past that end would hide everything in
    /// between: yesterday's run ending at 100 with 150 added is a cache that
    /// claims 101 to 149 never existed. Such a run is left as it is, stale but
    /// unbroken, and the next latest page brings it up to date. Either way the
    /// message is noted as seen, which is what keeps a page fetched before it
    /// from marking the run current.
    ///
    /// Inserted where its identifier puts it rather than appended, because a
    /// send's answer and the feed travel apart and either can come first; one
    /// already cached is replaced by this copy, and one older than the whole
    /// run is outside it and ignored. Placeholders never enter, for
    /// [`HistoryCache::put`]'s reason, and the newest [`HISTORY_CACHE_DEPTH`]
    /// are kept.
    pub(crate) fn arrive(&mut self, message: &Message) -> bool {
        if message.id <= 0 {
            return false;
        }
        let peer = message.chat_id;
        let seen = self.feed.seen.entry(peer).or_insert(message.id);
        *seen = (*seen).max(message.id);
        if !self.feed.current.contains(&peer) {
            return false;
        }

        let row = CachedMessage::from_message(message);
        let rows = self.peers.entry(peer).or_default();
        match rows.binary_search_by_key(&row.id, |cached| cached.id) {
            Ok(at) if rows[at] == row => return false,
            Ok(at) => rows[at] = row,
            Err(0) if !rows.is_empty() => return false,
            Err(at) => rows.insert(at, row),
        }
        let excess = rows.len().saturating_sub(HISTORY_CACHE_DEPTH);
        rows.drain(..excess);
        true
    }

    /// Replaces the text of message `id` of `peer`, recording the new words
    /// even when the row is outside the window.
    ///
    /// False for an unchanged cached row, so the caller does not churn, and for
    /// an uncached row, whose edit is owed to the archive but changes no cache.
    fn edit(&mut self, peer: i64, id: i64, text: &str) -> bool {
        let mut changed = false;
        if let Some(row) = self
            .peers
            .get_mut(&peer)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == id))
        {
            if row.text == text {
                return false;
            }
            text.clone_into(&mut row.text);
            changed = true;
        }
        self.updated_messages
            .entry(peer)
            .or_default()
            .insert(id, text.to_owned());
        changed
    }

    /// Takes the messages `ids` out of every peer.
    ///
    /// A current run that loses the newest message the feed showed is still
    /// current — the deletion is the conversation's own — so what it now ends
    /// at becomes the newest seen, or a later page ending there would read as
    /// one the feed had overtaken.
    fn delete(&mut self, ids: &[i64]) -> bool {
        let FeedMarks { current, seen } = &mut self.feed;
        let mut changed = false;
        // Carry even ids the window no longer holds: a row outside the window
        // still has to be dropped from the archive at the next save. The caller
        // marks the cache dirty for a deletion even when no cached row went.
        self.deleted_messages.extend(ids.iter().copied());
        self.peers.retain(|peer, rows| {
            let before = rows.len();
            rows.retain(|row| !ids.contains(&row.id));
            if rows.len() != before {
                changed = true;
                if current.contains(peer) && seen.get(peer).is_some_and(|id| ids.contains(id)) {
                    match rows.last() {
                        Some(row) => seen.insert(*peer, row.id),
                        None => seen.remove(peer),
                    };
                }
            }
            !rows.is_empty()
        });
        changed
    }

    /// Forgets which runs are current, because the feed may have missed
    /// something: it ended, it read past a failure, or the client under it was
    /// replaced. Every run is then stale until its next latest page.
    pub(crate) fn feed_interrupted(&mut self) {
        self.feed.current.clear();
    }
}

/// Which stretch of a conversation a page was fetched from, as
/// [`HistoryCache::merge`] needs to know it.
///
/// The fetch's own terms, restated so this module names no `proto` type: the
/// anchors are the ids a page was counted from, exclusive, the way the wire
/// counts them, and the flags are whether the page came back shorter than
/// asked for — the rule the cursor uses to call a direction exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageKind {
    /// The newest page; `whole` when it was short, so it is the whole
    /// conversation.
    Latest { whole: bool },

    /// The page just older than `before`; `reached_start` when it was short.
    Older { before: i64, reached_start: bool },

    /// The page just newer than `after`; `reached_end` when it was short.
    Newer { after: i64, reached_end: bool },

    /// A page centred on a message the reader jumped to.
    Around,
}

/// The file payload:
/// `{ "account": <phone|null>, "account_id": <user id|null>, "peers": { "<peer_id>": [<row>, …] },
/// "chats": [<chat>, …] }`.
///
/// `chats` is left out while empty, and a file without it — one written
/// before the list was cached — loads with no list rather than not at all.
#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    #[serde(default)]
    account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<i64>,
    #[serde(default)]
    peers: BTreeMap<i64, Vec<CachedMessage>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chats: Vec<CachedChat>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    aliases: BTreeMap<i64, String>,
    /// The bounded reaction/pin side tables, held in their own sealed SQL
    /// table rather than in a message's record.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    chips: BTreeMap<i64, Chips>,
    /// Chats the reader deleted. An ingest write drops only the rows it names,
    /// so a removal has to travel with the window.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    removed: BTreeSet<i64>,
    /// Messages the reader deleted, even ones the window no longer holds.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    deleted_messages: BTreeSet<i64>,
    /// Messages edited in each chat, with their new words carried alongside
    /// the ids so a row outside `peers` can be re-sealed too.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    updated_messages: BTreeMap<i64, BTreeMap<i64, String>>,
}

/// One conversation of the cached chat list: what its row draws.
///
/// No kind, because only private conversations are cached and that is the
/// one kind a row is read back as. No presence, because a status read back
/// from a file is a claim about a moment that has passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Cached Telegram flags remain independent.
struct CachedChat {
    id: i64,
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_message: Option<String>,
    #[serde(default)]
    unread_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_message_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_timestamp: Option<i64>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    unread_mark: bool,
    /// Missing from caches written before the flag existed: those read back as
    /// live accounts until the next fetch says otherwise.
    #[serde(default)]
    deleted: bool,
}

impl CachedChat {
    fn from_chat(chat: &Chat) -> Self {
        Self {
            id: chat.id,
            title: chat.title.clone(),
            last_message: chat.last_message.as_deref().map(str::to_owned),
            unread_count: chat.unread_count,
            last_message_id: chat.last_message_id,
            last_timestamp: chat.last_timestamp,
            pinned: chat.pinned,
            muted: chat.muted,
            unread_mark: chat.unread_mark,
            deleted: chat.deleted,
        }
    }

    fn to_chat(&self) -> Chat {
        Chat {
            read_outbox_max_id: None,
            id: self.id,
            title: self.title.clone(),
            kind: ChatKind::Private,
            last_message: self.last_message.clone().map(Cow::Owned),
            unread_count: self.unread_count,
            last_message_id: self.last_message_id,
            last_timestamp: self.last_timestamp,
            pinned: self.pinned,
            muted: self.muted,
            unread_mark: self.unread_mark,
            presence: None,
            deleted: self.deleted,
        }
    }

    /// Whether `chat` would be cached as this row: every field the row keeps,
    /// compared in place.
    fn matches(&self, chat: &Chat) -> bool {
        self.id == chat.id
            && self.title == chat.title
            && self.last_message.as_deref() == chat.last_message.as_deref()
            && self.unread_count == chat.unread_count
            && self.last_message_id == chat.last_message_id
            && self.last_timestamp == chat.last_timestamp
            && self.pinned == chat.pinned
            && self.muted == chat.muted
            && self.unread_mark == chat.unread_mark
            && self.deleted == chat.deleted
    }
}

/// One cached message: what the conversation view draws, and nothing else.
///
/// The peer is the map key rather than a field, so a row cannot name a
/// conversation other than the one it is filed under. The status is not
/// stored either: only numbered messages are cached, and a numbered message
/// is `Sent` or `Received` by its direction alone — the rule `proto` applies
/// to everything that arrives over the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedMessage {
    id: i64,
    text: String,
    timestamp: i64,
    is_outgoing: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    is_service: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply_to: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media: Option<CachedMedia>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_id: Option<i64>,
}

impl CachedMessage {
    fn from_message(message: &Message) -> Self {
        Self {
            id: message.id,
            text: message.text.clone().into_owned(),
            timestamp: message.timestamp,
            is_outgoing: message.is_outgoing,
            is_service: message.is_service,
            reply_to: message.reply_to,
            media: message.media.map(CachedMedia::from),
            media_id: message.media_id,
        }
    }

    fn to_message(&self, peer: i64) -> Message {
        Message {
            id: self.id,
            chat_id: peer,
            text: Cow::Owned(self.text.clone()),
            timestamp: self.timestamp,
            status: if self.is_outgoing {
                MessageStatus::Sent
            } else {
                MessageStatus::Received
            },
            is_outgoing: self.is_outgoing,
            is_service: self.is_service,
            reply_to: self.reply_to,
            media: self.media.map(MediaKind::from),
            media_id: self.media_id,
        }
    }
}

/// [`MediaKind`] as the file spells it.
///
/// A mirror rather than a derive on the domain type: `domain` carries no
/// `serde`, and the file's spelling is this module's promise, not the model's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CachedMedia {
    Photo,
    Video,
    Gif,
    Voice,
    Sticker,
    AnimatedSticker,
    File,
}

impl From<MediaKind> for CachedMedia {
    fn from(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Photo => Self::Photo,
            MediaKind::Video => Self::Video,
            MediaKind::Gif => Self::Gif,
            MediaKind::Voice => Self::Voice,
            MediaKind::Sticker => Self::Sticker,
            MediaKind::AnimatedSticker => Self::AnimatedSticker,
            MediaKind::File => Self::File,
        }
    }
}

impl From<CachedMedia> for MediaKind {
    fn from(kind: CachedMedia) -> Self {
        match kind {
            CachedMedia::Photo => Self::Photo,
            CachedMedia::Video => Self::Video,
            CachedMedia::Gif => Self::Gif,
            CachedMedia::Voice => Self::Voice,
            CachedMedia::Sticker => Self::Sticker,
            CachedMedia::AnimatedSticker => Self::AnimatedSticker,
            CachedMedia::File => Self::File,
        }
    }
}

impl HistoryFile {
    /// A handle on the archive for the account whose legacy path is `path`,
    /// sealed under `keys`.
    #[must_use]
    pub(crate) fn new(path: PathBuf, keys: Arc<dyn KeyProvider>) -> Self {
        Self {
            path: archive_path(&path),
            legacy: path,
            provider: Arc::new(Mutex::new(keys)),
            poisoned: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Points every clone at `keys` for the next open, without touching the
    /// archive. Used when bring-up learns the passphrase the session already
    /// uses: no meta envelope needs re-sealing, but the provider that opens
    /// the one on disk must be the one the reader typed.
    pub(crate) fn set_provider(&self, keys: Arc<dyn KeyProvider>) {
        *lock(&self.provider) = keys;
    }

    /// The provider held right now, for the callers that open a store.
    #[cfg(test)]
    fn keys(&self) -> Arc<dyn KeyProvider> {
        Arc::clone(&*lock(&self.provider))
    }

    /// Locks the shared provider and hands out a copy, so the caller can keep
    /// the guard until its store work is committed and a re-seal cannot land
    /// between the two.
    fn locked_keys(&self) -> (MutexGuard<'_, Arc<dyn KeyProvider>>, Arc<dyn KeyProvider>) {
        let guard = lock(&self.provider);
        let keys = Arc::clone(&guard);
        (guard, keys)
    }

    /// Moves the account-sealed `archive_key` to `to`, saying why when it could
    /// not. Records, state and chips are sealed under the archive key itself, so
    /// nothing but these two `meta` rows changes: a save that committed a
    /// moment ago is still there, and one that lands after this sees `to`.
    ///
    /// The previous envelope is kept as `archive_key_prev` until
    /// [`HistoryFile::confirm_rekey`] says the session file has been saved
    /// under `to`. A kill between the two leaves the session under the old
    /// passphrase and the archive under the new one; the next start opens
    /// `archive_key_prev` and puts meta back.
    pub(crate) fn reseal(&self, to: Arc<dyn KeyProvider>) -> Result<(), String> {
        let (mut guard, current) = self.locked_keys();
        if !self.path.is_file() {
            *guard = to;
            return Ok(());
        }
        let mut store = open_store(&self.path, &current)
            .map_err(|error| format!("history archive could not be read: {error:#}"))?;
        let envelope: Vec<u8> = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_ARCHIVE_KEY],
                |row| row.get(0),
            )
            .map_err(|error| format!("history archive could not be read: {error:#}"))?;
        let archive_key = open_archive_key(&envelope, current.as_ref())
            .map_err(|error| format!("history archive could not be read: {error:#}"))?;

        let prev_envelope = sealed::seal(&archive_key, current.as_ref())
            .map_err(|error| format!("history archive could not be re-sealed: {error:#}"))?;
        let next_envelope = sealed::seal(&archive_key, to.as_ref())
            .map_err(|error| format!("history archive could not be re-sealed: {error:#}"))?;
        let prev_plain = open_archive_key(&prev_envelope, current.as_ref())
            .map_err(|error| format!("history archive could not be re-sealed: {error:#}"))?;
        let next_plain = open_archive_key(&next_envelope, to.as_ref())
            .map_err(|error| format!("history archive could not be re-sealed: {error:#}"))?;
        if prev_plain != archive_key || next_plain != archive_key {
            return Err("history archive could not be re-sealed: a key did not round-trip".into());
        }

        let transaction = store
            .conn
            .transaction()
            .map_err(|error| format!("history archive could not be re-sealed: {error:#}"))?;
        transaction
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![META_ARCHIVE_KEY_PREV, prev_envelope],
            )
            .and_then(|_| {
                transaction.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![META_ARCHIVE_KEY, next_envelope],
                )
            })
            .and_then(|_| transaction.commit())
            .map_err(|error| format!("history archive could not be re-sealed: {error:#}"))?;
        drop(store);
        *guard = to;
        Ok(())
    }

    /// Drops `archive_key_prev` once the session is saved under the new
    /// passphrase. Under the provider lock, so no ordinary open can promote the
    /// old key back between the open and the delete. No archive: nothing to do.
    pub(crate) fn confirm_rekey(&self) -> Result<(), String> {
        let (_guard, keys) = self.locked_keys();
        if !self.path.is_file() {
            return Ok(());
        }
        let store = open_store(&self.path, &keys)
            .map_err(|error| format!("history archive could not be read: {error:#}"))?;
        store
            .conn
            .execute(
                "DELETE FROM meta WHERE key = ?1",
                params![META_ARCHIVE_KEY_PREV],
            )
            .map_err(|error| format!("history archive could not be confirmed: {error:#}"))?;
        Ok(())
    }

    /// A handle sealed under a fixed passphrase, so tests need no keyring.
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf) -> Self {
        Self::new(
            path,
            Arc::new(telegram_framework::PassphraseProvider::new("test")),
        )
    }

    /// Reads the stored cache and the account tag it was saved under.
    ///
    /// Only the newest [`HISTORY_CACHE_DEPTH`] messages of each chat are read
    /// out of SQL — the same bounded windows the cache holds — not the whole
    /// archive. The rest stays sealed on disk until a range read asks for it.
    ///
    /// A missing database is an empty cache, not an error; a database that
    /// cannot be opened, a record that is not sealed, and any record whose
    /// plaintext does not name the address it was filed under are warned about
    /// and read as empty. Only a missing database reads as empty without
    /// consequence: a failure poisons the handle, so no save can write over an
    /// archive whose contents are unknown until sign-out removes it. What a
    /// readable archive holds goes through
    /// [`HistoryCache::put`], so a hand-written or older archive is held to the
    /// same bounds as one this build wrote. The legacy `history.json`, if it
    /// still exists, is warned about once and left where it is — never read,
    /// migrated, or deleted.
    pub(crate) fn load(&self) -> LoadedHistory {
        if self.legacy.is_file() {
            warn_legacy_once();
        }
        match self.read_payload() {
            Ok(Some(payload)) => Self::from_payload(payload),
            Ok(None) => LoadedHistory::empty(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "history archive could not be read; it is left as it is and nothing is written over it"
                );
                self.poisoned.store(true, Ordering::Relaxed);
                LoadedHistory::empty()
            }
        }
    }

    /// The bounded windows of the archive as one payload, or `None` when it
    /// does not exist yet.
    ///
    /// The non-message state is one sealed blob in `meta`; the newest
    /// [`HISTORY_CACHE_DEPTH`] messages of each chat are read out of SQL and
    /// decoded and address-checked on the way out. Older rows stay on disk.
    fn read_payload(&self) -> Result<Option<Payload>> {
        if !self.path.is_file() {
            return Ok(None);
        }
        let (_guard, keys) = self.locked_keys();
        let store = open_store(&self.path, &keys)?;
        let state = store.state()?;
        let mut peers: BTreeMap<i64, Vec<CachedMessage>> = BTreeMap::new();
        for (chat_id, row) in store.bounded_records(HISTORY_CACHE_DEPTH)? {
            peers.entry(chat_id).or_default().push(row);
        }
        Ok(Some(Payload {
            account: state.account,
            account_id: state.account_id,
            peers,
            chats: state.chats,
            aliases: state.aliases,
            chips: store.chips()?,
            removed: BTreeSet::new(),
            deleted_messages: BTreeSet::new(),
            updated_messages: BTreeMap::new(),
        }))
    }

    /// The cache a payload holds, within the same bounds a save applies.
    fn from_payload(payload: Payload) -> LoadedHistory {
        let mut cache = HistoryCache::default();
        for (peer, rows) in payload.peers {
            let messages: Vec<Message> = rows.iter().map(|row| row.to_message(peer)).collect();
            cache.put(peer, &messages);
        }
        cache.chats = payload.chats;
        cache.chats.truncate(HISTORY_CACHE_CHATS);
        cache.aliases = payload.aliases;
        for (peer, chips) in payload.chips {
            if !chips.reactions.is_empty() {
                cache.reactions.insert(peer, chips.reactions);
            }
            if !chips.pinned.is_empty() {
                cache.pinned.insert(peer, chips.pinned);
            }
        }
        LoadedHistory {
            account: payload.account,
            account_id: payload.account_id,
            cache,
        }
    }

    /// The messages of one chat, read from its recorded ranges on demand.
    ///
    /// Unlike [`HistoryFile::load`], which reads only the bounded windows, this
    /// opens the archive, asks the ledger which stretches the chat holds and
    /// reads those rows, decoding and unsealing each on the way out. A missing
    /// or unreadable archive is an empty answer, the load discipline. Paging
    /// uses [`HistoryFile::page`], which decodes only one bounded page; this
    /// whole-chat read is kept for callers that genuinely need every row.
    #[allow(dead_code)]
    pub(crate) fn get(&self, peer: i64) -> Vec<Message> {
        if !self.path.is_file() {
            return Vec::new();
        }
        let (_guard, keys) = self.locked_keys();
        let Ok(store) = open_store(&self.path, &keys) else {
            return Vec::new();
        };
        let Ok(ranges) = store.ranges(peer) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (lo, hi) in ranges {
            match store.range_rows(peer, lo, hi) {
                Ok(mut part) => rows.append(&mut part),
                Err(error) => {
                    tracing::warn!(%error, "a history range could not be read");
                    return Vec::new();
                }
            }
        }
        rows.into_iter().map(|row| row.to_message(peer)).collect()
    }

    /// The next older page of `peer` strictly below `below`, oldest first, and
    /// the `lo_id` of the single recorded range it came from.
    ///
    /// Only the range that reaches the window — the stretch whose top is at or
    /// just below `below`, the highest such stretch whose own low end is also
    /// below `below` — is read, so paging never crosses a gap into an older
    /// range, and a newer range that starts at or above `below` cannot hide an
    /// older one that does reach below it. At most `limit` rows are decoded;
    /// everything else on disk is left untouched. `None` for the range means
    /// no range reaches `below`, so there is nothing contiguous to show offline.
    pub(crate) fn page(&self, peer: i64, below: i64, limit: usize) -> (Vec<Message>, Option<i64>) {
        if !self.path.is_file() {
            return (Vec::new(), None);
        }
        let (_guard, keys) = self.locked_keys();
        let Ok(store) = open_store(&self.path, &keys) else {
            return (Vec::new(), None);
        };
        let Ok(ranges) = store.ranges(peer) else {
            return (Vec::new(), None);
        };
        let reaching = ranges
            .iter()
            .filter(|(_, hi)| *hi >= below.saturating_sub(1))
            .filter(|(lo, _)| *lo < below)
            .max_by_key(|(_, hi)| *hi);
        let Some(&(lo, _)) = reaching else {
            return (Vec::new(), None);
        };
        match store.page(peer, below, lo, limit) {
            Ok(rows) => (
                rows.into_iter()
                    .map(|(_, row)| row.to_message(peer))
                    .collect(),
                Some(lo),
            ),
            Err(error) => {
                tracing::warn!(%error, "a history page could not be read");
                (Vec::new(), None)
            }
        }
    }

    /// The encoded text bytes held on disk, by a SQL read rather than a copy of
    /// every row in memory. `None` when the archive cannot be read.
    #[allow(dead_code)]
    pub(crate) fn text_bytes(&self) -> Option<usize> {
        if !self.path.is_file() {
            return None;
        }
        let (_guard, keys) = self.locked_keys();
        let store = open_store(&self.path, &keys).ok()?;
        store.text_bytes().ok()
    }

    /// The stretches of `peer` the archive holds, oldest first.
    ///
    /// SQL-backed, like [`HistoryFile::get`], so a caller can check what a
    /// fetch left behind without reading every row into memory.
    #[allow(dead_code)]
    pub(crate) fn ranges(&self, peer: i64) -> Vec<(i64, i64)> {
        if !self.path.is_file() {
            return Vec::new();
        }
        let (_guard, keys) = self.locked_keys();
        let Ok(store) = open_store(&self.path, &keys) else {
            return Vec::new();
        };
        store.ranges(peer).unwrap_or_default()
    }

    /// Whether the archive has reached `limit` bytes of message text.
    ///
    /// `0` means no limit, and an archive that cannot be read is not full: the
    /// next write either repairs it or the wire refills it. A limit is read
    /// from configuration, never fixed here.
    pub(crate) fn full(&self, limit: u64) -> bool {
        limit != 0 && self.text_bytes().is_some_and(|used| used as u64 >= limit)
    }

    /// Stores `cache` under the phone `account`: [`HistoryFile::encode`] then
    /// [`HistoryFile::write`], on one thread.
    ///
    /// The loop does not call this — it encodes where the cache lives and
    /// writes off the loop's thread — so only the tests that pin the archive's
    /// shape do.
    #[cfg(test)]
    pub(crate) fn save(&self, cache: &HistoryCache, account: Option<&str>) {
        if let Some(bytes) = Self::encode(
            cache,
            &HistoryTag {
                account_id: None,
                phone: account.map(str::to_owned),
            },
        ) {
            self.write(&bytes);
        }
    }

    /// The payload that stores `cache` under `tag`, or `None` when it could
    /// not be serialised, which is warned about.
    ///
    /// Apart from [`HistoryFile::write`] so the two can run on different
    /// threads: the snapshot is taken where the cache is, and only the disk
    /// work is handed off.
    pub(crate) fn encode(cache: &HistoryCache, tag: &HistoryTag) -> Option<Vec<u8>> {
        let payload = Payload {
            account: tag.phone.clone(),
            account_id: tag.account_id,
            peers: cache.peers.clone(),
            chats: cache.chats.clone(),
            aliases: cache.aliases.clone(),
            // Named even when empty: an ingest upsert of an empty side table
            // is what erases a cleared strip instead of resurrecting it on load.
            chips: cache
                .peers
                .keys()
                .chain(cache.reactions.keys())
                .chain(cache.pinned.keys())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|peer| {
                    (
                        *peer,
                        Chips {
                            reactions: cache.reactions.get(peer).cloned().unwrap_or_default(),
                            pinned: cache.pinned.get(peer).cloned().unwrap_or_default(),
                        },
                    )
                })
                .collect(),
            removed: cache
                .removed
                .iter()
                .filter(|peer| !cache.peers.contains_key(*peer))
                .copied()
                .collect(),
            deleted_messages: cache.deleted_messages.clone(),
            updated_messages: cache.updated_messages.clone(),
        };
        serde_json::to_vec(&payload)
            .inspect_err(|error| tracing::warn!(%error, "history could not be serialised"))
            .ok()
    }

    /// Stores the encoded payload as sealed records, atomically.
    ///
    /// Best-effort: a failure is warned about and the run carries on with the
    /// messages still in memory — the archive is a head start for the next
    /// launch, and the wire can always refill it. A provider that cannot reach
    /// its key fails in [`Store::open`], before any file is touched.
    #[cfg(test)]
    pub(crate) fn write(&self, bytes: &[u8]) {
        let _ = self.try_write(bytes);
    }

    /// The record write, saying why when the archive was not written.
    ///
    /// For the seeding example, which owns the whole store: a write that cannot
    /// land must say why rather than fail silently.
    #[allow(dead_code)]
    pub(crate) fn try_write(&self, bytes: &[u8]) -> Result<(), String> {
        if self.poisoned.load(Ordering::Relaxed) {
            tracing::warn!(
                "history save refused: the archive could not be read when it was opened"
            );
            return Err(
                "the history archive could not be read; sign out to clear it before saving again"
                    .to_owned(),
            );
        }
        let payload: Payload = serde_json::from_slice(bytes)
            .map_err(|error| format!("history could not be read for writing: {error}"))?;
        self.store_payload(&payload)
            .map_err(|error| format!("history could not be written: {error:#}"))
    }

    /// Opens the archive and folds `payload` into it in one transaction: the
    /// bounded window is an ingest, never a replacement of what is stored.
    #[allow(dead_code)]
    fn store_payload(&self, payload: &Payload) -> Result<()> {
        let (_guard, keys) = self.locked_keys();
        let mut store = open_store(&self.path, &keys)?;
        store.ingest_payload(payload)
    }

    /// Writes fetched pages and the encoded payload in one transaction: the
    /// pages first, each with its own extent, then the bounded window with its
    /// deletions.
    ///
    /// A page is not a window: it may be older than the cache holds, or land
    /// while the newest end moves on, and it must keep the stretch it was
    /// fetched from instead of being rebuilt from whatever rows survive in
    /// memory. Pages that touch nothing stored are kept under their own range
    /// by [`Store::ingest_pages_and_payload`]; whether to show such a page is
    /// the fetch's decision. One commit covers both halves, so a death between
    /// them cannot leave a page that restores a row the window deleted. Poison
    /// and the provider swap are handled exactly as for a window write.
    pub(crate) fn ingest_pages_and_write(
        &self,
        pages: &[(i64, Vec<Message>)],
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.poisoned.load(Ordering::Relaxed) {
            tracing::warn!(
                "history save refused: the archive could not be read when it was opened"
            );
            return Err(
                "the history archive could not be read; sign out to clear it before saving again"
                    .to_owned(),
            );
        }
        let payload: Payload = serde_json::from_slice(bytes)
            .map_err(|error| format!("history could not be read for writing: {error}"))?;
        let prepared: Vec<(i64, Vec<CachedMessage>)> = pages
            .iter()
            .map(|(chat_id, messages)| {
                let mut rows: Vec<CachedMessage> = messages
                    .iter()
                    .filter(|message| message.id > 0 && message.chat_id == *chat_id)
                    .map(CachedMessage::from_message)
                    .collect();
                rows.sort_by_key(|row| row.id);
                (*chat_id, rows)
            })
            .collect();
        let (_guard, keys) = self.locked_keys();
        let mut store = open_store(&self.path, &keys)
            .map_err(|error| format!("history could not be written: {error:#}"))?;
        store
            .ingest_pages_and_payload(&prepared, &payload)
            .map_err(|error| format!("history could not be written: {error:#}"))
    }

    /// Removes the database and its WAL, shared-memory, journal and salt
    /// siblings; a missing file is success. The legacy `history.json` is not
    /// one of them and is deliberately left alone.
    fn remove_archive(&self) {
        for path in archive_siblings(&self.path) {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(%error, "history archive could not be removed"),
            }
        }
    }

    /// Logout calls this, and a failed delete must not fail the logout, so the
    /// failure is a warning and the return is nothing. The reader asked for the
    /// archive to go, so a poison from an unreadable archive is lifted with it:
    /// the next save starts a fresh file rather than being refused forever.
    pub(crate) fn clear(&self) {
        self.remove_archive();
        self.poisoned.store(false, Ordering::Relaxed);
    }
}

/// Whether a stored file may seed this launch.
///
/// The drafts rule turned around. For drafts a false clear loses the reader's
/// words, so only a named mismatch discards. Here a false clear costs one
/// fetch the wire was going to make anyway, while a false accept paints
/// another account's conversation under a peer id that happens to be reused.
///
/// A file tagged with a user id is accepted: who is signed in is not reliably
/// knowable before `Ready`, which checks it ([`HistoryTag::holds_for`]). A
/// file with only the legacy phone tag is accepted only when it equals the
/// configured phone, unnamed matching unnamed.
pub(crate) fn history_acceptable(
    stored_phone: Option<&str>,
    stored_id: Option<i64>,
    configured: Option<&str>,
) -> bool {
    stored_id.is_some() || stored_phone == configured
}

/// The one warning about a legacy `history.json`, however many accounts load.
fn warn_legacy_once() {
    use std::sync::Once;

    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "a legacy televim.history.json is still on disk; it was not migrated or deleted"
        );
    });
}

/// The database beside the legacy JSON file: one account, one `.db`.
fn archive_path(legacy: &Path) -> PathBuf {
    legacy.with_extension("db")
}

/// The database and every file SQLite keeps beside it, plus the salt.
fn archive_siblings(db: &Path) -> Vec<PathBuf> {
    let name = db.file_name().unwrap_or_default().to_os_string();
    let sibling = |suffix: &str| {
        let mut name = name.clone();
        name.push(suffix);
        db.with_file_name(name)
    };
    vec![
        db.to_path_buf(),
        sibling("-wal"),
        sibling("-shm"),
        sibling("-journal"),
        sibling(SALT_SUFFIX),
    ]
}

fn salt_sibling(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(SALT_SUFFIX);
    db.with_file_name(name)
}

/// Creates the database file before SQLite sees it, restricted from the
/// start: no window in which the archive is readable by anyone else.
#[cfg(unix)]
fn create_db_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_db_file(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Restricts a freshly written sibling (the salt) to its owner.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict the permissions of the history archive");
    }
}

/// No-op on platforms whose ACLs `std` cannot express portably.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// The schema this build writes. Recorded in `meta` so a later build can tell
/// an archive it understands from one it must not guess at.
const SCHEMA_VERSION: i64 = 1;
const META_VERSION: &str = "schema_version";
const META_STATE: &str = "state";
const META_ARCHIVE_KEY: &str = "archive_key";
/// The previous passphrase's envelope of the same random archive key. Present
/// only while a passphrase change is half-finished: it lets the next start
/// tell an interrupted change from a completed one.
const META_ARCHIVE_KEY_PREV: &str = "archive_key_prev";
const SALT_SUFFIX: &str = ".salt";
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;

/// Locks a mutex, ignoring poison: the provider cell is replaced whole, so a
/// panic elsewhere cannot leave it half-written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The key source records, state and chips are sealed under: the random
/// archive key itself, with a fixed salt. It never changes when the account
/// passphrase changes, so rows stay readable and re-sealing touches meta only.
#[derive(Clone)]
struct RecordKey {
    bytes: [u8; KEY_LEN],
}

impl std::fmt::Debug for RecordKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecordKey(<redacted>)")
    }
}

impl KeyProvider for RecordKey {
    fn sealing_key(&self) -> Result<(sealed::Salt, FileKey), SessionError> {
        Ok(([0u8; SALT_LEN], FileKey::from_bytes(self.bytes)))
    }

    fn opening_key(&self, _salt: &sealed::Salt) -> Result<FileKey, SessionError> {
        Ok(FileKey::from_bytes(self.bytes))
    }

    fn accepts_passphrase(&self) -> bool {
        false
    }
}

/// Opens an account-sealed `archive_key` envelope into its 32 bytes.
fn open_archive_key(envelope: &[u8], keys: &dyn KeyProvider) -> Result<[u8; KEY_LEN]> {
    let plain = sealed::open(envelope, keys)?
        .ok_or_else(|| anyhow!("the archive key was stored as plaintext"))?;
    plain
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("the archive key is the wrong length"))
}

/// Pragmas applied on every open, before any table is touched. WAL with a full
/// sync makes a committed row survive a process kill; a 256 KiB page cache and
/// no memory mapping bound what SQLite holds resident; the WAL cannot grow
/// without limit. `fullfsync` is macOS only and is enabled there below.
const OPEN_PRAGMAS: &str = "PRAGMA journal_mode=WAL;
    PRAGMA synchronous=FULL;
    PRAGMA cache_size=-256;
    PRAGMA mmap_size=0;
    PRAGMA journal_size_limit=1048576;";

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS meta (
        key TEXT PRIMARY KEY,
        value BLOB NOT NULL
    );
    CREATE TABLE IF NOT EXISTS messages (
        chat_key BLOB NOT NULL,
        message_id INTEGER NOT NULL,
        sealed BLOB NOT NULL,
        PRIMARY KEY (chat_key, message_id)
    );
    -- The stretches of a chat that are held. Ids are not dense, so a run cannot
    -- be inferred from its ends; the ledger is what makes a hole a fact.
    CREATE TABLE IF NOT EXISTS ranges (
        chat_key BLOB NOT NULL,
        lo_id INTEGER NOT NULL,
        hi_id INTEGER NOT NULL,
        PRIMARY KEY (chat_key, lo_id)
    );
    -- One sealed, bounded side table per chat: reactions and pins change without
    -- the message changing, so they are not carved into a record.
    CREATE TABLE IF NOT EXISTS chips (
        chat_key BLOB PRIMARY KEY,
        sealed BLOB NOT NULL
    );";

/// The non-message half of the archive, sealed as one blob in `meta`: a chat
/// id or a title must never sit in a column in the clear.
#[derive(Debug, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    account_id: Option<i64>,
    #[serde(default)]
    chats: Vec<CachedChat>,
    #[serde(default)]
    aliases: BTreeMap<i64, String>,
}

/// One record's plaintext. The address is carried inside, and `decode_record`
/// refuses a row whose plaintext does not name the column pair it was found
/// under: `sealed`'s AAD is fixed, so the binding has to live here.
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    chat_key: Vec<u8>,
    chat_id: i64,
    message_id: i64,
    row: CachedMessage,
}

/// One chat's reaction strips and pinned ids, as stored in the sealed side
/// table. A plain mirror of the domain's types, which carry no `serde`, with
/// the same bounds: [`REACTION_ENTRIES`] messages with strips and
/// [`PINNED_IDS`] pins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Chips {
    #[serde(default)]
    reactions: Vec<(i64, Vec<CachedReaction>)>,
    #[serde(default)]
    pinned: Vec<i64>,
}

/// The sealed plaintext of one chip row. Like [`Record`], it carries its own
/// address so a side table cannot be moved under another chat key.
#[derive(Debug, Serialize, Deserialize)]
struct ChipRecord {
    chat_key: Vec<u8>,
    chat_id: i64,
    chips: Chips,
}

/// [`Reaction`] as the file spells it: `domain` carries no `serde`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum CachedReactionKind {
    Emoticon(String),
    CustomEmoji(i64),
    Paid,
}

/// [`ReactionCount`] as the file spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedReaction {
    reaction: CachedReactionKind,
    count: u32,
    chosen: bool,
}

impl From<&Reaction> for CachedReactionKind {
    fn from(reaction: &Reaction) -> Self {
        match reaction {
            Reaction::Emoticon(emoticon) => Self::Emoticon(emoticon.clone()),
            Reaction::CustomEmoji(id) => Self::CustomEmoji(*id),
            Reaction::Paid => Self::Paid,
        }
    }
}

impl From<&CachedReactionKind> for Reaction {
    fn from(reaction: &CachedReactionKind) -> Self {
        match reaction {
            CachedReactionKind::Emoticon(emoticon) => Self::Emoticon(emoticon.clone()),
            CachedReactionKind::CustomEmoji(id) => Self::CustomEmoji(*id),
            CachedReactionKind::Paid => Self::Paid,
        }
    }
}

impl From<&ReactionCount> for CachedReaction {
    fn from(count: &ReactionCount) -> Self {
        Self {
            reaction: CachedReactionKind::from(&count.reaction),
            count: count.count,
            chosen: count.chosen,
        }
    }
}

impl From<&CachedReaction> for ReactionCount {
    fn from(count: &CachedReaction) -> Self {
        Self {
            reaction: Reaction::from(&count.reaction),
            count: count.count,
            chosen: count.chosen,
        }
    }
}

/// `HMAC(chat_id)` under a key derived from the file key. The column never
/// holds the id itself; the derivation key and the salt are unsealable only by
/// the account whose keyring/passphrase opened the archive.
fn keyed_chat(hmac_key: &[u8; KEY_LEN], chat_id: i64) -> Vec<u8> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(hmac_key).expect("HMAC accepts a key of any length");
    mac.update(&chat_id.to_le_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Seals one row under its address. Returns the column key and the envelope.
fn seal_record(
    keys: &dyn KeyProvider,
    hmac_key: &[u8; KEY_LEN],
    chat_id: i64,
    row: &CachedMessage,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let chat_key = keyed_chat(hmac_key, chat_id);
    let record = Record {
        chat_key: chat_key.clone(),
        chat_id,
        message_id: row.id,
        row: row.clone(),
    };
    let plain = serde_json::to_vec(&record)?;
    Ok((chat_key, sealed::seal(&plain, keys)?))
}

fn insert_record(
    conn: &Connection,
    chat_key: &[u8],
    message_id: i64,
    envelope: &[u8],
) -> Result<()> {
    conn.execute(
        "INSERT INTO messages (chat_key, message_id, sealed) VALUES (?1, ?2, ?3)
         ON CONFLICT(chat_key, message_id) DO UPDATE SET sealed = excluded.sealed",
        params![chat_key, message_id, envelope],
    )?;
    Ok(())
}

/// Decodes a record and checks its address. `Ok(None)` from `sealed::open`
/// means the bytes were never sealed: that is a hard error here, never a
/// legacy-plaintext fallback.
fn decode_record(
    envelope: &[u8],
    keys: &dyn KeyProvider,
    hmac_key: &[u8; KEY_LEN],
    chat_key: &[u8],
    message_id: i64,
) -> Result<Record> {
    let plain = sealed::open(envelope, keys)?
        .ok_or_else(|| anyhow!("a history record was stored as plaintext"))?;
    let record: Record = serde_json::from_slice(&plain)?;
    if record.chat_key != chat_key
        || record.message_id != message_id
        || keyed_chat(hmac_key, record.chat_id) != chat_key
    {
        return Err(anyhow!("a history record was filed under another address"));
    }
    Ok(record)
}

/// Opens one account's archive, creating it with a `0600` file and schema v1
/// on first use. Paired with [`Store::put_record`], [`Store::get_record`] and
/// [`Store::all_records`], this is the record layer later stages build on.
fn open_store(path: &Path, keys: &Arc<dyn KeyProvider>) -> Result<Store> {
    Store::open(path, keys)
}

/// A connection to one account's archive. Opened per operation rather than
/// held: `HistoryFile` is cloned onto blocking threads, and a `rusqlite`
/// connection is neither `Clone` nor `Sync`.
struct Store {
    conn: Connection,
    record_keys: RecordKey,
    hmac_key: [u8; KEY_LEN],
}

/// What opening `archive_key` found about the half-finished change.
#[derive(Clone, Copy)]
enum KeyRepair {
    /// A brand-new archive: no envelopes exist yet.
    Fresh,
    /// The primary opened. `archive_key_prev` stays until the session is
    /// durable: an ordinary write under the new key must not cut the way back.
    DropPrev,
    /// Only the previous envelope opened. The passphrase change never
    /// finished; promote that key to the primary and drop the previous row.
    PromotePrev,
}

fn meta_envelope(conn: &Connection, key: &str) -> Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Reads the random archive key, preferring the current passphrase's envelope
/// and falling back to the previous one when a change was cut short.
fn load_archive_key(
    conn: &Connection,
    keys: &dyn KeyProvider,
    existed: bool,
) -> Result<([u8; KEY_LEN], KeyRepair)> {
    if !existed {
        let mut key = [0u8; KEY_LEN];
        getrandom::fill(&mut key)?;
        let envelope = sealed::seal(&key, keys)?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![META_ARCHIVE_KEY, envelope],
        )?;
        return Ok((key, KeyRepair::Fresh));
    }
    if let Some(envelope) = meta_envelope(conn, META_ARCHIVE_KEY)? {
        match open_archive_key(&envelope, keys) {
            Ok(key) => return Ok((key, KeyRepair::DropPrev)),
            Err(error) => {
                // A wrong primary is not yet a reason to poison: a passphrase
                // change may have been interrupted before the session saved.
                if let Some(prev) = meta_envelope(conn, META_ARCHIVE_KEY_PREV)? {
                    return Ok((open_archive_key(&prev, keys)?, KeyRepair::PromotePrev));
                }
                return Err(error);
            }
        }
    }
    match meta_envelope(conn, META_ARCHIVE_KEY_PREV)? {
        Some(prev) => Ok((open_archive_key(&prev, keys)?, KeyRepair::PromotePrev)),
        None => Err(anyhow!("the archive key is missing")),
    }
}

/// Settles a half-finished passphrase change, in one transaction. Idempotent:
/// either a stale previous row goes, or the key the old session still uses is
/// promoted back to the primary.
fn repair_archive_key(
    conn: &mut Connection,
    keys: &dyn KeyProvider,
    archive_key: &[u8; KEY_LEN],
    repair: KeyRepair,
) -> Result<()> {
    match repair {
        // `Fresh` has nothing to settle. `DropPrev` keeps the previous envelope
        // until `confirm_rekey` says the session was saved under the new
        // passphrase: a write that opens the archive in between must leave the
        // way back intact.
        KeyRepair::Fresh | KeyRepair::DropPrev => {}
        KeyRepair::PromotePrev => {
            let envelope = sealed::seal(archive_key, keys)?;
            if open_archive_key(&envelope, keys)? != *archive_key {
                return Err(anyhow!("the archive key could not be re-sealed"));
            }
            let transaction = conn.transaction()?;
            transaction.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![META_ARCHIVE_KEY, envelope],
            )?;
            transaction.execute(
                "DELETE FROM meta WHERE key = ?1",
                params![META_ARCHIVE_KEY_PREV],
            )?;
            transaction.commit()?;
        }
    }
    Ok(())
}

impl Store {
    fn open(path: &Path, keys: &Arc<dyn KeyProvider>) -> Result<Self> {
        // Reach the key before any file exists: an unreachable key must not
        // leave a fresh, empty database behind.
        keys.sealing_key()?;
        let existed = path.is_file();
        if !existed {
            if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                fs::create_dir_all(parent)?;
            }
            create_db_file(path)?;
        }
        let mut conn = Connection::open(path)?;
        conn.execute_batch(OPEN_PRAGMAS)?;
        #[cfg(target_os = "macos")]
        conn.execute_batch("PRAGMA fullfsync=ON;")?;
        conn.execute_batch(SCHEMA)?;

        let salt_path = salt_sibling(path);
        let salt: [u8; SALT_LEN] = if existed {
            let bytes = fs::read(&salt_path)?;
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("the history archive salt is the wrong length"))?
        } else {
            let mut bytes = [0u8; SALT_LEN];
            getrandom::fill(&mut bytes)?;
            fs::write(&salt_path, bytes)?;
            restrict_permissions(&salt_path);
            bytes
        };

        let (archive_key, repair) = load_archive_key(&conn, keys.as_ref(), existed)?;
        repair_archive_key(&mut conn, keys.as_ref(), &archive_key, repair)?;

        let version: Option<i64> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_VERSION],
                |row| row.get(0),
            )
            .optional()?;
        match version {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                    params![META_VERSION, SCHEMA_VERSION],
                )?;
            }
            Some(version) if version == SCHEMA_VERSION => {}
            Some(version) => {
                return Err(anyhow!("unsupported history schema version {version}"));
            }
        }

        // The salt is not secret; it keeps the chat hash distinct between
        // databases even when two accounts happen to share a file key.
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&archive_key).expect("HMAC accepts a key of any length");
        mac.update(&salt);
        let mut hmac_key = [0u8; KEY_LEN];
        hmac_key.copy_from_slice(&mac.finalize().into_bytes());
        Ok(Self {
            conn,
            record_keys: RecordKey { bytes: archive_key },
            hmac_key,
        })
    }

    fn state(&self) -> Result<State> {
        let envelope: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_STATE],
                |row| row.get(0),
            )
            .optional()?;
        match envelope {
            None => Ok(State {
                account: None,
                account_id: None,
                chats: Vec::new(),
                aliases: BTreeMap::new(),
            }),
            Some(envelope) => {
                let plain = sealed::open(&envelope, &self.record_keys)?
                    .ok_or_else(|| anyhow!("the archive state was stored as plaintext"))?;
                Ok(serde_json::from_slice(&plain)?)
            }
        }
    }

    /// The record layer beneath range reads and ingest: the archive grows by
    /// records, and a save seals the rows of its window through the same path.
    #[allow(dead_code)]
    fn put_record(&self, chat_id: i64, row: &CachedMessage) -> Result<()> {
        let (chat_key, envelope) = seal_record(&self.record_keys, &self.hmac_key, chat_id, row)?;
        insert_record(&self.conn, &chat_key, row.id, &envelope)
    }

    #[allow(dead_code)]
    fn get_record(&self, chat_id: i64, message_id: i64) -> Result<Option<CachedMessage>> {
        let chat_key = keyed_chat(&self.hmac_key, chat_id);
        let envelope: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT sealed FROM messages WHERE chat_key = ?1 AND message_id = ?2",
                params![chat_key, message_id],
                |row| row.get(0),
            )
            .optional()?;
        envelope
            .map(|envelope| {
                decode_record(
                    &envelope,
                    &self.record_keys,
                    &self.hmac_key,
                    &chat_key,
                    message_id,
                )
                .map(|record| record.row)
            })
            .transpose()
    }

    /// Every record of one chat, oldest id first.
    #[allow(dead_code)]
    fn all_records(&self, chat_id: i64) -> Result<Vec<CachedMessage>> {
        let chat_key = keyed_chat(&self.hmac_key, chat_id);
        let mut statement = self.conn.prepare(
            "SELECT message_id, sealed FROM messages WHERE chat_key = ?1 ORDER BY message_id",
        )?;
        let rows = statement.query_map(params![chat_key], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (message_id, envelope) = row?;
            decode_record(
                &envelope,
                &self.record_keys,
                &self.hmac_key,
                &chat_key,
                message_id,
            )
            .map(|record| record.row)
        })
        .collect()
    }

    /// The newest `depth` records of each chat, with the chat id recovered from
    /// the sealed plaintext (the column holds only the keyed hash).
    ///
    /// The bound is applied in SQL per chat, so a launch never decrypts the
    /// whole archive just to fill its windows. Oldest rows of a chat that has
    /// more than `depth` are not read at all.
    fn bounded_records(&self, depth: usize) -> Result<Vec<(i64, CachedMessage)>> {
        let limit = i64::try_from(depth).map_err(|_| anyhow!("the window depth is too large"))?;
        let mut chats = self
            .conn
            .prepare("SELECT DISTINCT chat_key FROM messages")?;
        let chat_keys: Vec<Vec<u8>> = chats
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::new();
        for chat_key in chat_keys {
            let mut statement = self.conn.prepare(
                "SELECT message_id, sealed FROM messages WHERE chat_key = ?1
                 ORDER BY message_id DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(params![chat_key, limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            let mut window = Vec::new();
            for row in rows {
                let (message_id, envelope) = row?;
                let record = decode_record(
                    &envelope,
                    &self.record_keys,
                    &self.hmac_key,
                    &chat_key,
                    message_id,
                )?;
                window.push((record.chat_id, record.row));
            }
            // SQL read newest-first to bound with `LIMIT`; the cache is oldest-first.
            window.reverse();
            out.extend(window);
        }
        Ok(out)
    }

    /// The newest `limit` rows of `chat_id` strictly below `below`, decoded and
    /// unsealed, oldest id first. `lo` confines the read to one recorded range
    /// so a page never crosses a gap into an older stretch. The bound is applied
    /// in SQL: only the rows selected by `LIMIT` are decoded, the rest of the
    /// conversation is never read or unsealed.
    fn page(
        &self,
        chat_id: i64,
        below: i64,
        lo: i64,
        limit: usize,
    ) -> Result<Vec<(i64, CachedMessage)>> {
        let chat_key = keyed_chat(&self.hmac_key, chat_id);
        let limit = i64::try_from(limit).map_err(|_| anyhow!("the page size is too large"))?;
        let mut statement = self.conn.prepare(
            "SELECT message_id, sealed FROM messages
             WHERE chat_key = ?1 AND message_id < ?2 AND message_id >= ?3
             ORDER BY message_id DESC LIMIT ?4",
        )?;
        let rows = statement.query_map(params![chat_key, below, lo, limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (message_id, envelope) = row?;
            let record = decode_record(
                &envelope,
                &self.record_keys,
                &self.hmac_key,
                &chat_key,
                message_id,
            )?;
            out.push((message_id, record.row));
        }
        // SQL read newest-first to bound with `LIMIT`; a page is oldest-first.
        out.reverse();
        Ok(out)
    }

    /// Every record of every chat, with the chat id recovered from the sealed
    /// plaintext (the column holds only the keyed hash).
    #[cfg(test)]
    fn every_record(&self) -> Result<Vec<(i64, CachedMessage)>> {
        let mut statement = self.conn.prepare(
            "SELECT chat_key, message_id, sealed FROM messages ORDER BY chat_key, message_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (chat_key, message_id, envelope) = row?;
            decode_record(
                &envelope,
                &self.record_keys,
                &self.hmac_key,
                &chat_key,
                message_id,
            )
            .map(|record| (record.chat_id, record.row))
        })
        .collect()
    }

    /// Folds each bounded window of `payload` into the archive in one
    /// transaction, alongside the state and side tables.
    ///
    /// Only the chats the payload names are touched. A window that does not
    /// join its stored range is kept as a range of its own: a refusal here
    /// belongs to the fetch decision, not the store. Messages the reader
    /// deleted are dropped last, and the ranges of the chats that held them
    /// are rebuilt from the rows that remain, so a deletion inside a run
    /// splits it rather than letting the window write claim the gap again.
    #[allow(dead_code)]
    fn ingest_payload(&mut self, payload: &Payload) -> Result<()> {
        let transaction = self.conn.transaction()?;
        ingest_payload_in(&transaction, &self.record_keys, &self.hmac_key, payload)?;
        transaction.commit()?;
        Ok(())
    }

    /// Folds `pages` into the archive and then the bounded window `payload`,
    /// all in one transaction.
    ///
    /// The order is the one the write owes the reader: every page lands before
    /// the window's deletions, and one commit covers both, so a death between
    /// the two cannot leave a page that restores a row the window deleted.
    fn ingest_pages_and_payload(
        &mut self,
        pages: &[(i64, Vec<CachedMessage>)],
        payload: &Payload,
    ) -> Result<()> {
        let transaction = self.conn.transaction()?;
        for (chat_id, rows) in pages {
            let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
                continue;
            };
            let reach = (first.id.min(last.id), first.id.max(last.id));
            let held = ranges_on(&transaction, &self.hmac_key, *chat_id)?;
            ingest_in(
                &transaction,
                &self.record_keys,
                &self.hmac_key,
                *chat_id,
                rows,
                reach,
                &held,
            )?;
        }
        ingest_payload_in(&transaction, &self.record_keys, &self.hmac_key, payload)?;
        transaction.commit()?;
        Ok(())
    }

    /// The bounded reaction/pin side tables, with their chat ids recovered
    /// from the sealed plaintext and their addresses checked.
    #[allow(dead_code)]
    fn chips(&self) -> Result<BTreeMap<i64, Chips>> {
        let mut statement = self
            .conn
            .prepare("SELECT chat_key, sealed FROM chips ORDER BY chat_key")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut chips = BTreeMap::new();
        for row in rows {
            let (chat_key, envelope) = row?;
            let plain = sealed::open(&envelope, &self.record_keys)?
                .ok_or_else(|| anyhow!("a side table was stored as plaintext"))?;
            let record: ChipRecord = serde_json::from_slice(&plain)?;
            if record.chat_key != chat_key || keyed_chat(&self.hmac_key, record.chat_id) != chat_key
            {
                return Err(anyhow!("a side table was filed under another address"));
            }
            chips.insert(record.chat_id, record.chips);
        }
        Ok(chips)
    }

    /// The stretches of `chat_id` held on disk, oldest first.
    #[allow(dead_code)]
    fn ranges(&self, chat_id: i64) -> Result<Vec<(i64, i64)>> {
        ranges_on(&self.conn, &self.hmac_key, chat_id)
    }

    /// The sealed rows of one chat in one recorded stretch, decoded and
    /// unsealed on the way out. A whole chat is the union of its ranges.
    #[allow(dead_code)]
    fn range_rows(&self, chat_id: i64, lo: i64, hi: i64) -> Result<Vec<CachedMessage>> {
        let chat_key = keyed_chat(&self.hmac_key, chat_id);
        let mut statement = self.conn.prepare(
            "SELECT message_id, sealed FROM messages
             WHERE chat_key = ?1 AND message_id BETWEEN ?2 AND ?3 ORDER BY message_id",
        )?;
        let rows = statement.query_map(params![chat_key, lo, hi], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (message_id, envelope) = row?;
            decode_record(
                &envelope,
                &self.record_keys,
                &self.hmac_key,
                &chat_key,
                message_id,
            )
            .map(|record| record.row)
        })
        .collect()
    }

    /// The text bytes a SQL read of the archive accounts for, by the same
    /// rule as [`encoded_len`]. Kept beside the rows so the on-disk cost is a
    /// query rather than a second copy of everything in memory.
    #[allow(dead_code)]
    fn text_bytes(&self) -> Result<usize> {
        let mut statement = self.conn.prepare("SELECT sealed FROM messages")?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        let mut total = 0;
        for row in rows {
            let envelope = row?;
            let plain = sealed::open(&envelope, &self.record_keys)?
                .ok_or_else(|| anyhow!("a history record was stored as plaintext"))?;
            let record: Record = serde_json::from_slice(&plain)?;
            total += encoded_len(&record.row.text);
        }
        Ok(total)
    }

    /// Folds one page into `chat_id` and its range ledger in one transaction.
    ///
    /// `reach` is the page's own bounds as the fetch knows them: for an
    /// anchored page the anchor is one end, the proof that the page joins the
    /// run without sharing a row with it. A page that touches no recorded
    /// range is kept under its own range rather than lost — whether the gap is
    /// safe to show is the fetch decision's, not the store's. A page joining
    /// two ranges bridges them into one; nothing already stored is dropped, so
    /// a range only widens.
    ///
    /// Returns whether anything was written.
    #[allow(dead_code)]
    fn ingest_page(
        &mut self,
        chat_id: i64,
        rows: &[CachedMessage],
        reach: (i64, i64),
    ) -> Result<bool> {
        let held = self.ranges(chat_id)?;
        let transaction = self.conn.transaction()?;
        let written = ingest_in(
            &transaction,
            &self.record_keys,
            &self.hmac_key,
            chat_id,
            rows,
            reach,
            &held,
        )?;
        transaction.commit()?;
        Ok(written)
    }
}

/// Drops the named messages wherever they are stored and rebuilds the ranges
/// of the chats that held them.
///
/// A private message id belongs to one conversation, so a deletion that names
/// no chat can still be applied by id alone. A deleted id splits the stretch
/// that contained it; a stretch left with no rows disappears. Rebuilding from
/// the rows that remain (rather than trusting the old ends) is what keeps a
/// hole in the middle from being claimed as covered.
fn delete_messages(transaction: &rusqlite::Transaction<'_>, deleted: &BTreeSet<i64>) -> Result<()> {
    // The set holds every id this session has deleted, and SQLite has a cap on
    // statement parameters: the ids go in in chunks so a long session cannot
    // make the save fail.
    const CHUNK: usize = 400;
    let ids: Vec<i64> = deleted.iter().copied().collect();
    let mut affected: BTreeSet<Vec<u8>> = BTreeSet::new();
    for chunk in ids.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let keys_sql =
            format!("SELECT DISTINCT chat_key FROM messages WHERE message_id IN ({placeholders})");
        let mut statement = transaction.prepare(&keys_sql)?;
        let keys = statement
            .query_map(params_from_iter(chunk.iter()), |row| row.get(0))?
            .collect::<std::result::Result<Vec<Vec<u8>>, _>>()?;
        affected.extend(keys);
    }

    let mut ledgers = Vec::new();
    for chat_key in affected {
        let mut statement = transaction
            .prepare("SELECT lo_id, hi_id FROM ranges WHERE chat_key = ?1 ORDER BY lo_id")?;
        let ranges: Vec<(i64, i64)> = statement
            .query_map(params![chat_key], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        ledgers.push((chat_key, ranges));
    }

    for chunk in ids.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let delete_sql = format!("DELETE FROM messages WHERE message_id IN ({placeholders})");
        transaction.execute(&delete_sql, params_from_iter(chunk.iter()))?;
    }

    for (chat_key, ranges) in ledgers {
        let rebuilt = ranges_after_delete(transaction, &chat_key, &ranges, deleted)?;
        transaction.execute("DELETE FROM ranges WHERE chat_key = ?1", params![chat_key])?;
        for (lo, hi) in rebuilt {
            transaction.execute(
                "INSERT INTO ranges (chat_key, lo_id, hi_id) VALUES (?1, ?2, ?3)",
                params![chat_key, lo, hi],
            )?;
        }
    }
    Ok(())
}

/// What the recorded stretches of one chat become once `deleted` ids are gone.
fn ranges_after_delete(
    transaction: &rusqlite::Transaction<'_>,
    chat_key: &[u8],
    held: &[(i64, i64)],
    deleted: &BTreeSet<i64>,
) -> Result<Vec<(i64, i64)>> {
    let mut out = Vec::new();
    for &(lo, hi) in held {
        let mut spans = vec![(lo, hi)];
        for &id in deleted.iter().filter(|id| (lo..=hi).contains(id)) {
            let mut split = Vec::new();
            for (start, end) in spans {
                if start <= id && id <= end {
                    if start < id {
                        split.push((start, id.saturating_sub(1)));
                    }
                    if id < end {
                        split.push((id.saturating_add(1), end));
                    }
                } else {
                    split.push((start, end));
                }
            }
            spans = split;
        }
        for (start, end) in spans {
            let has_row: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages
                  WHERE chat_key = ?1 AND message_id BETWEEN ?2 AND ?3)",
                params![chat_key, start, end],
                |row| row.get(0),
            )?;
            if has_row {
                out.push((start, end));
            }
        }
    }
    Ok(out)
}

/// The transaction half of [`Store::ingest_page`], shared with a save of a
/// whole window. `held` is the ledger as it stood before the write.
/// The stretches of `chat_id` held on disk, oldest first, read through the
/// given connection so an open transaction can see its own earlier writes.
fn ranges_on(
    conn: &rusqlite::Connection,
    hmac_key: &[u8; KEY_LEN],
    chat_id: i64,
) -> Result<Vec<(i64, i64)>> {
    let chat_key = keyed_chat(hmac_key, chat_id);
    let mut statement =
        conn.prepare("SELECT lo_id, hi_id FROM ranges WHERE chat_key = ?1 ORDER BY lo_id")?;
    let rows = statement.query_map(params![chat_key], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.map(|row| row.map_err(anyhow::Error::from)).collect()
}

/// Folds each bounded window of `payload` into the archive inside the open
/// `transaction`, alongside the state and side tables; the caller commits.
///
/// The held ranges are read through the transaction itself, so they reflect
/// the pages ingested earlier inside the same one. Only the chats the payload
/// names are touched. A window that does not join its stored range is kept as
/// a range of its own: a refusal here belongs to the fetch decision, not the
/// store. Messages the reader deleted are dropped last, and the ranges of the
/// chats that held them are rebuilt from the rows that remain, so a deletion
/// inside a run splits it rather than letting the window write claim the gap
/// again.
fn ingest_payload_in(
    transaction: &rusqlite::Transaction<'_>,
    keys: &dyn KeyProvider,
    hmac_key: &[u8; KEY_LEN],
    payload: &Payload,
) -> Result<()> {
    let state = State {
        account: payload.account.clone(),
        account_id: payload.account_id,
        chats: payload.chats.clone(),
        aliases: payload.aliases.clone(),
    };
    let state_plain = serde_json::to_vec(&state)?;
    let state_envelope = sealed::seal(&state_plain, keys)?;

    let held: BTreeMap<i64, Vec<(i64, i64)>> = payload
        .peers
        .keys()
        .map(|chat_id| Ok((*chat_id, ranges_on(transaction, hmac_key, *chat_id)?)))
        .collect::<Result<_>>()?;
    for chat_id in &payload.removed {
        let chat_key = keyed_chat(hmac_key, *chat_id);
        transaction.execute(
            "DELETE FROM messages WHERE chat_key = ?1",
            params![chat_key],
        )?;
        transaction.execute("DELETE FROM ranges WHERE chat_key = ?1", params![chat_key])?;
        transaction.execute("DELETE FROM chips WHERE chat_key = ?1", params![chat_key])?;
    }
    for (chat_id, rows) in &payload.peers {
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            continue;
        };
        let reach = (first.id.min(last.id), first.id.max(last.id));
        ingest_in(
            transaction,
            keys,
            hmac_key,
            *chat_id,
            rows,
            reach,
            held.get(chat_id).map_or(&[][..], Vec::as_slice),
        )?;
    }
    // Re-seal edits. A window row carries its own new words; a row outside
    // the window is read from its stored envelope and keeps every field but
    // its text. Deletions below run last, so an edit and a deletion of the
    // same id end deleted.
    for (chat_id, edited) in &payload.updated_messages {
        for (message_id, text) in edited {
            if let Some(row) = payload
                .peers
                .get(chat_id)
                .and_then(|rows| rows.iter().find(|row| row.id == *message_id))
            {
                let (key, envelope) = seal_record(keys, hmac_key, *chat_id, row)?;
                insert_record(transaction, &key, row.id, &envelope)?;
                continue;
            }
            let chat_key = keyed_chat(hmac_key, *chat_id);
            let envelope: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT sealed FROM messages WHERE chat_key = ?1 AND message_id = ?2",
                    params![chat_key, message_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(envelope) = envelope else {
                continue;
            };
            let mut record = decode_record(&envelope, keys, hmac_key, &chat_key, *message_id)?;
            record.row.text.clone_from(text);
            let (key, envelope) = seal_record(keys, hmac_key, *chat_id, &record.row)?;
            insert_record(transaction, &key, *message_id, &envelope)?;
        }
    }
    if !payload.deleted_messages.is_empty() {
        delete_messages(transaction, &payload.deleted_messages)?;
    }
    for (chat_id, chips) in &payload.chips {
        let chat_key = keyed_chat(hmac_key, *chat_id);
        let record = ChipRecord {
            chat_key: chat_key.clone(),
            chat_id: *chat_id,
            chips: chips.clone(),
        };
        let plain = serde_json::to_vec(&record)?;
        let envelope = sealed::seal(&plain, keys)?;
        transaction.execute(
            "INSERT INTO chips (chat_key, sealed) VALUES (?1, ?2)
             ON CONFLICT(chat_key) DO UPDATE SET sealed = excluded.sealed",
            params![chat_key, envelope],
        )?;
    }
    transaction.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![META_STATE, state_envelope],
    )?;
    Ok(())
}

fn ingest_in(
    transaction: &rusqlite::Transaction<'_>,
    keys: &dyn KeyProvider,
    hmac_key: &[u8; KEY_LEN],
    chat_id: i64,
    rows: &[CachedMessage],
    reach: (i64, i64),
    held: &[(i64, i64)],
) -> Result<bool> {
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Ok(false);
    };
    let (lo, hi) = (first.id.min(last.id), first.id.max(last.id));
    let chat_key = keyed_chat(hmac_key, chat_id);
    let joined: Vec<(i64, i64)> = held
        .iter()
        .filter(|(rlo, rhi)| *rhi >= reach.0 && *rlo <= reach.1)
        .copied()
        .collect();
    if !held.is_empty() && joined.is_empty() {
        // A page that touches nothing stored is still what the wire returned:
        // keep it under its own range rather than losing what was downloaded.
        // Whether the gap is safe to show is the fetch decision's, not ours.
        tracing::warn!(chat_id, "history page stored as a disjoint range");
    }

    for row in rows {
        let (key, envelope) = seal_record(keys, hmac_key, chat_id, row)?;
        insert_record(transaction, &key, row.id, &envelope)?;
    }
    for (old_lo, _) in &joined {
        transaction.execute(
            "DELETE FROM ranges WHERE chat_key = ?1 AND lo_id = ?2",
            params![chat_key, old_lo],
        )?;
    }
    let new_lo = joined
        .iter()
        .map(|(rlo, _)| *rlo)
        .min()
        .unwrap_or(lo)
        .min(lo);
    let new_hi = joined
        .iter()
        .map(|(_, rhi)| *rhi)
        .max()
        .unwrap_or(hi)
        .max(hi);

    transaction.execute(
        "INSERT INTO ranges (chat_key, lo_id, hi_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(chat_key, lo_id) DO UPDATE SET hi_id = excluded.hi_id",
        params![chat_key, new_lo, new_hi],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: i64 = 7;

    /// Writes a hand-written JSON payload into the archive the way a save
    /// would, so a fixture can exercise the load bounds without a wire.
    fn seed(file: &HistoryFile, plain: impl AsRef<[u8]>) {
        let payload: Payload =
            serde_json::from_slice(plain.as_ref()).expect("the fixture is a payload");
        file.store_payload(&payload)
            .expect("the fixture is written");
    }

    /// A key source whose key is not the one `for_test` seals under.
    fn other_keys() -> Arc<dyn KeyProvider> {
        Arc::new(telegram_framework::PassphraseProvider::new("another"))
    }

    /// A history file in a scratch directory, so a run never touches the
    /// developer's own.
    fn scratch(name: &str) -> (tempfile::TempDir, HistoryFile) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = HistoryFile::for_test(dir.path().join(name));
        (dir, file)
    }

    fn message(id: i64, chat_id: i64, text: &str) -> Message {
        Message {
            id,
            chat_id,
            text: Cow::Owned(text.to_owned()),
            timestamp: id * 10,
            status: MessageStatus::Received,
            is_outgoing: false,
            is_service: false,
            reply_to: None,
            media: None,
            media_id: None,
        }
    }

    /// The fields a round trip has to keep, as one comparable value:
    /// `Message` itself has no `PartialEq`.
    fn fields(message: &Message) -> (i64, i64, &str, i64, MessageStatus, bool, Option<i64>) {
        (
            message.id,
            message.chat_id,
            &*message.text,
            message.timestamp,
            message.status,
            message.is_outgoing,
            message.reply_to,
        )
    }

    fn ids(messages: &[Message]) -> Vec<i64> {
        messages.iter().map(|message| message.id).collect()
    }

    // ---- the record layer (stage 01) ------------------------------------

    #[test]
    fn a_record_round_trips_every_field_through_a_reopened_database() {
        let (_dir, file) = scratch("history.json");
        let mut written = message(7, PEER, "the text");
        written.timestamp = 1_700_000_000;
        written.is_outgoing = true;
        written.status = MessageStatus::Sent;
        written.is_service = true;
        written.reply_to = Some(3);
        written.media = Some(MediaKind::Voice);
        written.media_id = Some(91);
        let row = CachedMessage::from_message(&written);

        {
            let store = open_store(&file.path, &file.keys()).expect("the archive opens");
            store.put_record(PEER, &row).expect("the record is written");
        }
        let reopened = open_store(&file.path, &file.keys()).expect("it reopens");
        let read = reopened
            .get_record(PEER, 7)
            .expect("the record opens")
            .expect("the record is there");
        assert_eq!(read, row, "the sealed row comes back byte for byte");
        let back = read.to_message(PEER);
        assert_eq!(
            fields(&back),
            fields(&written),
            "id, chat, text, timestamp, direction and reply link survive"
        );
        assert_eq!(back.media, written.media, "the media kind survives");
        assert_eq!(back.media_id, written.media_id, "the media id survives");
        assert_eq!(
            back.is_service, written.is_service,
            "the service flag survives"
        );
        assert_eq!(
            reopened.all_records(PEER).expect("all records open"),
            vec![row],
            "a chat read is the same record"
        );
    }

    #[test]
    fn a_committed_record_survives_a_reopen_without_a_clean_shutdown() {
        // A power cut itself cannot be simulated in a unit test; this is the
        // other half of the promise — a committed write survives the process
        // going away without closing the connection. WAL and synchronous=FULL
        // are what cover the power-loss half, which needs real hardware.
        let (_dir, file) = scratch("history.json");
        let row = CachedMessage::from_message(&message(1, PEER, "committed"));

        {
            let store = open_store(&file.path, &file.keys()).expect("the archive opens");
            store.put_record(PEER, &row).expect("the write commits");
            // No close and no checkpoint: the handle is dropped as a killed
            // process would drop it.
        }
        let reopened = open_store(&file.path, &file.keys()).expect("it reopens");
        assert_eq!(
            reopened.get_record(PEER, 1).expect("the record opens"),
            Some(row),
            "what was committed is readable again"
        );
    }

    #[test]
    fn plaintext_bytes_are_refused_on_read() {
        let (_dir, file) = scratch("history.json");
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let chat_key = keyed_chat(&store.hmac_key, PEER);
        let record = Record {
            chat_key: chat_key.clone(),
            chat_id: PEER,
            message_id: 1,
            row: CachedMessage::from_message(&message(1, PEER, "plaintext")),
        };
        let plain = serde_json::to_vec(&record).expect("the fixture serialises");
        insert_record(&store.conn, &chat_key, 1, &plain).expect("the row is planted");

        assert!(
            store.get_record(PEER, 1).is_err(),
            "sealed::open's Ok(None) is a hard error, never a plaintext fallback"
        );
        assert_eq!(
            file.load(),
            LoadedHistory::empty(),
            "a load refuses the archive rather than reading the row as legacy"
        );
    }

    #[test]
    fn a_record_moved_to_another_chat_key_is_refused() {
        let (_dir, file) = scratch("history.json");
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let row = CachedMessage::from_message(&message(1, PEER, "private"));
        let (chat_key, envelope) = seal_record(&store.record_keys, &store.hmac_key, PEER, &row)
            .expect("the record is sealed for PEER");
        let moved = keyed_chat(&store.hmac_key, 99);
        insert_record(&store.conn, &moved, 1, &envelope).expect("the row is copied");

        assert_eq!(
            chat_key,
            keyed_chat(&store.hmac_key, PEER),
            "the original key"
        );
        assert!(
            store.get_record(99, 1).is_err(),
            "the copied row does not open under the chat it was moved to"
        );
        assert!(store.all_records(99).is_err(), "nor does a chat read of it");
    }

    #[test]
    fn a_replayed_record_under_another_address_is_refused() {
        let (_dir, file) = scratch("history.json");
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let row = CachedMessage::from_message(&message(1, PEER, "one"));
        let (chat_key, envelope) = seal_record(&store.record_keys, &store.hmac_key, PEER, &row)
            .expect("the record is sealed at id 1");
        insert_record(&store.conn, &chat_key, 2, &envelope).expect("the row is replayed at id 2");

        assert!(
            store.get_record(PEER, 2).is_err(),
            "the address inside the plaintext disagrees with the column"
        );
        assert_eq!(
            store.get_record(PEER, 1).expect("the original opens"),
            None,
            "the replay did not create a second copy of the original"
        );

        // Written twice at its own address, the row is replaced, not doubled.
        let other = CachedMessage::from_message(&message(1, PEER, "two"));
        let (_, second) = seal_record(&store.record_keys, &store.hmac_key, PEER, &other)
            .expect("a new record for the same address");
        insert_record(&store.conn, &chat_key, 1, &second).expect("the row is replaced");
        assert_eq!(
            store.get_record(PEER, 1).expect("the row opens"),
            Some(other),
            "one address holds one row, the last one written"
        );
    }

    #[test]
    fn sign_out_removes_the_database_and_its_wal_and_shm_siblings() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        let rows: Vec<Message> = (1..=50).map(|id| message(id, PEER, "row")).collect();
        cache.put(PEER, &rows);
        file.save(&cache, None);

        // Hold a connection open so SQLite keeps its -wal and -shm beside the
        // database: these are the files sign-out has to take with it.
        let open = open_store(&file.path, &file.keys()).expect("the archive opens");
        // WAL mode has no `-journal`; the other four are live while the
        // connection is open.
        let sibling = |suffix: &str| {
            let mut name = file.path.file_name().expect("a file name").to_os_string();
            name.push(suffix);
            file.path.with_file_name(name)
        };
        let live: Vec<PathBuf> = vec![
            file.path.clone(),
            sibling("-wal"),
            sibling("-shm"),
            salt_sibling(&file.path),
        ];
        assert!(
            live.iter().all(|path| path.exists()),
            "all live siblings exist"
        );
        let other = HistoryFile::new(file.path.clone(), other_keys());
        assert!(
            other.get(PEER).is_empty(),
            "another account cannot read these rows before sign-out"
        );

        file.clear();

        assert!(
            archive_siblings(&file.path)
                .iter()
                .all(|path| !path.exists()),
            "the database, its WAL, its shared memory, its journal and its salt are gone"
        );
        assert_eq!(
            file.load(),
            LoadedHistory::empty(),
            "nothing survives sign-out"
        );
        assert!(
            other.get(PEER).is_empty(),
            "and no other account reads the rows afterwards"
        );
        drop(open);
    }

    #[test]
    fn the_pragmas_are_applied_when_the_archive_opens() {
        let (_dir, file) = scratch("history.json");
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let journal: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal_mode reads");
        assert_eq!(journal, "wal");
        let synchronous: i64 = store
            .conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous reads");
        assert_eq!(synchronous, 2, "FULL");
        let cache_size: i64 = store
            .conn
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .expect("cache_size reads");
        assert_eq!(cache_size, -256, "256 KiB, not the default");
        let mmap_size: i64 = store
            .conn
            .query_row("PRAGMA mmap_size", [], |row| row.get(0))
            .expect("mmap_size reads");
        assert_eq!(mmap_size, 0, "no memory mapping");
        let journal_limit: i64 = store
            .conn
            .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
            .expect("journal_size_limit reads");
        assert!(journal_limit > 0, "the WAL is bounded");
        #[cfg(target_os = "macos")]
        {
            let fullfsync: i64 = store
                .conn
                .query_row("PRAGMA fullfsync", [], |row| row.get(0))
                .expect("fullfsync reads");
            assert_eq!(fullfsync, 1);
        }
    }

    #[test]
    fn a_save_round_trips_messages_and_the_account() {
        let (_dir, file) = scratch("history.json");
        let mut sent = message(2, PEER, "");
        sent.is_outgoing = true;
        sent.status = MessageStatus::Sent;
        sent.reply_to = Some(1);
        sent.media = Some(MediaKind::Photo);
        sent.media_id = Some(41);
        let written = vec![message(1, PEER, "first"), sent];
        let mut cache = HistoryCache::default();
        cache.put(PEER, &written);
        cache.put(30, &[message(9, 30, "elsewhere")]);

        file.save(&cache, Some("+15551234567"));

        let on_disk = fs::read(&file.path).expect("the database the save wrote");
        assert!(
            on_disk.starts_with(b"SQLite format 3\0"),
            "the target is a SQLite database"
        );
        assert!(
            !String::from_utf8_lossy(&on_disk).contains("first"),
            "no message body is readable on disk"
        );

        let loaded = file.load();
        assert_eq!(loaded.account.as_deref(), Some("+15551234567"));
        assert_eq!(loaded.cache, cache);
        let read = loaded.cache.get(PEER);
        assert_eq!(
            read.iter().map(fields).collect::<Vec<_>>(),
            written.iter().map(fields).collect::<Vec<_>>(),
            "every field the view draws comes back, oldest first"
        );
        assert_eq!(read[1].media, Some(MediaKind::Photo));
        assert_eq!(
            read[1].media_id,
            Some(41),
            "the media id comes back, so a forwarded copy is still found"
        );
        assert_eq!(ids(&loaded.cache.get(30)), vec![9]);

        // The round trip also goes through the range read, which opens the
        // archive and decodes just the stored stretch for one chat.
        assert_eq!(ids(&file.get(PEER)), vec![1, 2]);
        assert_eq!(file.text_bytes(), Some(cache.encoded_text_bytes()));
    }

    #[test]
    fn the_fullness_answer_compares_stored_bytes_with_the_configured_limit() {
        let (dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "four words of text")]);
        file.save(&cache, None);
        let used = file.text_bytes().expect("the archive reads");

        assert!(file.full(1), "one byte is below what is stored");
        assert!(!file.full(used as u64 + 1), "one byte more is room");
        assert!(!file.full(0), "zero is no limit, however full the archive");

        let empty = HistoryFile::for_test(dir.path().join("empty.history.json"));
        assert!(!empty.full(1), "an archive that does not exist is not full");
    }

    /// The slot's label keys the tag: an id slot's file carries the id, the
    /// legacy slot still accepts the configured phone, and a phone-only file
    /// from the legacy slot is another account's at the id slot.
    #[test]
    fn a_history_file_for_an_id_slot_carries_its_id_and_discards_another_account() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let cfg = crate::config::Config {
            phone: Some("+15551234567".to_owned()),
            ..crate::config::Config::default()
        };
        let legacy = crate::accounts::slot_paths(&config_path, &cfg, None);
        let seven = crate::accounts::slot_paths(&config_path, &cfg, Some(7));
        let seven_label = seven.label(&cfg);
        let legacy_label = legacy.label(&cfg);

        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "seven's")]);

        // What `Ready` stamps, and the next write lays down.
        let tag = HistoryTag {
            account_id: Some(7),
            phone: seven_label.clone(),
        };
        let file = HistoryFile::for_test(seven.history.clone());
        file.write(&HistoryFile::encode(&cache, &tag).expect("the cache serialises"));

        let loaded = file.load();
        assert_eq!(loaded.account_id, Some(7), "the id is the tag");
        assert_eq!(
            loaded.account.as_deref(),
            Some("7"),
            "and the slot's label rides beside it"
        );
        assert!(history_acceptable(
            loaded.account.as_deref(),
            loaded.account_id,
            seven_label.as_deref()
        ));
        assert!(tag.holds_for(7, None), "the id holds for its own account");
        assert!(
            !tag.holds_for(8, None),
            "another account's id is still discarded"
        );

        assert!(
            history_acceptable(Some("+15551234567"), None, legacy_label.as_deref()),
            "the legacy slot keeps accepting its configured phone"
        );
        assert!(
            !history_acceptable(Some("+15551234567"), None, seven_label.as_deref()),
            "a phone-only file is another account's at the id slot"
        );
    }

    /// A row writes the media id only when it has one: a file from before the
    /// id was kept carries no key, and reads back with none.
    #[test]
    fn a_row_carries_the_media_id_only_when_it_has_one() {
        let mut with_id = message(1, PEER, "");
        with_id.media = Some(MediaKind::Photo);
        with_id.media_id = Some(41);
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[with_id, message(2, PEER, "bare")]);

        let bytes =
            HistoryFile::encode(&cache, &HistoryTag::default()).expect("the cache serialises");
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).expect("the encoded payload is JSON");
        let rows = &payload["peers"]["7"];

        assert_eq!(rows[0]["media_id"].as_i64(), Some(41), "the id is written");
        assert!(
            rows[1].get("media_id").is_none(),
            "and a row without one omits the key, as an older file did"
        );
    }

    /// `CachedMedia` is this module's spelling of `MediaKind`, so a kind's tag
    /// round-trips through the file. The animated sticker's tag is new, which is
    /// why no existing tag changed and an older file still reads.
    #[test]
    fn every_media_kind_round_trips_through_the_files_spelling() {
        for kind in [
            MediaKind::Photo,
            MediaKind::Video,
            MediaKind::Gif,
            MediaKind::Voice,
            MediaKind::Sticker,
            MediaKind::AnimatedSticker,
            MediaKind::File,
        ] {
            let cached = CachedMedia::from(kind);
            let json = serde_json::to_string(&cached).expect("a media tag serialises");
            let read: CachedMedia = serde_json::from_str(&json).expect("and reads back");

            assert_eq!(read, cached, "{kind:?}");
            assert_eq!(MediaKind::from(read), kind);
        }
        assert_eq!(
            serde_json::to_string(&CachedMedia::AnimatedSticker).expect("a tag serialises"),
            "\"animatedsticker\"",
            "the file's spelling of the new kind"
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_cache() {
        let (_dir, file) = scratch("history.json");

        assert_eq!(file.load(), LoadedHistory::empty());
    }

    #[test]
    fn garbage_at_the_legacy_path_is_left_and_the_next_save_writes_the_database() {
        let (_dir, file) = scratch("history.json");
        fs::write(&file.legacy, "{ not json").expect("the legacy junk");

        assert_eq!(
            file.load(),
            LoadedHistory::empty(),
            "legacy junk is not read, warned about, launch carries on"
        );

        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "one")]);
        file.save(&cache, None);
        assert_eq!(
            ids(&file.load().cache.get(PEER)),
            vec![1],
            "the next save writes the archive"
        );
        assert_eq!(
            fs::read(&file.legacy).expect("the legacy file is still there"),
            b"{ not json",
            "the legacy file is left exactly as it was"
        );
    }

    const LEGACY: &str = r#"{"account": "+1555", "peers": {"7": [
        {"id": 4, "text": "legacy secret", "timestamp": 40, "is_outgoing": true}
    ]}}"#;

    /// Translated from `a_legacy_plaintext_file_is_rewritten_sealed`: the
    /// legacy file is now warned about once and left alone, never migrated.
    #[test]
    fn a_legacy_plaintext_file_is_left_on_disk_and_loads_empty() {
        let (_dir, file) = scratch("history.json");
        fs::write(&file.legacy, LEGACY).expect("the legacy file");

        let loaded = file.load();
        assert_eq!(loaded, LoadedHistory::empty(), "nothing is migrated");
        assert!(!file.path.exists(), "no database was created by a load");
        assert_eq!(
            fs::read(&file.legacy).expect("the legacy file is still there"),
            LEGACY.as_bytes(),
            "the plaintext stays exactly where it was"
        );
        assert_eq!(file.load(), loaded, "a second load is no different");
    }

    #[test]
    fn service_flag_round_trips_and_old_cache_rows_default_to_false() {
        let mut message = message(4, PEER, "joined Telegram");
        message.is_service = true;
        let cached = CachedMessage::from_message(&message);
        assert!(cached.to_message(PEER).is_service);

        let old: CachedMessage =
            serde_json::from_str(r#"{"id":4,"text":"old row","timestamp":40,"is_outgoing":false}"#)
                .expect("old cache row deserializes");
        assert!(!old.to_message(PEER).is_service);
    }

    /// Translated from `migration_wipes_the_pre_migration_bytes`: nothing is
    /// wiped now, and sign-out must not reach the legacy file either.
    #[cfg(unix)]
    #[test]
    fn the_legacy_file_is_neither_wiped_nor_removed() {
        let (dir, file) = scratch("history.json");
        fs::write(&file.legacy, LEGACY).expect("the legacy file");
        let unrelated = dir.path().join("other.json");
        fs::write(&unrelated, b"x").expect("an unrelated file");

        file.load();
        file.clear();

        assert_eq!(
            fs::read(&file.legacy).expect("the legacy file is still there"),
            LEGACY.as_bytes(),
            "the pre-migration bytes were not wiped"
        );
        assert_eq!(
            fs::read(&unrelated).expect("the unrelated file is still there"),
            b"x",
            "a file of another name was not touched"
        );
    }

    #[test]
    fn a_file_sealed_under_another_key_is_left_alone_and_loads_empty() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "one")]);
        file.save(&cache, None);
        let before = fs::read(&file.path).expect("the sealed file");

        let stranger = HistoryFile::new(file.path.clone(), other_keys());

        assert_eq!(stranger.load(), LoadedHistory::empty());
        assert_eq!(
            fs::read(&file.path).expect("the file is still there"),
            before,
            "a wrong key never destroys what the right one could read"
        );
        assert_eq!(file.load().cache, cache, "the right key still reads it");
    }

    /// A key source that cannot be reached: a locked keyring.
    #[derive(Debug)]
    struct Locked;

    impl KeyProvider for Locked {
        fn sealing_key(
            &self,
        ) -> Result<(sealed::Salt, telegram_framework::FileKey), telegram_framework::SessionError>
        {
            Err(telegram_framework::SessionError::Unavailable(
                "locked".into(),
            ))
        }

        fn opening_key(
            &self,
            _salt: &sealed::Salt,
        ) -> Result<telegram_framework::FileKey, telegram_framework::SessionError> {
            Err(telegram_framework::SessionError::Unavailable(
                "locked".into(),
            ))
        }
    }

    #[test]
    fn an_unreachable_key_source_skips_the_write_and_loads_empty() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "one")]);
        file.save(&cache, None);
        let before = fs::read(&file.path).expect("the sealed file");

        let locked = HistoryFile::new(file.path.clone(), Arc::new(Locked));
        assert_eq!(locked.load(), LoadedHistory::empty());
        locked.save(&HistoryCache::default(), None);

        assert_eq!(
            fs::read(&file.path).expect("the file is still there"),
            before,
            "nothing is written, and nothing is lost"
        );
    }

    /// Translated from `a_save_leaves_a_whole_file_and_no_temp_behind`: the
    /// whole-file envelope is now one sealed row in a versioned database.
    #[test]
    fn a_save_leaves_a_sealed_database_and_schema_v1_behind() {
        let (_dir, file) = scratch("history.json");
        let mut photo = message(1, PEER, "");
        photo.media = Some(MediaKind::Photo);
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[photo]);

        file.save(&cache, Some("+15551234567"));

        let raw = fs::read(&file.path).expect("the database the save wrote");
        assert!(
            raw.starts_with(b"SQLite format 3\0"),
            "the target is a database"
        );
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let version: i64 = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_VERSION],
                |row| row.get(0),
            )
            .expect("the schema version was recorded");
        assert_eq!(version, SCHEMA_VERSION);
        let state = store.state().expect("the sealed state opens");
        assert_eq!(state.account.as_deref(), Some("+15551234567"));
        let rows = store.all_records(PEER).expect("the sealed record opens");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].media,
            Some(CachedMedia::Photo),
            "media as a kind only"
        );
        let envelope: Vec<u8> = store
            .conn
            .query_row("SELECT sealed FROM messages", [], |row| row.get(0))
            .expect("the sealed row");
        assert!(sealed::is_sealed(&envelope), "the row is an envelope");
        assert!(
            !fs::read_dir(file.path.parent().expect("a parent directory"))
                .expect("the directory reads")
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp")),
            "no half-written temp is left beside the target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_dir, file) = scratch("history.json");
        file.save(&HistoryCache::default(), None);

        let mode = fs::metadata(&file.path)
            .expect("the file the save wrote")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn clearing_a_missing_file_succeeds_and_clearing_removes() {
        let (_dir, file) = scratch("history.json");

        file.clear();

        file.save(&HistoryCache::default(), None);
        assert!(file.path.exists());
        file.clear();
        assert!(!file.path.exists());
    }

    #[test]
    fn a_legacy_phone_tag_still_accepts_only_the_configured_phone() {
        assert!(
            history_acceptable(Some("+1555"), None, Some("+1555")),
            "same"
        );
        assert!(
            history_acceptable(None, None, None),
            "unnamed matches unnamed"
        );
        assert!(
            !history_acceptable(Some("+15550000001"), None, Some("+15550000002")),
            "a named mismatch is someone else's conversations"
        );
        assert!(
            !history_acceptable(None, None, Some("+1555")),
            "an untagged file may be anyone's: a false clear costs a fetch"
        );
        assert!(
            !history_acceptable(Some("+1555"), None, None),
            "a tagged file under an unnamed launch may be someone else's"
        );
    }

    #[test]
    fn an_account_id_is_accepted_at_launch_whatever_the_configured_phone() {
        assert!(history_acceptable(None, Some(7), Some("+1555")));
        assert!(history_acceptable(None, Some(7), None));
    }

    #[test]
    fn the_post_ready_rule_compares_the_id_then_the_legacy_phone() {
        let id = |id| HistoryTag {
            account_id: Some(id),
            phone: None,
        };
        let phone = |phone: &str| HistoryTag {
            account_id: None,
            phone: Some(phone.to_owned()),
        };
        assert!(id(7).holds_for(7, Some("+1")), "the phone plays no part");
        assert!(!id(7).holds_for(8, None));
        assert!(phone("+1").holds_for(7, Some("+1")));
        assert!(!phone("+1").holds_for(7, Some("+2")));
        assert!(phone("+1").holds_for(7, None), "unknown is not a mismatch");
        assert!(HistoryTag::default().holds_for(7, Some("+2")));
    }

    #[test]
    fn a_payload_round_trips_the_signed_in_account_id() {
        let (_dir, file) = scratch("history.json");
        let bytes = HistoryFile::encode(
            &HistoryCache::default(),
            &HistoryTag {
                account_id: Some(42),
                phone: None,
            },
        )
        .expect("the cache serialises");
        file.write(&bytes);

        let loaded = file.load();
        assert_eq!(loaded.account_id, Some(42));
        assert_eq!(loaded.account, None);
    }

    #[test]
    fn a_mismatched_account_id_does_not_seed_the_cache() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "someone else's")]);
        let tag = HistoryTag {
            account_id: Some(1),
            phone: None,
        };
        file.write(&HistoryFile::encode(&cache, &tag).expect("the cache serialises"));

        let loaded = file.load();
        let live = HistoryTag {
            account_id: loaded.account_id,
            phone: loaded.account,
        };
        assert!(live.holds_for(1, None));
        assert!(!live.holds_for(2, None), "another account's cache goes");
    }

    #[test]
    fn a_mismatched_file_does_not_seed_the_cache() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "someone else's")]);
        assert!(cache.set_chats(&[chat(PEER, "someone else's friend")]));
        file.save(&cache, Some("+10000000001"));

        let loaded = file.load();
        let seeded = if history_acceptable(
            loaded.account.as_deref(),
            loaded.account_id,
            Some("+19999999999"),
        ) {
            loaded.cache
        } else {
            HistoryCache::default()
        };

        assert!(
            seeded.get(PEER).is_empty(),
            "messages saved under another account never reach the window"
        );
        assert!(
            seeded.chats().is_empty(),
            "nor does the list they were read from: one tag guards both"
        );
    }

    // ---- the chat list ---------------------------------------------------

    fn chat(id: i64, title: &str) -> Chat {
        Chat {
            read_outbox_max_id: None,
            id,
            title: title.to_owned(),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            muted: false,
            unread_mark: false,
            presence: None,
            deleted: false,
        }
    }

    /// The fields a row of the list draws, as one comparable value: `Chat`
    /// itself has no `PartialEq`.
    fn row(chat: &Chat) -> (i64, &str, Option<&str>, u32, Option<i64>, Option<i64>, bool) {
        (
            chat.id,
            &chat.title,
            chat.last_message.as_deref(),
            chat.unread_count,
            chat.last_message_id,
            chat.last_timestamp,
            chat.pinned,
        )
    }

    #[test]
    fn a_save_round_trips_the_chat_list_without_presence() {
        let (_dir, file) = scratch("history.json");
        let mut pinned = chat(30, "pinned");
        pinned.pinned = true;
        pinned.unread_count = 2;
        pinned.last_message = Some(Cow::Borrowed("see you"));
        pinned.last_message_id = Some(41);
        pinned.last_timestamp = Some(410);
        pinned.presence = Some(domain::presence::Presence::Online);
        let listed = vec![pinned, chat(PEER, "seven")];
        let mut cache = HistoryCache::default();
        assert!(cache.set_chats(&listed));

        file.save(&cache, Some("+1555"));

        let loaded = file.load().cache;
        assert_eq!(loaded, cache);
        let read = loaded.chats();
        assert_eq!(
            read.iter().map(row).collect::<Vec<_>>(),
            listed.iter().map(row).collect::<Vec<_>>(),
            "every field a row draws comes back, in the list's order"
        );
        assert!(
            read.iter()
                .all(|chat| chat.presence.is_none() && chat.is_private()),
            "and no presence, which was stale the moment it was written"
        );
    }

    #[test]
    fn the_same_list_again_changes_nothing() {
        let mut cache = HistoryCache::default();
        let listed = vec![chat(1, "one"), chat(2, "two")];
        assert!(cache.set_chats(&listed));

        let mut online = listed.clone();
        online[0].presence = Some(domain::presence::Presence::Online);
        assert!(!cache.set_chats(&online), "a presence is not kept");

        let mut previewed = listed.clone();
        previewed[1].last_message = Some(Cow::Borrowed("new"));
        assert!(cache.set_chats(&previewed), "a preview is");
        assert!(cache.set_chats(&[]), "and an emptied list empties it");
        assert!(cache.chats().is_empty());
    }

    #[test]
    fn only_private_chats_are_cached_and_only_the_head_of_the_list() {
        let mut cache = HistoryCache::default();
        let mut bot = chat(1, "bot");
        bot.kind = ChatKind::Bot;
        let count = i64::try_from(HISTORY_CACHE_CHATS).expect("the cap fits an id");
        let listed: Vec<Chat> = std::iter::once(bot)
            .chain((2..=count + 5).map(|id| chat(id, "person")))
            .collect();

        assert!(cache.set_chats(&listed));
        let kept = cache.chats();
        assert_eq!(kept.len(), HISTORY_CACHE_CHATS);
        assert_eq!(kept.first().map(|chat| chat.id), Some(2), "no bot");
        assert_eq!(kept.last().map(|chat| chat.id), Some(count + 1));
        assert!(
            !cache.set_chats(&listed),
            "the same list again is the same cut, so nothing changed"
        );
    }

    #[test]
    fn muted_chats_round_trip_and_older_rows_default_to_unmuted() {
        let mut source = chat(PEER, "Ada");
        source.muted = true;
        let cached = CachedChat::from_chat(&source);
        let encoded = serde_json::to_string(&cached).expect("the chat serializes");
        let restored: CachedChat = serde_json::from_str(&encoded).expect("the chat parses");
        assert!(restored.to_chat().muted);

        let older: CachedChat =
            serde_json::from_str(r#"{"id":7,"title":"Ada"}"#).expect("an older cached row parses");
        assert!(!older.to_chat().muted);
    }

    #[test]
    fn unread_marks_round_trip_and_older_rows_default_to_unmarked() {
        let mut source = chat(PEER, "Ada");
        source.unread_mark = true;
        let cached = CachedChat::from_chat(&source);
        let encoded = serde_json::to_string(&cached).expect("the chat serializes");
        let restored: CachedChat = serde_json::from_str(&encoded).expect("the chat parses");
        assert!(restored.to_chat().unread_mark);

        let older: CachedChat =
            serde_json::from_str(r#"{"id":7,"title":"Ada"}"#).expect("an older cached row parses");
        assert!(!older.to_chat().unread_mark);
    }

    /// A file written before the list was cached has no `chats` at all, and
    /// still seeds its messages.
    #[test]
    fn a_file_without_a_chat_list_loads_its_messages() {
        let (_dir, file) = scratch("history.json");
        seed(
            &file,
            r#"{"account": "+1555", "peers": {"7": [
                {"id": 4, "text": "hello", "timestamp": 40, "is_outgoing": true}
            ]}}"#,
        );

        let loaded = file.load();
        assert_eq!(ids(&loaded.cache.get(PEER)), vec![4]);
        assert!(loaded.cache.chats().is_empty());
    }

    #[test]
    fn the_depth_cap_evicts_the_oldest() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let page: Vec<Message> = (1..=depth + 5).map(|id| message(id, PEER, "row")).collect();
        let mut cache = HistoryCache::default();

        cache.put(PEER, &page);

        let kept = ids(&cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(
            kept.first(),
            Some(&6),
            "the five oldest went, not the newest"
        );
        assert_eq!(kept.last(), Some(&(depth + 5)));
    }

    #[test]
    fn placeholders_and_other_conversations_never_enter_the_cache() {
        let mut cache = HistoryCache::default();

        cache.put(
            PEER,
            &[
                message(1, PEER, "numbered"),
                message(0, PEER, "zero"),
                message(-3, PEER, "a send in flight"),
                message(2, 99, "another conversation"),
            ],
        );

        assert_eq!(ids(&cache.get(PEER)), vec![1]);
        assert!(cache.get(99).is_empty());
    }

    #[test]
    fn a_put_replaces_and_an_empty_put_or_remove_forgets() {
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "old")]);

        cache.put(PEER, &[message(2, PEER, "new")]);
        assert_eq!(ids(&cache.get(PEER)), vec![2], "replaced, not merged");

        cache.put(PEER, &[message(-1, PEER, "placeholder only")]);
        assert_eq!(
            cache,
            HistoryCache::default(),
            "nothing cacheable is no entry"
        );

        cache.put(PEER, &[message(3, PEER, "again")]);
        cache.remove(PEER);
        assert_eq!(cache, HistoryCache::default());
    }

    /// A hand-written file loads, and is held to the bounds a saved one is:
    /// the placeholder goes, the status comes from the direction, and the
    /// peer from the key.
    #[test]
    fn a_hand_written_file_seeds_the_cache_within_its_bounds() {
        let (_dir, file) = scratch("history.json");
        seed(
            &file,
            r#"{"account": "+1555", "peers": {"7": [
                {"id": -1, "text": "never sent", "timestamp": 1, "is_outgoing": true},
                {"id": 4, "text": "hello", "timestamp": 40, "is_outgoing": true, "reply_to": 3},
                {"id": 5, "text": "", "timestamp": 50, "is_outgoing": false, "media": "voice"}
            ]}}"#,
        );

        let loaded = file.load();
        assert!(history_acceptable(
            loaded.account.as_deref(),
            loaded.account_id,
            Some("+1555")
        ));

        let read = loaded.cache.get(PEER);
        assert_eq!(ids(&read), vec![4, 5]);
        assert_eq!(
            fields(&read[0]),
            (4, PEER, "hello", 40, MessageStatus::Sent, true, Some(3))
        );
        assert_eq!(read[1].status, MessageStatus::Received);
        assert_eq!(read[1].media, Some(MediaKind::Voice));
        assert_eq!(read[1].media_id, None, "an older file wrote no id");
        assert_eq!(read[1].display_body(), "[voice]");
    }

    // ---- merging a page --------------------------------------------------

    /// Messages of [`PEER`] with these ids, in the order given.
    fn page(ids: impl IntoIterator<Item = i64>) -> Vec<Message> {
        ids.into_iter().map(|id| message(id, PEER, "row")).collect()
    }

    /// A cache holding these ids for [`PEER`].
    fn cached(ids: impl IntoIterator<Item = i64>) -> HistoryCache {
        let mut cache = HistoryCache::default();
        cache.put(PEER, &page(ids));
        cache
    }

    const LATEST: PageKind = PageKind::Latest { whole: false };

    #[test]
    fn a_page_into_an_empty_cache_is_the_cache_only_when_it_is_the_latest() {
        let mut cache = HistoryCache::default();
        assert!(cache.merge(PEER, &page(1..=3), LATEST));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);

        for kind in [
            PageKind::Older {
                before: 10,
                reached_start: false,
            },
            PageKind::Newer {
                after: 0,
                reached_end: false,
            },
            PageKind::Around,
        ] {
            let mut cache = HistoryCache::default();
            assert!(
                !cache.merge(PEER, &page(1..=3), kind),
                "{kind:?} cannot be shown to be the newest end"
            );
            assert_eq!(cache, HistoryCache::default());
        }
    }

    #[test]
    fn an_identical_page_changes_nothing() {
        let mut cache = cached(1..=5);
        let before = cache.clone();

        assert!(!cache.merge(PEER, &page(1..=5), LATEST));
        assert!(!cache.merge(PEER, &page(2..=4), PageKind::Around));
        assert_eq!(cache, before);
    }

    #[test]
    fn an_overlapping_latest_page_keeps_the_older_rows_and_adds_the_newer() {
        let mut cache = cached(1..=5);

        assert!(cache.merge(PEER, &page(4..=8), LATEST));
        assert_eq!(ids(&cache.get(PEER)), (1..=8).collect::<Vec<_>>());
    }

    #[test]
    fn an_edited_message_is_replaced_by_the_wire_copy() {
        let mut cache = cached(1..=3);
        let mut edited = page(1..=3);
        edited[1].text = Cow::Borrowed("edited");

        assert!(cache.merge(PEER, &edited, LATEST));
        let read = cache.get(PEER);
        assert_eq!(ids(&read), vec![1, 2, 3]);
        assert_eq!(&*read[1].text, "edited");
    }

    #[test]
    fn a_message_missing_inside_the_page_is_deleted() {
        let mut cache = cached(1..=6);

        assert!(cache.merge(PEER, &page([3, 5, 6]), LATEST));
        assert_eq!(
            ids(&cache.get(PEER)),
            vec![1, 2, 3, 5, 6],
            "4 is inside what the page speaks for, so it is gone; 1 and 2 are not"
        );

        let mut cache = cached(1..=6);
        assert!(cache.merge(PEER, &page([2, 5]), PageKind::Around));
        assert_eq!(
            ids(&cache.get(PEER)),
            vec![1, 2, 5, 6],
            "an around page speaks for its own oldest to newest only"
        );
    }

    #[test]
    fn a_latest_page_drops_cached_rows_newer_than_it() {
        let mut cache = cached(1..=6);

        assert!(cache.merge(PEER, &page(3..=4), LATEST));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_short_latest_page_is_the_whole_conversation() {
        let mut cache = cached(1..=6);

        assert!(cache.merge(PEER, &page(3..=4), PageKind::Latest { whole: true }));
        assert_eq!(
            ids(&cache.get(PEER)),
            vec![3, 4],
            "a page that reached the start leaves nothing older to keep"
        );
    }

    #[test]
    fn a_latest_page_past_a_gap_replaces_the_cache() {
        let mut cache = cached(1..=5);

        assert!(cache.merge(PEER, &page(20..=25), LATEST));
        assert_eq!(
            ids(&cache.get(PEER)),
            (20..=25).collect::<Vec<_>>(),
            "nothing proves 6..20 empty, so the old run cannot be kept"
        );
    }

    #[test]
    fn an_older_page_from_the_cached_oldest_is_prepended() {
        let mut cache = cached(10..=15);
        let older = PageKind::Older {
            before: 10,
            reached_start: false,
        };

        assert!(cache.merge(PEER, &page([3, 5, 7]), older));
        assert_eq!(ids(&cache.get(PEER)), vec![3, 5, 7, 10, 11, 12, 13, 14, 15]);
    }

    #[test]
    fn a_short_older_page_speaks_for_everything_below_its_anchor() {
        let mut cache = cached(1..=15);
        let older = PageKind::Older {
            before: 10,
            reached_start: true,
        };

        assert!(cache.merge(PEER, &page([5, 7]), older));
        assert_eq!(ids(&cache.get(PEER)), vec![5, 7, 10, 11, 12, 13, 14, 15]);
    }

    #[test]
    fn a_page_that_does_not_reach_the_cached_run_is_ignored() {
        let mut cache = cached(10..=15);
        let before = cache.clone();

        for kind in [
            // Counted from below the cached oldest: 9 may be missing.
            PageKind::Older {
                before: 8,
                reached_start: false,
            },
            // Counted from above the cached newest: 16 may be missing.
            PageKind::Newer {
                after: 17,
                reached_end: false,
            },
        ] {
            assert!(!cache.merge(PEER, &page(1..=5), kind), "{kind:?}");
            assert!(!cache.merge(PEER, &page(20..=25), kind), "{kind:?}");
        }
        assert!(!cache.merge(PEER, &page(1..=5), PageKind::Around));
        assert!(!cache.merge(PEER, &page(20..=25), PageKind::Around));
        assert_eq!(cache, before);
    }

    #[test]
    fn a_newer_page_from_the_cached_newest_extends_the_run() {
        let mut cache = cached(1..=5);
        let newer = PageKind::Newer {
            after: 5,
            reached_end: false,
        };

        assert!(cache.merge(PEER, &page([8, 9]), newer));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3, 4, 5, 8, 9]);
    }

    #[test]
    fn a_jump_page_that_overlaps_the_run_is_merged() {
        let mut cache = cached(10..=15);

        assert!(cache.merge(PEER, &page([7, 9, 11, 12]), PageKind::Around));
        assert_eq!(
            ids(&cache.get(PEER)),
            vec![7, 9, 11, 12, 13, 14, 15],
            "10 sat between the page's ends and is gone"
        );
    }

    #[test]
    fn placeholders_and_other_conversations_are_dropped_from_a_page() {
        let mut cache = cached(1..=3);
        let mut mixed = page([-2, 0, 3, 4]);
        mixed.push(message(5, 99, "another conversation"));

        assert!(cache.merge(PEER, &mixed, LATEST));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3, 4]);
        assert!(cache.get(99).is_empty());

        assert!(
            !cache.merge(PEER, &page([-1, 0]), LATEST),
            "a page of placeholders only is an empty page"
        );
    }

    #[test]
    fn the_cap_keeps_the_newest_when_pages_merge() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let mut cache = cached(101..=100 + depth);
        let full = cache.clone();
        let older = PageKind::Older {
            before: 101,
            reached_start: false,
        };

        assert!(
            !cache.merge(PEER, &page(1..=100), older),
            "a full cache has no room for older rows"
        );
        assert_eq!(cache, full);

        let newer = PageKind::Newer {
            after: 100 + depth,
            reached_end: true,
        };
        assert!(cache.merge(PEER, &page(101 + depth..=110 + depth), newer));
        let kept = ids(&cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(kept.first(), Some(&111), "the ten oldest went");
        assert_eq!(kept.last(), Some(&(110 + depth)));
    }

    #[test]
    fn an_empty_page_never_wipes_the_cache() {
        let mut cache = cached(1..=3);
        let before = cache.clone();

        for kind in [
            LATEST,
            PageKind::Latest { whole: true },
            PageKind::Older {
                before: 1,
                reached_start: true,
            },
            PageKind::Newer {
                after: 3,
                reached_end: true,
            },
            PageKind::Around,
        ] {
            assert!(!cache.merge(PEER, &[], kind), "{kind:?}");
        }
        assert_eq!(cache, before);
    }

    #[test]
    fn a_page_out_of_order_or_with_repeats_is_cached_oldest_first_once() {
        let mut cache = HistoryCache::default();

        assert!(cache.merge(PEER, &page([3, 1, 2, 3]), LATEST));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);
    }

    // ---- folding in the feed ---------------------------------------------

    /// A cache whose run for [`PEER`] is `ids`, made current the way the loop
    /// makes it: by a latest page landing.
    fn current(ids: impl IntoIterator<Item = i64>) -> HistoryCache {
        let mut cache = HistoryCache::default();
        assert!(cache.merge(PEER, &page(ids), LATEST));
        cache
    }

    fn arrival(id: i64) -> UpdateEvent {
        UpdateEvent::NewMessage(message(id, PEER, "new"))
    }

    fn edit(chat_id: i64, message_id: i64, text: &'static str) -> UpdateEvent {
        UpdateEvent::MessageEdited {
            chat_id,
            message_id,
            new_text: Cow::Borrowed(text),
        }
    }

    fn deletion(ids: &[i64]) -> UpdateEvent {
        UpdateEvent::MessagesDeleted {
            message_ids: ids.to_vec(),
        }
    }

    #[test]
    fn an_edit_replaces_the_cached_text_in_place() {
        let mut cache = cached(1..=3);

        assert!(cache.apply_update(&edit(PEER, 2, "edited")));
        let read = cache.get(PEER);
        assert_eq!(ids(&read), vec![1, 2, 3]);
        assert_eq!(&*read[1].text, "edited");

        assert!(
            !cache.apply_update(&edit(PEER, 2, "edited")),
            "the same text again changes nothing"
        );
    }

    #[test]
    fn an_edit_of_a_message_not_cached_changes_nothing() {
        let mut cache = cached(1..=3);
        let before = cache.clone();

        assert!(!cache.apply_update(&edit(PEER, 9, "edited")), "not cached");
        assert!(
            !cache.apply_update(&edit(99, 2, "edited")),
            "the id is cached, but under another conversation"
        );
        assert_eq!(cache, before);
    }

    #[test]
    fn a_deletion_names_no_peer_and_removes_from_every_one() {
        let mut cache = cached(1..=3);
        cache.put(
            30,
            &[message(4, 30, "elsewhere"), message(5, 30, "elsewhere")],
        );

        assert!(cache.apply_update(&deletion(&[2, 4])));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 3]);
        assert_eq!(ids(&cache.get(30)), vec![5]);

        assert!(!cache.apply_update(&deletion(&[42])), "nothing cached");

        assert!(cache.apply_update(&deletion(&[1, 3])));
        assert!(cache.get(PEER).is_empty());
        assert_eq!(
            cache,
            {
                let mut left = HistoryCache::default();
                left.put(30, &[message(5, 30, "elsewhere")]);
                left
            },
            "an emptied peer is no entry"
        );
    }

    #[test]
    fn a_new_message_joins_a_run_a_latest_page_made_current() {
        let mut cache = current(1..=3);

        assert!(cache.apply_update(&arrival(4)));
        assert!(cache.apply_update(&arrival(6)));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3, 4, 6]);
    }

    /// Yesterday's run, as the file restores it: a message now is no proof
    /// that nothing came between, so it is left out.
    #[test]
    fn a_new_message_is_ignored_while_the_run_is_not_known_current() {
        let mut cache = cached(1..=100);
        let before = cache.clone();

        assert!(!cache.apply_update(&arrival(150)));
        assert_eq!(cache, before);

        let mut cache = HistoryCache::default();
        assert!(
            !cache.apply_update(&arrival(150)),
            "nor does one start a run of its own"
        );
        assert_eq!(cache, HistoryCache::default());

        let mut cache = cached(1..=3);
        assert!(
            !cache.merge(PEER, &page([2, 3]), PageKind::Around,),
            "only a latest page speaks for the newest end"
        );
        assert!(!cache.apply_update(&arrival(4)));
    }

    #[test]
    fn an_empty_latest_page_does_not_make_a_run_current() {
        let mut cache = cached(1..=3);

        assert!(!cache.merge(PEER, &[], LATEST));
        assert!(!cache.apply_update(&arrival(9)));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);
    }

    /// A latest page fetched just before a message arrived does not carry it,
    /// so the run it lands as is not current — or the next arrival would be
    /// appended past the one it missed.
    #[test]
    fn a_latest_page_the_feed_has_overtaken_is_not_current() {
        let mut cache = HistoryCache::default();
        assert!(
            !cache.apply_update(&arrival(4)),
            "seen while nothing is current"
        );

        assert!(cache.merge(PEER, &page(1..=3), LATEST));
        assert!(!cache.apply_update(&arrival(5)), "4 is missing below it");
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);

        assert!(cache.merge(PEER, &page(1..=5), LATEST));
        assert!(
            cache.apply_update(&arrival(6)),
            "a page that reached 5 is current"
        );
        assert_eq!(ids(&cache.get(PEER)), (1..=6).collect::<Vec<_>>());
    }

    /// A stale page that drops what the feed added leaves the run short of a
    /// message that exists, so it is no longer current.
    #[test]
    fn a_page_that_drops_a_fed_message_takes_the_mark_away() {
        let mut cache = current(1..=3);
        assert!(cache.apply_update(&arrival(4)));

        assert!(cache.merge(PEER, &page(1..=3), LATEST));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);
        assert!(!cache.apply_update(&arrival(5)));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);
    }

    /// Deleting the newest message is the conversation's own doing, and a run
    /// that matches it is still current.
    #[test]
    fn deleting_the_newest_message_keeps_the_run_current() {
        let mut cache = current(1..=3);
        assert!(cache.apply_update(&arrival(4)));
        assert!(cache.apply_update(&deletion(&[4])));

        assert!(!cache.merge(PEER, &page(1..=3), LATEST), "the same run");
        assert!(cache.apply_update(&arrival(5)));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3, 5]);
    }

    #[test]
    fn an_interrupted_feed_leaves_every_run_stale_until_its_next_latest_page() {
        let mut cache = current(1..=3);

        cache.feed_interrupted();
        assert!(!cache.apply_update(&arrival(4)));
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);

        assert!(cache.merge(PEER, &page(1..=4), LATEST));
        assert!(cache.apply_update(&arrival(5)));
        assert_eq!(ids(&cache.get(PEER)), (1..=5).collect::<Vec<_>>());
    }

    #[test]
    fn an_arrival_lands_where_its_id_puts_it_once() {
        let mut cache = current([10, 12]);

        assert!(
            cache.apply_update(&arrival(11)),
            "a send's answer came first"
        );
        assert_eq!(ids(&cache.get(PEER)), vec![10, 11, 12]);

        assert!(!cache.apply_update(&arrival(11)), "the same copy again");
        let mut edited = message(11, PEER, "the feed's copy");
        edited.is_outgoing = true;
        assert!(cache.arrive(&edited), "a different copy replaces it");
        assert_eq!(ids(&cache.get(PEER)), vec![10, 11, 12]);
        assert_eq!(&*cache.get(PEER)[1].text, "the feed's copy");

        assert!(
            !cache.apply_update(&arrival(9)),
            "older than the whole run is outside it"
        );
        assert_eq!(ids(&cache.get(PEER)), vec![10, 11, 12]);
    }

    #[test]
    fn a_placeholder_never_arrives() {
        let mut cache = current(1..=3);
        let before = cache.clone();

        assert!(!cache.apply_update(&arrival(0)));
        assert!(!cache.apply_update(&arrival(-4)));
        assert_eq!(cache, before);
    }

    #[test]
    fn an_arrival_into_a_full_run_drops_the_oldest() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let mut cache = current(1..=depth);

        assert!(cache.apply_update(&arrival(depth + 1)));
        let kept = ids(&cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(kept.first(), Some(&2));
        assert_eq!(kept.last(), Some(&(depth + 1)));
    }

    #[test]
    fn events_that_are_not_about_a_message_change_nothing() {
        let mut cache = current(1..=3);

        for event in [
            UpdateEvent::ReadReceipt {
                chat_id: PEER,
                max_id: 3,
            },
            UpdateEvent::PeerTyping {
                chat_id: PEER,
                typing: true,
            },
            UpdateEvent::PeerStatus {
                chat_id: PEER,
                presence: domain::presence::Presence::Online,
            },
        ] {
            assert!(!cache.apply_update(&event), "{event:?}");
        }
    }

    /// A hand-written peer deeper than the cap is cut on load, oldest first.
    #[test]
    fn an_over_deep_file_is_cut_to_the_cap_on_load() {
        let (_dir, file) = scratch("history.json");
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let rows: Vec<CachedMessage> = (1..=depth + 1)
            .map(|id| CachedMessage::from_message(&message(id, PEER, "row")))
            .collect();
        let payload = Payload {
            account: None,
            account_id: None,
            peers: BTreeMap::from([(PEER, rows)]),
            chats: Vec::new(),
            aliases: BTreeMap::new(),
            chips: BTreeMap::new(),
            removed: BTreeSet::new(),
            deleted_messages: BTreeSet::new(),
            updated_messages: BTreeMap::new(),
        };
        seed(
            &file,
            serde_json::to_vec(&payload).expect("the payload serialises"),
        );

        let kept = ids(&file.load().cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(kept.first(), Some(&2));
    }

    /// A launch reads only the newest window of each chat, not the whole
    /// archive. Every row stays on disk and is still reachable by a range read.
    #[test]
    fn load_reads_only_the_newest_window_of_each_chat() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let (_dir, file, mut store) = scratch_store("history.json");
        for peer in [PEER, PEER + 1] {
            let ids: Vec<i64> = (1..=depth * 3 + 7).collect();
            let rows: Vec<CachedMessage> = ids
                .iter()
                .map(|id| CachedMessage::from_message(&message(*id, peer, "row")))
                .collect();
            assert!(
                store
                    .ingest_page(peer, &rows, (1, depth * 3 + 7))
                    .expect("the rows land")
            );
        }
        drop(store);

        let loaded = file.load().cache;
        let expected: Vec<i64> = ((depth * 2 + 8)..=depth * 3 + 7).collect();
        for peer in [PEER, PEER + 1] {
            assert_eq!(
                ids(&loaded.get(peer)),
                expected,
                "only the newest window loads"
            );
        }

        // The rows the load did not read are still in the archive.
        assert_eq!(
            ids(&file.get(PEER)).len(),
            usize::try_from(depth * 3 + 7).expect("the count fits")
        );
    }

    // ---- Worst case: control-character text at the byte budget -------------

    #[test]
    fn encoded_len_mirrors_serde_json() {
        assert_eq!(encoded_len("hello"), 5);
        assert_eq!(encoded_len("\"\\"), 4);
        assert_eq!(encoded_len("\u{1}"), 6);
        assert_eq!(encoded_len("\n\r\t\u{8}\u{c}"), 10);
        assert_eq!(encoded_len("é€😀"), 2 + 3 + 4);
        assert_eq!(encoded_len("a\"b\u{1f}\n"), 1 + 2 + 1 + 6 + 2);
        for text in [
            "plain",
            "q\"\\",
            "\u{0}\u{1f}\u{7f}",
            "\n\r\t\u{8}\u{c}",
            "é€😀",
        ] {
            let json = serde_json::to_string(text).expect("a string serialises");
            assert_eq!(encoded_len(text), json.len() - 2, "{text:?}");
        }
    }

    /// A cache at the document's shape: [`HISTORY_CACHE_PEERS`] conversations of
    /// [`HISTORY_CACHE_DEPTH`] rows, each row's text `per_row` characters
    /// of `'\u{1}'`, one UTF-8 byte that JSON-escapes to six (`\u0001`).
    fn control_char_cache(per_row: usize) -> HistoryCache {
        let text = "\u{1}".repeat(per_row);
        let mut cache = HistoryCache::default();
        for peer in 1..=i64::try_from(HISTORY_CACHE_PEERS).expect("a small bound") {
            let rows: Vec<Message> = (1..=i64::try_from(HISTORY_CACHE_DEPTH)
                .expect("a small bound"))
                .map(|id| message(id, peer, &text))
                .collect();
            cache.put(peer, &rows);
        }
        cache
    }

    /// This process's resident set size, by the same per-OS route as
    /// `examples/memory_harness.rs`: `/proc/self/status` on Linux,
    /// `proc_pidinfo` on macOS, nothing elsewhere.
    #[cfg(target_os = "linux")]
    fn resident_bytes() -> Option<u64> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|line| {
            let rest = line.strip_prefix("VmRSS:")?;
            rest.split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kb| kb * 1024)
        })
    }

    #[cfg(target_os = "macos")]
    fn resident_bytes() -> Option<u64> {
        use std::ffi::c_void;
        use std::mem::size_of;

        /// The kernel's `struct proc_taskinfo`; `resident_size` is the field read.
        #[repr(C)]
        #[derive(Default)]
        struct ProcTaskInfo {
            virtual_size: u64,
            resident_size: u64,
            total_user: u64,
            total_system: u64,
            threads_user: u64,
            threads_system: u64,
            policy: i32,
            faults: i32,
            pageins: i32,
            cow_faults: i32,
            messages_sent: i32,
            messages_received: i32,
            syscalls_mach: i32,
            syscalls_unix: i32,
            context_switches: i32,
            threads: i32,
            numrunning: i32,
            priority: i32,
        }
        const _: () = assert!(size_of::<ProcTaskInfo>() == 96);

        unsafe extern "C" {
            fn proc_pidinfo(
                pid: i32,
                flavor: i32,
                arg: u64,
                buffer: *mut c_void,
                buffersize: i32,
            ) -> i32;
        }

        let mut info = ProcTaskInfo::default();
        let size = i32::try_from(size_of::<ProcTaskInfo>()).ok()?;
        // SAFETY: the buffer is this struct's own size, which is what is passed
        // as `buffersize`; a short write is rejected by the length check below.
        let written = unsafe {
            proc_pidinfo(
                i32::try_from(std::process::id()).ok()?,
                4, // PROC_PIDTASKINFO
                0,
                std::ptr::from_mut(&mut info).cast(),
                size,
            )
        };
        (written == size && info.resident_size > 0).then_some(info.resident_size)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn resident_bytes() -> Option<u64> {
        None
    }

    /// Runs `work` while a sampler thread polls the resident set, and returns
    /// `(baseline, peak, result)`: RSS before the run, the highest sample
    /// during it (never below the baseline), and what `work` returned.
    fn peak_rss_during<T>(work: impl FnOnce() -> T) -> (Option<u64>, Option<u64>, T) {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let baseline = resident_bytes();
        let peak = Arc::new(AtomicU64::new(baseline.unwrap_or(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let sampler = {
            let (peak, stop) = (Arc::clone(&peak), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Some(now) = resident_bytes() {
                        peak.fetch_max(now, Ordering::Relaxed);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };
        let result = work();
        if let Some(now) = resident_bytes() {
            peak.fetch_max(now, Ordering::Relaxed);
        }
        stop.store(true, Ordering::Relaxed);
        sampler.join().expect("the sampler thread ends cleanly");
        let peak = peak.load(Ordering::Relaxed);
        (baseline, (peak > 0).then_some(peak), result)
    }

    /// What one worst-case write and re-open measured.
    struct WorstCase {
        text_bytes: usize,
        serialised: usize,
        /// Bytes of the serialised payload that are escaped message text.
        escaped_text: usize,
        baseline: Option<u64>,
        write_peak: Option<u64>,
        load_peak: Option<u64>,
    }

    /// Encodes, writes and re-opens `cache`, asserting the structural facts a
    /// control-character cache must keep.
    fn write_and_reopen_worst_case(cache: &HistoryCache) -> WorstCase {
        let (_dir, file) = scratch("history.json");
        let text_bytes = cache.encoded_text_bytes();
        assert_eq!(
            cache.peers.values().map(Vec::len).sum::<usize>(),
            HISTORY_CACHE_PEERS * HISTORY_CACHE_DEPTH,
            "the fill is complete: nothing was evicted before the write"
        );

        let tag = HistoryTag {
            account_id: Some(1),
            phone: None,
        };
        let (baseline, write_peak, (serialised, escaped_text)) = peak_rss_during(|| {
            let bytes = HistoryFile::encode(cache, &tag).expect("the cache serialises");
            let len = bytes.len();
            file.write(&bytes);
            // `control_char_cache` text is `\u0001` runs and nothing else in
            // the payload escapes, so this is the text portion exactly.
            let escaped = bytes.windows(6).filter(|w| *w == b"\\u0001").count() * 6;
            (len, escaped)
        });
        assert!(
            serialised > text_bytes,
            "control characters inflate on the wire: {serialised} vs {text_bytes}"
        );
        assert!(file.path.exists(), "the write left a file");

        let (_, load_peak, loaded) = peak_rss_during(|| file.load());
        assert_eq!(loaded.account_id, Some(1));
        assert_eq!(loaded.cache.encoded_text_bytes(), text_bytes);
        assert_eq!(loaded.cache.get(1).len(), HISTORY_CACHE_DEPTH);
        assert_eq!(fields(&loaded.cache.get(1)[0]), fields(&cache.get(1)[0]));
        WorstCase {
            text_bytes,
            serialised,
            escaped_text,
            baseline,
            write_peak,
            load_peak,
        }
    }

    #[test]
    fn cache_worst_case_small_fill_keeps_the_structural_invariants() {
        // A few hundred KiB: 32 x 200 rows of 40 control characters.
        let cache = control_char_cache(40);
        let run = write_and_reopen_worst_case(&cache);
        assert_eq!(
            run.text_bytes,
            HISTORY_CACHE_PEERS * HISTORY_CACHE_DEPTH * 40 * 6
        );
        assert_eq!(run.escaped_text, run.text_bytes, "the count is the wire");
    }

    #[test]
    #[ignore = "release-only: serialises ~48 MB through serde_json, too slow for a debug build; \
                run `cargo test -p televim --release --all-features --bin televim \
                cache_worst_case -- --include-ignored --nocapture`"]
    #[allow(clippy::cast_precision_loss, reason = "display only")]
    fn cache_worst_case_full_budget_write_is_measured() {
        // Largest whole per-row size whose *encoded* total fits the bound:
        // each `\u{1}` is one character and six encoded bytes.
        let per_row = HISTORY_CACHE_BYTES / (HISTORY_CACHE_PEERS * HISTORY_CACHE_DEPTH * 6);
        let cache = control_char_cache(per_row);
        assert!(cache.encoded_text_bytes() > HISTORY_CACHE_BYTES - HISTORY_CACHE_BYTES / 100);

        let run = write_and_reopen_worst_case(&cache);
        // The structural bound: the escaped text in the serialised payload is
        // within the budget, and is exactly what the accounting counted.
        assert!(
            run.escaped_text <= HISTORY_CACHE_BYTES,
            "serialised text portion {} B exceeds the {HISTORY_CACHE_BYTES} B budget",
            run.escaped_text
        );
        assert_eq!(run.escaped_text, run.text_bytes);

        let mb = |bytes: Option<u64>| {
            bytes.map_or("n/a".to_owned(), |b| format!("{:.1} MB", b as f64 / 1e6))
        };
        println!(
            "cache_worst_case: rows             = {}",
            HISTORY_CACHE_PEERS * HISTORY_CACHE_DEPTH
        );
        println!("cache_worst_case: encoded text     = {}", run.text_bytes);
        println!("cache_worst_case: serialised len   = {}", run.serialised);
        println!(
            "cache_worst_case: ratio            = {:.2}x",
            run.serialised as f64 / run.text_bytes as f64
        );
        println!("cache_worst_case: baseline RSS     = {}", mb(run.baseline));
        println!(
            "cache_worst_case: peak RSS (write) = {}",
            mb(run.write_peak)
        );
        println!("cache_worst_case: peak RSS (load)  = {}", mb(run.load_peak));
    }

    // ---- ranges, transactional ingest and the chip side tables (stage 02) --

    fn scratch_store(name: &str) -> (tempfile::TempDir, HistoryFile, Store) {
        let (dir, file) = scratch(name);
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        (dir, file, store)
    }

    /// Sealed rows for [`PEER`] with these ids, oldest id first.
    fn rows(ids: impl IntoIterator<Item = i64>) -> Vec<CachedMessage> {
        ids.into_iter()
            .map(|id| CachedMessage::from_message(&message(id, PEER, "row")))
            .collect()
    }

    /// Plants rows and a range directly, for a gap a fetch recorded by some
    /// other path — the ledger can hold more than one stretch per chat.
    fn plant(store: &Store, ids: impl IntoIterator<Item = i64>, range: (i64, i64)) {
        for row in rows(ids) {
            store.put_record(PEER, &row).expect("the row is planted");
        }
        let chat_key = keyed_chat(&store.hmac_key, PEER);
        store
            .conn
            .execute(
                "INSERT INTO ranges (chat_key, lo_id, hi_id) VALUES (?1, ?2, ?3)",
                params![chat_key, range.0, range.1],
            )
            .expect("the range is planted");
    }

    /// A bounded page read selects only the newest `limit` rows below the
    /// anchor, so a corrupt row far below it is never decoded — while a
    /// whole-chat read still fails on that same row.
    #[test]
    fn a_bounded_page_read_decodes_only_the_rows_below_the_anchor() {
        let (_dir, file, store) = scratch_store("history.json");
        plant(&store, 1..=1_000, (1, 1_000));

        // A row far below the anchor no longer carries a valid sealed envelope.
        let chat_key = keyed_chat(&store.hmac_key, PEER);
        store
            .conn
            .execute(
                "UPDATE messages SET sealed = ?1 WHERE chat_key = ?2 AND message_id = ?3",
                params![vec![0u8; 16], chat_key, 1_i64],
            )
            .expect("the corrupt row is planted");

        let (page, lo) = file.page(PEER, 1_001, 100);
        assert_eq!(lo, Some(1));
        assert_eq!(
            page.iter().map(|m| m.id).collect::<Vec<_>>(),
            (901..=1_000).collect::<Vec<_>>(),
            "exactly the newest 100 rows below the anchor, oldest first"
        );
        assert!(
            file.get(PEER).is_empty(),
            "a whole-chat read still decodes the corrupt row and fails"
        );
    }

    /// A page read picks the one range that reaches the anchor, not an older
    /// range across a gap, and reports nothing when no range reaches it.
    #[test]
    fn a_page_read_uses_only_the_range_that_reaches_the_anchor() {
        let (_dir, file, store) = scratch_store("history.json");
        plant(&store, 1..=100, (1, 100));
        plant(&store, 301..=400, (301, 400));

        let (page, lo) = file.page(PEER, 401, 100);
        assert_eq!(lo, Some(301));
        assert_eq!(
            page.iter().map(|m| m.id).collect::<Vec<_>>(),
            (301..=400).collect::<Vec<_>>(),
            "the 301 range only, never the 1 range across the gap"
        );

        let (page, lo) = file.page(PEER, 501, 100);
        assert!(page.is_empty());
        assert_eq!(lo, None, "no range reaches an anchor above both");
    }

    /// A page read anchored inside an older range does not let a newer
    /// disconnected range hide it: a range that starts at or above the
    /// anchor contributes no row below it, however high its top reaches.
    #[test]
    fn a_newer_range_does_not_hide_an_older_one() {
        let (_dir, file, store) = scratch_store("history.json");
        plant(&store, 1..=100, (1, 100));
        plant(&store, 301..=400, (301, 400));

        let (page, lo) = file.page(PEER, 90, 100);
        assert_eq!(lo, Some(1));
        assert_eq!(
            page.iter().map(|m| m.id).collect::<Vec<_>>(),
            (1..=89).collect::<Vec<_>>(),
            "the rows below the anchor in the older range"
        );
    }

    /// Fetched pages and the bounded window land in one transaction: a window
    /// failure rolls the pages back too, so a crash mid-write cannot leave a
    /// queued page restoring a row the window's deletions had removed.
    #[test]
    fn pages_and_the_window_land_in_one_transaction_or_not_at_all() {
        let (_dir, file, store) = scratch_store("history.json");
        plant(&store, 1..=5, (1, 5));

        // The window's edit path must read row 3's stored envelope, and that
        // envelope no longer decodes.
        let chat_key = keyed_chat(&store.hmac_key, PEER);
        store
            .conn
            .execute(
                "UPDATE messages SET sealed = ?1 WHERE chat_key = ?2 AND message_id = ?3",
                params![vec![0u8; 16], chat_key, 3_i64],
            )
            .expect("the corrupt row is planted");

        let payload = Payload {
            account: None,
            account_id: None,
            peers: BTreeMap::new(),
            chats: Vec::new(),
            aliases: BTreeMap::new(),
            chips: BTreeMap::new(),
            removed: BTreeSet::new(),
            deleted_messages: BTreeSet::new(),
            updated_messages: BTreeMap::from([(PEER, BTreeMap::from([(3, "edited".to_owned())]))]),
        };
        let bytes = serde_json::to_vec(&payload).expect("the payload encodes");

        let pages = vec![(PEER, (40..=45).map(|id| message(id, PEER, "row")).collect())];
        let result = file.ingest_pages_and_write(&pages, &bytes);
        assert!(result.is_err(), "the window phase fails on the corrupt row");

        // The pages rolled back with the window: neither their rows nor their
        // range survived the one failed transaction.
        assert_eq!(file.ranges(PEER), vec![(1, 5)]);
        let conn = rusqlite::Connection::open(&file.path).expect("the archive opens raw");
        let landed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE message_id BETWEEN 40 AND 45",
                [],
                |row| row.get(0),
            )
            .expect("the raw row count reads");
        assert_eq!(landed, 0, "the page did not land when the window failed");
    }

    #[test]
    fn a_page_before_and_after_a_stored_run_merges_into_one_range() {
        let (_dir, _file, mut store) = scratch_store("history.json");
        assert!(
            store
                .ingest_page(PEER, &rows(10..=15), (10, 15))
                .expect("the first page lands")
        );
        assert_eq!(store.ranges(PEER).expect("ranges read"), vec![(10, 15)]);

        // Below the run, anchored at its oldest.
        assert!(
            store
                .ingest_page(PEER, &rows([5, 7, 9]), (5, 10))
                .expect("the older page lands")
        );
        assert_eq!(store.ranges(PEER).expect("ranges read"), vec![(5, 15)]);

        // Above the run, anchored at its newest.
        assert!(
            store
                .ingest_page(PEER, &rows([17, 20]), (15, 20))
                .expect("the newer page lands")
        );
        assert_eq!(store.ranges(PEER).expect("ranges read"), vec![(5, 20)]);
        assert_eq!(
            store.all_records(PEER).expect("rows read").len(),
            11,
            "every row is still held, in one stretch"
        );
    }

    #[test]
    fn ingest_keeps_every_row_past_the_window_depth() {
        let (_dir, _file, mut store) = scratch_store("history.json");
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let ids: Vec<i64> = (1..=depth + 5).collect();

        assert!(
            store
                .ingest_page(PEER, &rows(ids.iter().copied()), (1, depth + 5))
                .expect("the page lands")
        );

        assert_eq!(
            store.all_records(PEER).expect("rows read").len(),
            ids.len(),
            "nothing is trimmed to make room for a page"
        );
        assert_eq!(
            store.ranges(PEER).expect("ranges read"),
            vec![(1, depth + 5)],
            "the range names every row it holds"
        );
    }

    #[test]
    fn a_page_that_leaves_a_gap_is_kept_as_its_own_range() {
        let (_dir, _file, mut store) = scratch_store("history.json");
        assert!(
            store
                .ingest_page(PEER, &rows(10..=15), (10, 15))
                .expect("first page")
        );
        let before_rows = store.all_records(PEER).expect("rows read");

        assert!(
            store
                .ingest_page(PEER, &rows(20..=25), (20, 25))
                .expect("a disjoint page is stored, not refused"),
            "a page whose reach touches no stored range is still kept"
        );
        let mut expected = before_rows;
        expected.extend(rows(20..=25));
        assert_eq!(store.all_records(PEER).expect("rows read"), expected);
        assert_eq!(
            store.ranges(PEER).expect("ranges read"),
            vec![(10, 15), (20, 25)],
            "the gap is recorded as a second stretch"
        );
    }

    #[test]
    fn a_page_that_fills_a_recorded_gap_closes_it() {
        let (_dir, _file, mut store) = scratch_store("history.json");
        plant(&store, 10..=12, (10, 12));
        plant(&store, 18..=20, (18, 20));
        assert_eq!(
            store.ranges(PEER).expect("ranges read"),
            vec![(10, 12), (18, 20)],
            "the hole is recorded, not inferred"
        );

        assert!(
            store
                .ingest_page(PEER, &rows(13..=17), (12, 18))
                .expect("the bridging page lands")
        );
        assert_eq!(store.ranges(PEER).expect("ranges read"), vec![(10, 20)]);
        assert_eq!(
            store
                .all_records(PEER)
                .expect("rows read")
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            (10..=20).collect::<Vec<_>>()
        );
    }

    /// Writes `cache` through the ingest path and checks that the ledger names
    /// exactly the rows that were stored — one honest stretch per peer.
    fn assert_ranges_match_rows(cache: &HistoryCache) {
        let (_dir, file) = scratch("history.json");
        file.save(cache, None);
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let stored = store.all_records(PEER).expect("records read");
        let ranges = store.ranges(PEER).expect("ranges read");
        match (stored.first(), stored.last()) {
            (Some(first), Some(last)) => {
                assert_eq!(ranges, vec![(first.id, last.id)]);
            }
            _ => assert!(ranges.is_empty(), "no rows, no range"),
        }
        assert_eq!(
            file.text_bytes(),
            Some(cache.encoded_text_bytes()),
            "the SQL text count matches the cache's"
        );
    }

    #[test]
    fn every_merge_rule_leaves_a_range_that_matches_the_stored_rows() {
        let mut cache = cached(1..=5);
        assert!(cache.merge(PEER, &page(4..=8), LATEST));
        assert_ranges_match_rows(&cache);

        let mut cache = cached(10..=15);
        assert!(cache.merge(
            PEER,
            &page([3, 5, 7]),
            PageKind::Older {
                before: 10,
                reached_start: false,
            }
        ));
        assert_ranges_match_rows(&cache);

        let mut cache = cached(1..=5);
        assert!(cache.merge(
            PEER,
            &page([8, 9]),
            PageKind::Newer {
                after: 5,
                reached_end: false,
            }
        ));
        assert_ranges_match_rows(&cache);

        let mut cache = cached(1..=6);
        assert!(cache.merge(PEER, &page([3, 5, 6]), LATEST));
        assert_ranges_match_rows(&cache);

        let mut cache = cached(1..=5);
        assert!(cache.merge(PEER, &page(20..=25), LATEST));
        assert_ranges_match_rows(&cache);

        let mut cache = cached(10..=15);
        assert!(cache.merge(PEER, &page([7, 9, 11, 12]), PageKind::Around));
        assert_ranges_match_rows(&cache);
    }

    #[test]
    fn a_chat_past_the_window_survives_every_save_and_a_reopen() {
        let (_dir, file) = scratch("history.json");
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let mut cache = HistoryCache::default();

        let first: Vec<Message> = (1..=depth - 50)
            .map(|id| message(id, PEER, "first"))
            .collect();
        assert!(cache.merge(PEER, &first, LATEST));
        file.save(&cache, None);

        let second: Vec<Message> = (depth - 49..=depth + 50)
            .map(|id| message(id, PEER, "second"))
            .collect();
        assert!(cache.merge(
            PEER,
            &second,
            PageKind::Newer {
                after: depth - 50,
                reached_end: false,
            }
        ));
        file.save(&cache, None);

        let third: Vec<Message> = (depth + 51..=depth + 120)
            .map(|id| message(id, PEER, "third"))
            .collect();
        assert!(cache.merge(
            PEER,
            &third,
            PageKind::Newer {
                after: depth + 50,
                reached_end: false,
            }
        ));
        file.save(&cache, None);

        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let stored = store.all_records(PEER).expect("the rows open");
        assert_eq!(
            stored.len(),
            usize::try_from(depth + 120).expect("the count fits")
        );
        assert_eq!(
            stored.first().map(|row| row.id),
            Some(1),
            "the oldest is kept"
        );
        assert_eq!(stored.last().map(|row| row.id), Some(depth + 120));
        assert_eq!(
            store.ranges(PEER).expect("the ranges open"),
            vec![(1, depth + 120)],
            "the ledger names every row the saves left behind"
        );
        assert_eq!(ids(&file.get(PEER)), (1..=depth + 120).collect::<Vec<_>>());
        assert_eq!(file.load().cache.get(PEER).len(), HISTORY_CACHE_DEPTH);
    }

    #[test]
    fn a_disjoint_window_is_kept_as_its_own_range_on_save() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        assert!(cache.merge(PEER, &page(10..=15), LATEST));
        file.save(&cache, None);
        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let before_rows = store.all_records(PEER).expect("rows read");
        drop(store);

        let mut disjoint = HistoryCache::default();
        assert!(disjoint.merge(PEER, &page(20..=25), LATEST));
        file.save(&disjoint, None);

        let reopened = open_store(&file.path, &file.keys()).expect("it reopens");
        let mut expected = before_rows;
        expected.extend(rows(20..=25));
        assert_eq!(reopened.all_records(PEER).expect("rows read"), expected);
        assert_eq!(
            reopened.ranges(PEER).expect("ranges read"),
            vec![(10, 15), (20, 25)]
        );
    }

    #[test]
    fn a_chat_the_reader_removed_stays_removed_after_a_reopen() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        let elsewhere: Vec<Message> = (1..=3).map(|id| message(id, PEER + 1, "kept")).collect();
        cache.put(PEER, &page(1..=5));
        cache.put(PEER + 1, &elsewhere);
        file.save(&cache, None);

        cache.remove(PEER);
        file.save(&cache, None);

        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        assert!(store.all_records(PEER).expect("rows read").is_empty());
        assert!(store.ranges(PEER).expect("ranges read").is_empty());
        assert_eq!(store.all_records(PEER + 1).expect("rows read").len(), 3);
        let loaded = file.load().cache;
        assert!(loaded.get(PEER).is_empty());
        assert_eq!(loaded.get(PEER + 1).len(), 3);
    }

    /// A deletion inside the window is applied to the stored row and the run is
    /// split, not left claiming the deleted id.
    #[test]
    fn a_deleted_message_in_the_window_stays_gone_after_a_reopen() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(
            PEER,
            &(1..=depth)
                .map(|id| message(id, PEER, "row"))
                .collect::<Vec<_>>(),
        );
        file.save(&cache, None);

        assert!(cache.delete(&[depth / 2]));
        file.save(&cache, None);

        let expected: Vec<i64> = (1..=depth).filter(|id| *id != depth / 2).collect();
        assert_eq!(ids(&file.get(PEER)), expected, "the row stays deleted");
        assert_eq!(
            file.ranges(PEER),
            vec![(1, depth / 2 - 1), (depth / 2 + 1, depth)],
            "the hole splits the run"
        );
        assert_eq!(ids(&file.load().cache.get(PEER)), expected);
    }

    /// A row the window no longer holds can still be deleted: the id travels
    /// with the save and the surviving range is rebuilt from the rows left.
    #[test]
    fn a_deleted_message_past_the_window_stays_gone_after_a_reopen() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let (_dir, file) = scratch("history.json");
        let rows: Vec<CachedMessage> = (1..=depth + 5)
            .map(|id| CachedMessage::from_message(&message(id, PEER, "row")))
            .collect();
        seed(
            &file,
            serde_json::to_vec(&Payload {
                account: None,
                account_id: None,
                peers: BTreeMap::from([(PEER, rows)]),
                chats: Vec::new(),
                aliases: BTreeMap::new(),
                chips: BTreeMap::new(),
                removed: BTreeSet::new(),
                deleted_messages: BTreeSet::new(),
                updated_messages: BTreeMap::new(),
            })
            .expect("the payload serialises"),
        );

        let mut cache = file.load().cache;
        assert_eq!(
            ids(&cache.get(PEER)).first(),
            Some(&6),
            "ids 1 to 5 are past the window"
        );
        assert!(!cache.delete(&[1]), "nothing in the window held it");

        file.save(&cache, None);
        assert_eq!(ids(&file.get(PEER)), (2..=depth + 5).collect::<Vec<_>>());
        assert_eq!(file.ranges(PEER), vec![(2, depth + 5)]);
        assert_eq!(
            ids(&file.load().cache.get(PEER)),
            (6..=depth + 5).collect::<Vec<_>>()
        );
    }

    /// A session can name more deleted ids than one SQL statement accepts.
    #[test]
    fn a_batch_of_deletions_larger_than_one_statement_is_applied_whole() {
        let (_dir, file) = scratch("history.json");
        let rows: Vec<CachedMessage> = (1_i64..=1_200)
            .map(|id| CachedMessage::from_message(&message(id, PEER, "row")))
            .collect();
        seed(
            &file,
            serde_json::to_vec(&Payload {
                account: None,
                account_id: None,
                peers: BTreeMap::from([(PEER, rows)]),
                chats: Vec::new(),
                aliases: BTreeMap::new(),
                chips: BTreeMap::new(),
                removed: BTreeSet::new(),
                deleted_messages: BTreeSet::new(),
                updated_messages: BTreeMap::new(),
            })
            .expect("the payload serialises"),
        );

        let mut cache = file.load().cache;
        let all: Vec<i64> = (1..=1_200).collect();
        assert!(cache.delete(&all));
        file.save(&cache, None);

        assert!(file.get(PEER).is_empty(), "every row was deleted");
        assert!(file.ranges(PEER).is_empty(), "no range is left behind");
    }

    /// An edit of a row in the window is re-sealed on the next save.
    #[test]
    fn an_edited_message_survives_a_reopen() {
        let (_dir, file) = scratch("history.json");
        let mut cache = cached(1..=5);
        file.save(&cache, None);

        assert!(cache.edit(PEER, 3, "edited"));
        file.save(&cache, None);

        let stored = file.get(PEER);
        assert_eq!(stored[2].text, "edited");
        assert_eq!(ids(&stored), (1..=5).collect::<Vec<_>>());
        assert_eq!(file.load().cache.get(PEER)[2].text, "edited");
    }

    /// An edit reported for a row the window no longer holds is re-sealed on
    /// its own stored envelope, with every other field left as it was.
    #[test]
    fn an_edit_past_the_window_replaces_only_the_stored_text() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let (_dir, file) = scratch("history.json");
        let mut rows: Vec<CachedMessage> = (1..=depth + 5)
            .map(|id| {
                CachedMessage::from_message(&message(id, PEER, if id == 1 { "old" } else { "row" }))
            })
            .collect();
        rows[0].timestamp = 1_700_000_000;
        rows[0].is_outgoing = true;
        rows[0].reply_to = Some(3);
        rows[0].media = Some(CachedMedia::Photo);
        rows[0].media_id = Some(91);
        let original = rows[0].clone();
        seed(
            &file,
            serde_json::to_vec(&Payload {
                account: None,
                account_id: None,
                peers: BTreeMap::from([(PEER, rows)]),
                chats: Vec::new(),
                aliases: BTreeMap::new(),
                chips: BTreeMap::new(),
                removed: BTreeSet::new(),
                deleted_messages: BTreeSet::new(),
                updated_messages: BTreeMap::new(),
            })
            .expect("the payload serialises"),
        );

        let mut cache = file.load().cache;
        assert!(!cache.edit(PEER, 1, "changed"), "id 1 is past the window");
        file.save(&cache, None);

        let stored = file.get(PEER);
        assert_eq!(stored[0].text, "changed");
        assert_eq!(stored[0].timestamp, original.timestamp);
        assert_eq!(stored[0].is_outgoing, original.is_outgoing);
        assert_eq!(stored[0].reply_to, original.reply_to);
        assert_eq!(stored[0].media, Some(MediaKind::Photo));
        assert_eq!(stored[0].media_id, original.media_id);
        assert_eq!(
            stored.len(),
            HISTORY_CACHE_DEPTH + 5,
            "every other row survives"
        );
    }

    /// An edit and a deletion of the same id end deleted: deletions run last.
    #[test]
    fn an_edit_and_a_deletion_of_the_same_row_end_deleted() {
        let (_dir, file) = scratch("history.json");
        let mut cache = cached(1..=5);
        file.save(&cache, None);

        assert!(cache.apply_update(&edit(PEER, 3, "edited")));
        assert!(cache.apply_update(&deletion(&[3])));
        file.save(&cache, None);

        assert_eq!(ids(&file.get(PEER)), vec![1, 2, 4, 5]);
        assert_eq!(file.load().cache.get(PEER).len(), 4);
    }

    #[test]
    fn a_damaged_archive_is_not_written_over_until_sign_out() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &page(1..=3));
        file.save(&cache, None);

        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        store
            .conn
            .execute(
                "UPDATE messages SET sealed = x'00' WHERE message_id = 2",
                [],
            )
            .expect("the row is damaged");
        drop(store);

        let loaded = file.load();
        assert!(
            loaded.cache.get(PEER).is_empty(),
            "a damaged archive reads as empty"
        );

        let mut later = HistoryCache::default();
        later.put(PEER, &page(900..=902));
        file.save(&later, None);

        let store = open_store(&file.path, &file.keys()).expect("the archive opens");
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .expect("the rows count");
        assert_eq!(rows, 3, "the refused save left every stored row");
        let written: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE message_id >= 900",
                [],
                |row| row.get(0),
            )
            .expect("the later rows count");
        assert_eq!(written, 0, "the refused save wrote nothing");

        file.clear();
        file.save(&later, None);
        let reopened = open_store(&file.path, &file.keys()).expect("a fresh archive opens");
        assert_eq!(
            reopened
                .all_records(PEER)
                .expect("rows read")
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![900, 901, 902],
            "sign-out lifts the refusal and the next save starts a new archive"
        );
    }

    #[test]
    fn a_missing_archive_never_refuses_a_save() {
        let (_dir, file) = scratch("history.json");
        assert!(file.load().cache.peers.is_empty());

        let mut cache = HistoryCache::default();
        cache.put(PEER, &page(1..=3));
        file.save(&cache, None);

        let loaded = file.load().cache;
        assert_eq!(ids(&loaded.get(PEER)), vec![1, 2, 3]);
    }

    // ---- re-sealing under a new key -------------------------------------

    #[test]
    fn a_reseal_moves_every_stored_row_to_the_new_key() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let total = depth + 50;
        let (dir, file) = scratch("history.json");
        let legacy = dir.path().join("history.json");

        let chips = Chips {
            reactions: Vec::new(),
            pinned: vec![3, 4],
        };
        file.store_payload(&Payload {
            account: Some("+1".to_owned()),
            account_id: Some(7),
            peers: BTreeMap::from([(
                PEER,
                (1..=total)
                    .map(|id| CachedMessage::from_message(&message(id, PEER, "row")))
                    .collect::<Vec<_>>(),
            )]),
            chats: Vec::new(),
            aliases: BTreeMap::from([(PEER, "Kept".to_owned())]),
            chips: BTreeMap::from([(PEER, chips.clone())]),
            removed: BTreeSet::new(),
            deleted_messages: BTreeSet::new(),
            updated_messages: BTreeMap::new(),
        })
        .expect("the archive is written");

        let next: Arc<dyn KeyProvider> =
            Arc::new(telegram_framework::PassphraseProvider::new("new"));
        file.reseal(Arc::clone(&next))
            .expect("the archive is re-sealed");

        let moved = HistoryFile::new(legacy.clone(), Arc::clone(&next));
        assert_eq!(
            ids(&moved.get(PEER)),
            (1..=total).collect::<Vec<_>>(),
            "every row past the window moves to the new key"
        );
        assert_eq!(
            moved.get(PEER).first().map(|row| row.text.to_string()),
            Some("row".to_owned())
        );
        let loaded = moved.load().cache;
        assert_eq!(loaded.get(PEER).len(), HISTORY_CACHE_DEPTH);
        assert_eq!(
            loaded.aliases().get(&PEER).map(String::as_str),
            Some("Kept")
        );
        assert_eq!(
            loaded.pinned.get(&PEER).map(Vec::as_slice),
            Some(&[3, 4][..])
        );
        assert_eq!(
            moved.load().account_id,
            Some(7),
            "the tag crosses with the rows"
        );

        let old = HistoryFile::new(legacy, other_keys());
        assert!(
            old.load().cache.get(PEER).is_empty(),
            "the old key no longer opens it"
        );
    }

    #[test]
    fn a_reseal_that_cannot_seal_leaves_every_stored_row() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &page(1..=5));
        file.save(&cache, None);

        let error = file
            .reseal(Arc::new(Locked))
            .expect_err("a locked key cannot seal");
        assert!(error.contains("could not be re-sealed"), "{error}");

        assert_eq!(ids(&file.get(PEER)), vec![1, 2, 3, 4, 5]);
        assert_eq!(
            file.load().cache.get(PEER).len(),
            5,
            "the archive still reads under the current key"
        );
    }

    /// A passphrase change writes the archive first and the session second. A
    /// kill between the two leaves the session under the old passphrase, so a
    /// restart with that passphrase must still read every record and be able to
    /// save again. The previous envelope in `meta` is what makes that work.
    #[test]
    fn a_restart_after_an_unfinished_passphrase_change_still_reads_every_row() {
        let (dir, file) = scratch("history.json");
        let legacy = dir.path().join("history.json");
        let total = 12i64;
        let rows: Vec<Message> = (1..=total).map(|id| message(id, PEER, "row")).collect();
        let mut cache = HistoryCache::default();
        cache.put(PEER, &rows);
        file.save(&cache, Some("+1"));

        let next: Arc<dyn KeyProvider> =
            Arc::new(telegram_framework::PassphraseProvider::new("new"));
        file.reseal(Arc::clone(&next))
            .expect("the archive moves to the new passphrase");
        // No session save, and no call to `set_provider`: drop the handle the
        // way an interrupted process would.

        let restarted = HistoryFile::new(
            legacy,
            Arc::new(telegram_framework::PassphraseProvider::new("test")),
        );
        let loaded = restarted.load();
        assert_eq!(
            ids(&loaded.cache.get(PEER)),
            (1..=total).collect::<Vec<_>>()
        );
        assert_eq!(loaded.account.as_deref(), Some("+1"));
        let reopened = open_store(&restarted.path, &restarted.keys()).expect("meta was repaired");
        let prev: Option<Vec<u8>> = reopened
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_ARCHIVE_KEY_PREV],
                |row| row.get(0),
            )
            .optional()
            .expect("the meta query reads");
        assert!(
            prev.is_none(),
            "the completed transition left no previous key"
        );

        let mut later_rows = rows;
        later_rows.push(message(total + 1, PEER, "after"));
        let mut later = HistoryCache::default();
        later.put(PEER, &later_rows);
        restarted.save(&later, Some("+1"));
        assert_eq!(
            ids(&restarted.get(PEER)),
            (1..=total + 1).collect::<Vec<_>>(),
            "a save under the old passphrase keeps every row and lands"
        );
    }

    /// An ordinary write under the new passphrase must not drop the previous
    /// key before the session is saved. The old passphrase still recovers every
    /// row; `confirm_rekey` drops the envelope once, when told to.
    #[test]
    fn an_ordinary_write_keeps_the_previous_key_until_rekey_is_confirmed() {
        let (dir, file) = scratch("history.json");
        let total = 12i64;
        let rows: Vec<Message> = (1..=total).map(|id| message(id, PEER, "row")).collect();
        let mut cache = HistoryCache::default();
        cache.put(PEER, &rows);
        file.save(&cache, None);

        let next: Arc<dyn KeyProvider> =
            Arc::new(telegram_framework::PassphraseProvider::new("new"));
        file.reseal(Arc::clone(&next))
            .expect("the archive moves to the new key");

        // The concurrent ordinary save, between the reseal and the session save.
        let mut later_rows = rows.clone();
        later_rows.push(message(total + 1, PEER, "after"));
        let mut later = HistoryCache::default();
        later.put(PEER, &later_rows);
        file.save(&later, None);

        let store = open_store(&file.path, &file.keys()).expect("the new key opens");
        assert!(
            meta_envelope(&store.conn, META_ARCHIVE_KEY_PREV)
                .expect("meta reads")
                .is_some(),
            "the ordinary open left the way back in place"
        );
        drop(store);

        // Recovery on a copy: an old-key open promotes the previous envelope,
        // so the original is left alone for the confirm below.
        let legacy_copy = dir.path().join("copy.history.json");
        let copy_db = archive_path(&legacy_copy);
        for (from, to) in archive_siblings(&file.path)
            .into_iter()
            .zip(archive_siblings(&copy_db))
        {
            if from.is_file() {
                std::fs::copy(&from, &to).expect("the archive is copied");
            }
        }
        let old = HistoryFile::new(
            legacy_copy,
            Arc::new(telegram_framework::PassphraseProvider::new("test")),
        );
        assert_eq!(
            ids(&old.get(PEER)),
            (1..=total + 1).collect::<Vec<_>>(),
            "the old passphrase still recovers every row after the ordinary write"
        );

        file.confirm_rekey()
            .expect("the previous envelope is dropped");
        let store = open_store(&file.path, &file.keys()).expect("the new key still opens");
        assert!(
            meta_envelope(&store.conn, META_ARCHIVE_KEY_PREV)
                .expect("meta reads")
                .is_none()
        );
        assert_eq!(ids(&file.get(PEER)), (1..=total + 1).collect::<Vec<_>>());
    }

    /// A write that commits while a re-seal is running must not be lost, and
    /// every envelope on disk must still be a sealed, address-checked one. The
    /// shared provider mutex serialises the two; this test makes them overlap.
    #[test]
    fn a_reseal_does_not_lose_a_write_that_lands_while_it_runs() {
        let (_dir, file) = scratch("history.json");
        let total = 180i64;
        let start = Arc::new(std::sync::Barrier::new(2));
        let writer = {
            let file = file.clone();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                let mut written = Vec::new();
                let mut cache = HistoryCache::default();
                for id in 1..=total {
                    written.push(message(id, PEER, "written"));
                    cache.put(PEER, &written);
                    file.save(&cache, None);
                }
            })
        };
        start.wait();
        for round in 0..6 {
            let pass = if round % 2 == 0 { "new" } else { "test" };
            file.reseal(Arc::new(telegram_framework::PassphraseProvider::new(pass)))
                .expect("the archive re-seals while writes land");
        }
        writer.join().expect("the writer finished");

        assert_eq!(
            ids(&file.get(PEER)),
            (1..=total).collect::<Vec<_>>(),
            "every write the writer acknowledged is still readable"
        );
        let store = open_store(&file.path, &file.keys()).expect("the final provider opens it");
        store.state().expect("the state blob opens");
        store.every_record().expect("every record opens");
        store.chips().expect("every side table opens");
        let mut statement = store
            .conn
            .prepare("SELECT sealed FROM messages UNION ALL SELECT sealed FROM chips")
            .expect("the sealed columns read");
        let rows = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("the envelopes read");
        for envelope in rows {
            assert!(
                sealed::is_sealed(&envelope.expect("an envelope")),
                "no record or side table is stored as plaintext"
            );
        }
        let state: Vec<u8> = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_STATE],
                |row| row.get(0),
            )
            .expect("the state row reads");
        assert!(sealed::is_sealed(&state), "the state blob is sealed");
    }

    #[test]
    fn a_range_read_returns_the_same_rows_as_a_whole_load() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &page(1..=5));
        file.save(&cache, None);

        let read = file.get(PEER);
        assert_eq!(ids(&read), vec![1, 2, 3, 4, 5]);
        assert_eq!(
            read.iter().map(fields).collect::<Vec<_>>(),
            cache.get(PEER).iter().map(fields).collect::<Vec<_>>(),
            "the range read decodes and unseals the same rows on demand"
        );
    }

    #[test]
    fn chips_and_pins_survive_a_reopen() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &page(1..=5));
        let reaction = ReactionCount {
            reaction: Reaction::Emoticon("👍".to_owned()),
            count: 3,
            chosen: true,
        };
        assert!(cache.apply_update(&UpdateEvent::ReactionsChanged {
            chat_id: PEER,
            message_id: 2,
            reactions: vec![reaction.clone()],
        }));
        assert!(cache.apply_update(&UpdateEvent::PinChanged {
            chat_id: PEER,
            message_ids: vec![3, 4],
            pinned: true,
        }));
        file.save(&cache, None);

        let loaded = file.load().cache;
        let strips = loaded.reactions.get(&PEER).expect("the strips survive");
        assert_eq!(strips[0].0, 2, "the strip is still filed under its message");
        assert_eq!(
            strips[0]
                .1
                .iter()
                .map(ReactionCount::from)
                .collect::<Vec<_>>(),
            vec![reaction]
        );
        assert_eq!(
            loaded.pinned.get(&PEER).map(Vec::as_slice),
            Some([3, 4].as_slice())
        );

        // An unpin is a change too, and the narrower set is what is stored.
        let mut loaded = loaded;
        assert!(loaded.apply_update(&UpdateEvent::PinChanged {
            chat_id: PEER,
            message_ids: vec![3],
            pinned: false,
        }));
        file.save(&loaded, None);
        assert_eq!(
            file.load().cache.pinned.get(&PEER).map(Vec::as_slice),
            Some([4].as_slice())
        );
    }

    #[test]
    fn the_chip_side_tables_respect_their_bounds() {
        let mut cache = HistoryCache::default();
        let count = i64::try_from(REACTION_ENTRIES + 2).expect("the bound fits an id");
        let strip = vec![ReactionCount {
            reaction: Reaction::Paid,
            count: 1,
            chosen: false,
        }];
        for id in 1..=count {
            assert!(cache.apply_update(&UpdateEvent::ReactionsChanged {
                chat_id: PEER,
                message_id: id,
                reactions: strip.clone(),
            }));
        }
        assert_eq!(
            cache.reactions.get(&PEER).map_or(0, Vec::len),
            REACTION_ENTRIES,
            "the oldest strip went, not the newest"
        );
        assert_eq!(cache.reactions.get(&PEER).map(|r| r[0].0), Some(3));

        let count = i64::try_from(PINNED_IDS + 2).expect("the bound fits an id");
        assert!(cache.apply_update(&UpdateEvent::PinChanged {
            chat_id: PEER,
            message_ids: (1..=count).collect(),
            pinned: true,
        }));
        assert_eq!(cache.pinned.get(&PEER).map_or(0, Vec::len), PINNED_IDS);
        assert_eq!(
            cache.pinned.get(&PEER).and_then(|p| p.first()).copied(),
            Some(3)
        );
    }

    #[test]
    fn an_empty_reaction_strip_removes_the_entry() {
        let mut cache = HistoryCache::default();
        let event = |reactions: Vec<ReactionCount>| UpdateEvent::ReactionsChanged {
            chat_id: PEER,
            message_id: 5,
            reactions,
        };
        assert!(cache.apply_update(&event(vec![ReactionCount {
            reaction: Reaction::Paid,
            count: 1,
            chosen: false,
        }])));
        assert!(cache.reactions.contains_key(&PEER));
        assert!(cache.apply_update(&event(Vec::new())));
        assert_eq!(
            cache.reactions.get(&PEER).map_or(0, Vec::len),
            0,
            "an empty answer is the last reaction taken away"
        );
    }
}

#[cfg(test)]
mod alias_persistence_tests {
    use super::*;

    #[test]
    fn local_names_survive_a_save_and_a_load() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = HistoryFile::for_test(dir.path().join("history.json"));
        let mut cache = HistoryCache::default();

        assert!(cache.set_alias(7, Some("Ada")));
        file.save(&cache, None);

        let mut loaded = file.load().cache;
        assert_eq!(loaded.aliases().get(&7).map(String::as_str), Some("Ada"));

        assert!(loaded.set_alias(7, None));
        file.save(&loaded, None);
        assert!(file.load().cache.aliases().is_empty());
    }
}
