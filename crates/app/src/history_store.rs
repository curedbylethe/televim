//! Read messages on disk: the history file beside the configuration.
//!
//! The last [`HISTORY_CACHE_DEPTH`] messages of each private conversation, and
//! the chat list they were read from, so a launch can paint the list and a
//! conversation before the wire has answered. It lives here for the drafts
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
//! **Plaintext, like the drafts.** The file is restricted to its owner and
//! tagged with the account, but not encrypted; sealing it the way the session
//! is sealed is a recorded follow-up, not something this file half-does.
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
use std::sync::Arc;

use anyhow::{Context, Result};
use domain::chat::{Chat, ChatKind};
use domain::history::CONVERSATION_WINDOW;
use domain::message::{MediaKind, Message, MessageStatus};
use domain::updates::UpdateEvent;
use serde::{Deserialize, Serialize};
use telegram_framework::{KeyProvider, sealed};

/// How many messages of one conversation the file keeps.
///
/// Exactly one [`CONVERSATION_WINDOW`]: a cache hit fills the window the
/// conversation opens into with no wire follow-up, and keeping more would be
/// rows the window drops on arrival. Named apart from the window rather than
/// reusing it because the two are different promises — that one is memory,
/// this one is disk — and the assertion below is what keeps them equal until
/// someone decides they should not be.
pub(crate) const HISTORY_CACHE_DEPTH: usize = 200;

const _: () = assert!(HISTORY_CACHE_DEPTH == CONVERSATION_WINDOW);

/// How many conversations the file keeps messages for.
///
/// The depth bounds one conversation and this bounds how many, so the cache
/// has a worst case that can be written down rather than one that grows with
/// every conversation the reader ever opens. Thirty-two is a working set, not
/// an archive: more conversations than a reader moves between in a sitting,
/// and far fewer than an account holds — the chat list still names every one
/// of them, and a conversation past the bound opens the way an uncached one
/// always has, from the wire. At [`HISTORY_CACHE_DEPTH`] rows each it is 6 400
/// rows at most, which at a typical message's size is about a megabyte, on
/// disk and in memory alike.
///
/// The one past the bound that goes is the one least recently *touched* — see
/// [`HistoryCache::touch`] for what that means and why.
pub(crate) const HISTORY_CACHE_PEERS: usize = 32;

/// How many JSON-encoded bytes of message text the file keeps, summed over
/// every cached conversation.
///
/// Rows bound a conversation and conversations bound the file, but neither
/// bounds *size*: a row can carry 4,096 characters, so 6 400 rows could hold
/// tens of megabytes. This is the bound that does, and the one the worst case
/// in `docs/memory.md` is declared from. It counts [`CachedMessage::text`]
/// only — ids, flags and media descriptors are small and fixed per row, and the
/// chat list is capped by [`HISTORY_CACHE_CHATS`] — and past it the least
/// recently touched conversations go whole, as past [`HISTORY_CACHE_PEERS`].
/// The count is of *encoded* bytes ([`encoded_len`]), so the text portion of
/// the serialised file is at most eight MiB whatever the text holds, and lands
/// near ten megabytes on disk once JSON framing is counted.
pub(crate) const HISTORY_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// How many conversations of the chat list the file keeps.
///
/// The list as the wire last showed it is what a launch draws before the wire
/// has answered, so the file holds the head of it: pinned first, then newest,
/// the order it is kept in. Five hundred is more conversations than a screen
/// scrolls through before the fetched list replaces them, and a cap at all is
/// what gives the file a worst case — the fetched list itself has none.
pub(crate) const HISTORY_CACHE_CHATS: usize = 500;

/// The history file beside the configuration.
///
/// Owns its path, which is `config_path.with_extension("history.json")`
/// computed once in `run` and threaded to the loop — passed rather than
/// derived at each use, for the reason [`DraftFile`](crate::draft_store::DraftFile)
/// is: the load, the save and the logout clear cannot disagree about which
/// file they mean.
#[derive(Debug, Clone)]
pub(crate) struct HistoryFile {
    path: PathBuf,

    /// The key source the session file uses, so the cache is sealed under the
    /// same key and no second secret exists.
    keys: Arc<dyn KeyProvider>,
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
/// open. Every peer holds at least one message and at most
/// [`HISTORY_CACHE_DEPTH`], and none of them is a local placeholder; an empty
/// peer is no entry rather than an empty one. At most
/// [`HISTORY_CACHE_PEERS`] peers are held.
///
/// A `BTreeMap` for the drafts file's reason: the same cache always
/// serialises to the same bytes.
#[derive(Debug, Clone, Default)]
pub(crate) struct HistoryCache {
    peers: BTreeMap<i64, Vec<CachedMessage>>,

    /// What this session's update feed has said about the cached runs.
    feed: FeedMarks,

    /// When each peer was last touched, as a count of touches: what picks the
    /// peer to evict past [`HISTORY_CACHE_PEERS`].
    ///
    /// **Memory only**, like the feed marks. A file's peers come back
    /// untouched, and among untouched peers the one whose newest message is
    /// oldest goes first — the nearest thing to recency a file records without
    /// growing a field for it.
    touched: BTreeMap<i64, u64>,

    /// The count the next touch is stamped with.
    clock: u64,

    /// The reader's own names for people, keyed by peer id.
    ///
    /// In the same account-tagged file as the chats, so sign-out takes them
    /// with the history they describe.
    aliases: BTreeMap<i64, String>,

    /// The chat list as the wire last showed it, head first, at most
    /// [`HISTORY_CACHE_CHATS`] long.
    ///
    /// In the history file rather than a file of its own: one account tag
    /// guards both, and one removal on sign-out takes both — a list kept
    /// apart would be a second place for another account's conversations to
    /// survive.
    chats: Vec<CachedChat>,
}

/// Two caches are equal when they would write the same file: the feed marks
/// and the touches belong to the session that made them, are never written,
/// and a cache loaded from disk has neither.
impl PartialEq for HistoryCache {
    fn eq(&self, other: &Self) -> bool {
        self.peers == other.peers && self.chats == other.chats && self.aliases == other.aliases
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
            self.evict();
        }
    }

    /// Forgets everything cached for `peer`.
    pub(crate) fn remove(&mut self, peer: i64) {
        self.peers.remove(&peer);
        self.touched.remove(&peer);
        self.aliases.remove(&peer);
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

    /// Stamps `peer` as the most recently touched, which keeps it out of the
    /// next eviction.
    ///
    /// **Touched means written from the wire**: a page folded into its run —
    /// whether or not it changed a row, because a newest page landing is the
    /// reader opening the conversation — or a message added to it. Those are
    /// the only ways a peer enters the cache, so they are where a peer has to
    /// be stamped before an eviction can run. Seeding a window is not a touch:
    /// a conversation seeded online is followed by its newest page, which is
    /// one; and offline nothing enters the cache, so nothing is evicted.
    fn touch(&mut self, peer: i64) {
        self.clock += 1;
        self.touched.insert(peer, self.clock);
    }

    /// The JSON-encoded bytes of message text held, over every cached peer:
    /// what [`HISTORY_CACHE_BYTES`] bounds, so the text portion of the
    /// serialised file never exceeds it. See [`encoded_len`] for the rule.
    /// Summed on demand — at most 6 400 rows — so there is no counter to drift
    /// from the rows.
    fn encoded_text_bytes(&self) -> usize {
        self.peers
            .values()
            .flatten()
            .map(|row| encoded_len(&row.text))
            .sum()
    }

    /// Drops peers until at most [`HISTORY_CACHE_PEERS`] are left and their
    /// text fits [`HISTORY_CACHE_BYTES`], the least recently touched first.
    ///
    /// Untouched peers — what a file held — go before any touched one, the
    /// one whose newest message is oldest first; the peer id breaks a tie, so
    /// the choice is the same on every run.
    ///
    /// **The feed marks follow.** An evicted peer is no longer current: its
    /// run is gone, and a current mark on nothing would let the next arrival
    /// start a run of one message and call it the conversation's newest end
    /// with everything before it missing. What the feed has *seen* stays, for
    /// [`FeedMarks::seen`]'s reason — the message exists whether or not it is
    /// cached — so a page fetched before it still cannot mark a returning
    /// peer current.
    fn evict(&mut self) {
        while self.peers.len() > HISTORY_CACHE_PEERS
            || self.encoded_text_bytes() > HISTORY_CACHE_BYTES
        {
            let touched = &self.touched;
            let Some(oldest) = self
                .peers
                .iter()
                .min_by_key(|(peer, rows)| {
                    (
                        touched.get(peer).copied().unwrap_or(0),
                        rows.last().map_or(i64::MIN, |row| row.timestamp),
                    )
                })
                .map(|(peer, _)| *peer)
            else {
                return;
            };
            self.remove(oldest);
            self.feed.current.remove(&oldest);
        }
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
        self.touch(peer);
        if unchanged {
            return false;
        }
        self.peers.insert(peer, merged);
        self.evict();
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
            UpdateEvent::ReadReceipt { .. }
            | UpdateEvent::InboxRead { .. }
            | UpdateEvent::PeerTyping { .. }
            | UpdateEvent::PeerStatus { .. } => false,
        }
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
        self.touch(peer);
        self.evict();
        true
    }

    /// Replaces the text of message `id` of `peer`, if it is cached.
    fn edit(&mut self, peer: i64, id: i64, text: &str) -> bool {
        let Some(row) = self
            .peers
            .get_mut(&peer)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == id))
        else {
            return false;
        };
        if row.text == text {
            return false;
        }
        text.clone_into(&mut row.text);
        true
    }

    /// Takes the messages `ids` out of every peer.
    ///
    /// A current run that loses the newest message the feed showed is still
    /// current — the deletion is the conversation's own — so what it now ends
    /// at becomes the newest seen, or a later page ending there would read as
    /// one the feed had overtaken.
    fn delete(&mut self, ids: &[i64]) -> bool {
        let FeedMarks { current, seen } = &mut self.feed;
        let touched = &mut self.touched;
        let mut changed = false;
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
            if rows.is_empty() {
                touched.remove(peer);
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
}

/// One conversation of the cached chat list: what its row draws.
///
/// No kind, because only private conversations are cached and that is the
/// one kind a row is read back as. No presence, because a status read back
/// from a file is a claim about a moment that has passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            CachedMedia::File => Self::File,
        }
    }
}

impl HistoryFile {
    /// A handle on the history file at `path`, sealed under `keys`.
    #[must_use]
    pub(crate) fn new(path: PathBuf, keys: Arc<dyn KeyProvider>) -> Self {
        Self { path, keys }
    }

    /// The same file, sealed under `keys` from here on.
    #[must_use]
    pub(crate) fn rekeyed(self, keys: Arc<dyn KeyProvider>) -> Self {
        Self::new(self.path, keys)
    }

    /// Re-seals the file in place under `to`, when it opens under the current key.
    ///
    /// A file the current key does not open is left alone: it is a cache, and it
    /// reads as empty under either key. Best-effort like [`HistoryFile::write`].
    pub(crate) fn reseal(&self, to: Arc<dyn KeyProvider>) {
        let Ok(bytes) = fs::read(&self.path) else {
            return;
        };
        let plain = match sealed::open(&bytes, self.keys.as_ref()) {
            Ok(Some(plain)) => plain.to_vec(),
            Ok(None) => bytes,
            Err(error) => {
                tracing::warn!(%error, "history file left as it was: it does not open");
                return;
            }
        };
        Self::new(self.path.clone(), to).write(&plain);
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
    /// A missing file is an empty cache, not an error; corrupt JSON and any
    /// other read failure are warned about and empty too. What a readable
    /// file holds goes through [`HistoryCache::put`], so a hand-edited or
    /// older file is held to the same bounds as one this build wrote: no
    /// placeholders, no peer deeper than [`HISTORY_CACHE_DEPTH`], and no more
    /// than [`HISTORY_CACHE_PEERS`] peers — the ones with the newest messages.
    pub(crate) fn load(&self) -> LoadedHistory {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LoadedHistory::empty();
            }
            Err(error) => {
                tracing::warn!(%error, "history file could not be read");
                return LoadedHistory::empty();
            }
        };

        match sealed::open(&bytes, self.keys.as_ref()) {
            Ok(Some(plain)) => Self::parse(&plain).unwrap_or_else(LoadedHistory::empty),
            Ok(None) => {
                // Not an envelope: a file from a build that wrote plaintext.
                // Read it once, then seal the same bytes in its place, wiping
                // the old inode. Garbage is left for the next save to replace.
                let Some(loaded) = Self::parse(&bytes) else {
                    return LoadedHistory::empty();
                };
                self.write(&bytes);
                loaded
            }
            // A wrong key or a damaged envelope: a cache is recoverable, but
            // the right key may be back tomorrow, so the file stays until a
            // save overwrites it.
            Err(error) => {
                tracing::warn!(%error, "history file could not be opened; starting empty");
                LoadedHistory::empty()
            }
        }
    }

    /// The cache `plain` JSON holds, or `None` (warned about) when it is not
    /// a history payload.
    fn parse(plain: &[u8]) -> Option<LoadedHistory> {
        let payload = serde_json::from_slice::<Payload>(plain)
            .inspect_err(|error| tracing::warn!(%error, "discarding an unreadable history file"))
            .ok()?;
        let mut cache = HistoryCache::default();
        for (peer, rows) in payload.peers {
            let messages: Vec<Message> = rows.iter().map(|row| row.to_message(peer)).collect();
            cache.put(peer, &messages);
        }
        cache.chats = payload.chats;
        cache.chats.truncate(HISTORY_CACHE_CHATS);
        cache.aliases = payload.aliases;
        Some(LoadedHistory {
            account: payload.account,
            account_id: payload.account_id,
            cache,
        })
    }

    /// Stores `cache` under the phone `account`, atomically: [`HistoryFile::encode`]
    /// then [`HistoryFile::write`], on one thread.
    ///
    /// The loop does not call this — it encodes where the cache lives and
    /// writes off the loop's thread — so only the tests that pin the file's
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

    /// The bytes that store `cache` under `tag`, or `None` when it could
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
        };
        serde_json::to_vec(&payload)
            .inspect_err(|error| tracing::warn!(%error, "history could not be serialised"))
            .ok()
    }

    /// Seals `bytes` from [`HistoryFile::encode`] and replaces the file with
    /// the envelope, atomically.
    ///
    /// Best-effort: a failure is warned about and the run carries on with the
    /// messages still in memory — the file is a head start for the next
    /// launch, and the wire can always refill it. A provider that cannot open
    /// its own envelope fails here, before any file is touched. A plaintext
    /// file being replaced is zeroed once the envelope is in place.
    pub(crate) fn write(&self, bytes: &[u8]) {
        let envelope = match sealed::seal(bytes, self.keys.as_ref()) {
            Ok(envelope) => envelope,
            Err(error) => {
                tracing::warn!(%error, "history could not be sealed");
                return;
            }
        };
        let reopens = matches!(
            sealed::open(&envelope, self.keys.as_ref()),
            Ok(Some(plain)) if plain.as_slice() == bytes
        );
        if !reopens {
            tracing::warn!("the sealed history did not open again; not written");
            return;
        }
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty())
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, "history directory could not be created");
            return;
        }
        // Opened before the replace, which unlinks the old inode: the handle
        // is what lets the old bytes be zeroed afterwards.
        let legacy = open_unsealed(&self.path);
        if write_atomically(&self.path, &envelope)
            && let Some(mut old) = legacy
        {
            if let Err(error) = wipe(&mut old) {
                tracing::warn!(%error, "could not wipe the pre-migration history file");
            }
            remove_stale_temps(&self.path);
        }
    }

    /// Removes the file; a missing file is success.
    ///
    /// Logout calls this, and a failed delete must not fail the logout, so
    /// the failure is a warning and the return is nothing.
    pub(crate) fn clear(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "history file could not be removed"),
        }
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

/// Writes `bytes` so that the target only ever appears as a whole file.
///
/// The drafts file's `write_atomically`, copied with its reasoning: a sibling
/// temp in the same directory, restricted before a byte is in it, renamed over
/// the target — so an interrupted write leaves the previous history intact
/// rather than a truncated file, and the target inherits the temp's `0600`.
fn write_atomically(path: &Path, bytes: &[u8]) -> bool {
    let temp = temp_sibling(path);

    let result = (|| -> Result<()> {
        fs::File::create(&temp).context("creating the history temp file")?;
        restrict_permissions(&temp);
        fs::write(&temp, bytes).context("writing the history temp file")?;
        fs::rename(&temp, path).context("replacing the history file")?;
        Ok(())
    })();

    if let Err(error) = &result {
        // A half-written temp is not a history file, and nothing would ever
        // clean it up.
        let _ = fs::remove_file(&temp);
        tracing::warn!(%error, "history file could not be written");
    }
    result.is_ok()
}

/// The existing file at `path`, opened for writing, if it is not an envelope.
///
/// A legacy plaintext file (or junk about to be replaced); the handle is what
/// lets [`wipe`] reach it after the path has been replaced. Unix only:
/// elsewhere a rename over an open file is not reliable.
#[cfg(unix)]
fn open_unsealed(path: &Path) -> Option<fs::File> {
    use std::io::Read as _;

    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .ok()?;
    let mut head = Vec::with_capacity(sealed::MAGIC.len());
    file.by_ref()
        .take(sealed::MAGIC.len() as u64)
        .read_to_end(&mut head)
        .ok()?;
    (!sealed::is_sealed(&head)).then_some(file)
}

#[cfg(not(unix))]
fn open_unsealed(_path: &Path) -> Option<fs::File> {
    None
}

/// Overwrites `file` with zeros, in place, and syncs it. Best effort.
fn wipe(file: &mut fs::File) -> io::Result<()> {
    use std::io::{Read as _, Seek as _};

    let len = file.metadata()?.len();
    file.rewind()?;
    io::copy(&mut io::repeat(0).take(len), file)?;
    file.sync_all()
}

/// Wipes and removes the `<name>.<pid>.tmp` files [`write_atomically`] leaves
/// when a process dies mid-write; a build that wrote plaintext may have left
/// one holding it.
fn remove_stale_temps(path: &Path) {
    let Some(name) = path.file_name() else {
        return;
    };
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut prefix = name.to_os_string();
    prefix.push(".");
    let prefix = prefix.to_string_lossy().into_owned();

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if is_file
            && file_name
                .strip_prefix(&prefix)
                .is_some_and(|rest| rest.strip_suffix(".tmp").is_some())
        {
            if let Ok(mut file) = fs::OpenOptions::new().write(true).open(entry.path()) {
                let _ = wipe(&mut file);
            }
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The temp file [`write_atomically`] writes through, tagged with the process
/// id so two processes sharing a history path do not write the same temp.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Restricts a freshly written history file to its owner.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict the permissions of the history file");
    }
}

/// No-op on platforms whose ACLs `std` cannot express portably.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: i64 = 7;

    /// Writes `plain` to the file as a sealed envelope, the way a save would,
    /// for fixtures a test hand-writes.
    fn seed(file: &HistoryFile, plain: impl AsRef<[u8]>) {
        let envelope = sealed::seal(plain.as_ref(), file.keys.as_ref()).expect("the fixture seals");
        fs::write(&file.path, envelope).expect("the fixture is written");
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

        let on_disk = fs::read(&file.path).expect("the file the save wrote");
        assert!(
            sealed::is_sealed(&on_disk),
            "the bytes on disk are an envelope"
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

    #[test]
    fn a_missing_file_is_an_empty_cache() {
        let (_dir, file) = scratch("history.json");

        assert_eq!(file.load(), LoadedHistory::empty());
    }

    #[test]
    fn garbage_that_is_neither_envelope_nor_json_is_forgotten_and_the_next_save_overwrites_it() {
        let (_dir, file) = scratch("history.json");
        fs::write(&file.path, "{ not json").expect("the corrupt file");

        assert_eq!(
            file.load(),
            LoadedHistory::empty(),
            "corrupt JSON loads as nothing, warned about, launch carries on"
        );

        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "one")]);
        file.save(&cache, None);
        assert_eq!(
            ids(&file.load().cache.get(PEER)),
            vec![1],
            "the next save overwrites the corrupt file in place"
        );
    }

    const LEGACY: &str = r#"{"account": "+1555", "peers": {"7": [
        {"id": 4, "text": "legacy secret", "timestamp": 40, "is_outgoing": true}
    ]}}"#;

    #[test]
    fn a_legacy_plaintext_file_is_rewritten_sealed() {
        let (_dir, file) = scratch("history.json");
        fs::write(&file.path, LEGACY).expect("the legacy file");

        let loaded = file.load();
        assert_eq!(ids(&loaded.cache.get(PEER)), vec![4], "read once as it was");
        assert_eq!(loaded.account.as_deref(), Some("+1555"));

        let on_disk = fs::read(&file.path).expect("the file is still there");
        assert!(sealed::is_sealed(&on_disk), "rewritten as an envelope");
        assert!(
            !String::from_utf8_lossy(&on_disk).contains("legacy secret"),
            "the plaintext is gone from the file"
        );
        assert_eq!(file.load(), loaded, "and the sealed file loads the same");
    }

    /// The old inode is zeroed, not merely unlinked.
    #[cfg(unix)]
    #[test]
    fn migration_wipes_the_pre_migration_bytes() {
        use std::io::Read as _;

        let (dir, file) = scratch("history.json");
        fs::write(&file.path, LEGACY).expect("the legacy file");
        let stale = dir.path().join("history.json.99999.tmp");
        fs::write(&stale, LEGACY).expect("a stale temp");
        let unrelated = dir.path().join("other.json.1.tmp");
        fs::write(&unrelated, b"x").expect("an unrelated file");
        let mut old = fs::File::open(&file.path).expect("a handle on the old inode");

        file.load();

        let mut left = Vec::new();
        old.read_to_end(&mut left).expect("the old inode reads");
        assert_eq!(left.len(), LEGACY.len(), "the old file changed length");
        assert!(
            left.iter().all(|byte| *byte == 0),
            "plaintext was left behind"
        );
        assert!(!stale.exists(), "the stale temp was left");
        assert!(unrelated.exists(), "a file of another name was touched");
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

    #[test]
    fn a_save_leaves_a_whole_file_and_no_temp_behind() {
        let (_dir, file) = scratch("history.json");
        let mut photo = message(1, PEER, "");
        photo.media = Some(MediaKind::Photo);
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[photo]);

        file.save(&cache, Some("+15551234567"));

        let raw = fs::read(&file.path).expect("the file the save wrote");
        assert!(sealed::is_sealed(&raw), "the target is ciphertext");
        let plain = sealed::open(&raw, file.keys.as_ref())
            .expect("the envelope opens")
            .expect("the target is only ever the finished file");
        let parsed: serde_json::Value =
            serde_json::from_slice(&plain).expect("the sealed payload is JSON");
        let expected: serde_json::Value = serde_json::from_str(
            r#"{"account": "+15551234567", "peers": {"7": [
                {"id": 1, "text": "", "timestamp": 10, "is_outgoing": false, "media": "photo"}
            ]}}"#,
        )
        .expect("the expected payload is JSON");
        assert_eq!(parsed, expected, "the payload shape, media as a kind only");
        assert!(
            !temp_sibling(&file.path).exists(),
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

    // ---- the peer bound --------------------------------------------------

    fn peer_count() -> i64 {
        i64::try_from(HISTORY_CACHE_PEERS).expect("the bound fits an id")
    }

    /// A latest page of one message for `peer`, with an id of its own.
    fn land(cache: &mut HistoryCache, peer: i64) -> bool {
        cache.merge(peer, &[message(peer * 1000, peer, "row")], LATEST)
    }

    #[test]
    fn the_peer_bound_evicts_the_least_recently_touched() {
        let mut cache = HistoryCache::default();
        for peer in 1..=peer_count() {
            assert!(land(&mut cache, peer));
        }
        assert!(
            !land(&mut cache, 1),
            "the same page again changes no row, and still touches the peer"
        );

        assert!(land(&mut cache, peer_count() + 1));

        assert_eq!(cache.peers.len(), HISTORY_CACHE_PEERS);
        assert!(!cache.get(1).is_empty(), "touched last but one, so kept");
        assert!(
            cache.get(2).is_empty(),
            "the least recently touched is the one that went"
        );
        assert!(!cache.get(peer_count() + 1).is_empty());
        assert!(
            !cache.touched.contains_key(&2),
            "and nothing is kept about it"
        );
    }

    #[test]
    fn an_arrival_touches_its_peer() {
        let mut cache = HistoryCache::default();
        for peer in 1..=peer_count() {
            assert!(land(&mut cache, peer));
        }
        assert!(cache.arrive(&message(1001, 1, "new")), "1 is current");

        assert!(land(&mut cache, peer_count() + 1));

        assert!(!cache.get(1).is_empty(), "the arrival kept 1");
        assert!(cache.get(2).is_empty());
    }

    /// An evicted peer has no run, so it is not current — the next arrival
    /// does not start a run of one — while what the feed saw of it stays, so
    /// a page fetched before that message cannot mark it current either.
    #[test]
    fn eviction_keeps_the_feed_marks_consistent() {
        let mut cache = HistoryCache::default();
        assert!(cache.merge(PEER, &page(1..=3), LATEST));
        assert!(cache.apply_update(&arrival(4)));
        for peer in 100..100 + peer_count() {
            assert!(land(&mut cache, peer));
        }
        assert!(cache.get(PEER).is_empty(), "evicted");
        assert!(!cache.feed.current.contains(&PEER));
        assert_eq!(cache.feed.seen.get(&PEER), Some(&4));

        assert!(
            !cache.apply_update(&arrival(5)),
            "no run, so nothing to join and nothing started"
        );
        assert!(cache.get(PEER).is_empty());

        assert!(cache.merge(PEER, &page(1..=3), LATEST), "it comes back");
        assert!(
            !cache.apply_update(&arrival(6)),
            "a page short of 5 is not current"
        );
        assert_eq!(ids(&cache.get(PEER)), vec![1, 2, 3]);
    }

    /// A file holding more peers than the bound — written before the bound,
    /// or by hand — keeps those whose newest message is newest.
    #[test]
    fn a_file_past_the_peer_bound_keeps_the_newest_conversations() {
        let (_dir, file) = scratch("history.json");
        let peers: BTreeMap<i64, Vec<CachedMessage>> = (1..=peer_count() + 2)
            .map(|peer| {
                // Peer 1's message is the newest of all; 2 and 3 the oldest.
                let at = if peer == 1 { 10_000 } else { peer };
                let mut row = CachedMessage::from_message(&message(peer, peer, "row"));
                row.timestamp = at;
                (peer, vec![row])
            })
            .collect();
        let payload = Payload {
            account: None,
            account_id: None,
            peers,
            chats: Vec::new(),
            aliases: BTreeMap::new(),
        };
        seed(
            &file,
            serde_json::to_vec(&payload).expect("the payload serialises"),
        );

        let cache = file.load().cache;
        assert_eq!(cache.peers.len(), HISTORY_CACHE_PEERS);
        assert!(!cache.get(1).is_empty());
        assert!(cache.get(2).is_empty() && cache.get(3).is_empty());
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
        };
        seed(
            &file,
            serde_json::to_vec(&payload).expect("the payload serialises"),
        );

        let kept = ids(&file.load().cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(kept.first(), Some(&2));
    }

    /// A message of `len` bytes of text for `peer`.
    fn big(id: i64, peer: i64, len: usize) -> Message {
        message(id, peer, &"x".repeat(len))
    }

    #[test]
    fn the_byte_budget_evicts_the_least_recently_touched() {
        let third = HISTORY_CACHE_BYTES / 3 + 1;
        let mut cache = HistoryCache::default();
        for peer in 1..=3 {
            assert!(cache.merge(peer, &[big(1, peer, third)], LATEST));
        }
        assert!(cache.encoded_text_bytes() <= HISTORY_CACHE_BYTES);
        assert!(cache.get(1).is_empty(), "the least recently touched went");
        assert!(!cache.get(2).is_empty() && !cache.get(3).is_empty());
        assert!(!cache.touched.contains_key(&1));
        assert!(!cache.feed.current.contains(&1));
    }

    /// One message larger than the whole budget cannot be kept at all.
    #[test]
    fn a_five_megabyte_message_does_not_leave_the_cache_over_budget() {
        let five = 5 * 1024 * 1024;
        let mut cache = HistoryCache::default();
        assert!(cache.merge(1, &[big(1, 1, five)], LATEST));
        assert!(!cache.get(1).is_empty(), "5 MiB alone fits 8 MiB");
        assert!(cache.merge(2, &[big(1, 2, five)], LATEST));
        assert!(cache.encoded_text_bytes() <= HISTORY_CACHE_BYTES);
        assert!(cache.get(1).is_empty(), "the older giant is gone");
        assert!(!cache.get(2).is_empty());

        assert!(cache.merge(3, &[big(1, 3, HISTORY_CACHE_BYTES + 1)], LATEST));
        assert!(cache.encoded_text_bytes() <= HISTORY_CACHE_BYTES);
        assert!(
            cache.get(3).is_empty(),
            "over budget on its own, it goes too"
        );
    }

    #[test]
    fn a_file_past_the_byte_budget_is_cut_on_load() {
        let (_dir, file) = scratch("history.json");
        let third = HISTORY_CACHE_BYTES / 3 + 1;
        let peers: BTreeMap<i64, Vec<CachedMessage>> = (1..=3)
            .map(|peer| {
                let mut row = CachedMessage::from_message(&big(1, peer, third));
                row.timestamp = peer;
                (peer, vec![row])
            })
            .collect();
        let payload = Payload {
            account: None,
            account_id: None,
            peers,
            chats: Vec::new(),
            aliases: BTreeMap::new(),
        };
        seed(
            &file,
            serde_json::to_vec(&payload).expect("the payload serialises"),
        );

        let cache = file.load().cache;
        assert!(cache.encoded_text_bytes() <= HISTORY_CACHE_BYTES);
        assert!(cache.get(1).is_empty(), "the oldest conversation went");
        assert!(!cache.get(2).is_empty() && !cache.get(3).is_empty());
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

    #[test]
    fn the_budget_counts_encoded_bytes_not_raw_length() {
        // Two rows of about half the budget encoded, a twelfth raw: together
        // they fit by `len` many times over, so only the encoded count evicts.
        let mut cache = HistoryCache::default();
        let n = HISTORY_CACHE_BYTES / 6 / 2 + 1;
        for peer in 1..=2 {
            assert!(cache.merge(peer, &[message(1, peer, &"\u{1}".repeat(n))], LATEST));
        }
        assert!(cache.encoded_text_bytes() <= HISTORY_CACHE_BYTES);
        assert!(cache.get(1).is_empty(), "the least recently touched went");
        assert!(!cache.get(2).is_empty());
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
