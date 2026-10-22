//! Searching every conversation at once.
//!
//! `?` asks the server for matches across all peers, so this module is
//! server-authoritative: there is no local pass, because the history cache
//! holds at most a few peers' windows and a scan over it would look like an
//! answer while missing most of the archive. The state therefore moves from
//! in-flight to either a server answer, a pause, or a failure.
//!
//! # Grouping
//!
//! Hits arrive newest first, as the server sends global matches (unlike
//! [`crate::search`], which keeps oldest first). Grouping keeps that order and
//! splits it into contiguous runs of one chat: a chat that the
//! server interleaves with another appears as two groups, not one. The status
//! line counts chats, not groups, so it still says how many conversations the
//! hits span. There is deliberately no per-chat cap.
//!
//! # Paging
//!
//! A page is a round-trip bound, not a memory one: the state keeps every page it
//! is given, [`GlobalSearchState::has_more`] says whether the server holds more,
//! and [`GlobalSearchState::needs_more`] says whether another may be asked for.
//! Nothing is truncated, and the reader stops paging when they stop asking.
//!
//! # Coverage and pauses
//!
//! The loop keeps only the chats the list holds, which are people
//! ([`crate::chat::Chat::is_private`]), and hands this state the count of hits it
//! dropped, which the label reports. A flood wait becomes a pause that keeps the
//! rows already fetched.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::message::MediaKind;

/// What a global search is narrowed to, by attachment kind.
///
/// [`MediaFilter::None`] is no narrowing at all; the other variants are the
/// words the parser accepts and the label echoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaFilter {
    #[default]
    None,
    PhotoVideo,
    Document,
    Music,
    Voice,
    Url,
}

impl MediaFilter {
    /// The reader's word for this filter, or `None` when nothing is narrowed.
    #[must_use]
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::PhotoVideo => Some("photo_video"),
            Self::Document => Some("document"),
            Self::Music => Some("music"),
            Self::Voice => Some("voice"),
            Self::Url => Some("url"),
        }
    }
}

/// The search the reader asked for: the words sent, what they typed, and the
/// narrowing that rides with them.
///
/// Equality is what makes a stale answer detectably stale, so every field takes
/// part in it. `text` is the query the server sees, with tokens removed; `echo`
/// is the same words, because the status line restates each filter as a clause
/// rather than quoting the token back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSpec {
    /// The words sent to the server, filter tokens removed.
    pub text: String,

    /// The words the status line echoes; equal to `text` whenever a parser
    /// supplied the spec, so a filter is never printed twice.
    pub echo: String,

    /// The attachment kind to keep, if any.
    pub media: MediaFilter,

    /// The earliest date to keep, as unix seconds, inclusive.
    pub since: Option<i64>,

    /// The latest date to keep, as unix seconds, inclusive.
    pub until: Option<i64>,
}

impl SearchSpec {
    /// The tokenless spec: `text` and `echo` are the trimmed input, no narrowing.
    ///
    /// It keeps the tree green until the parser lands; it is not a second parser.
    #[must_use]
    pub fn plain(text: &str) -> Self {
        let text = text.trim();
        Self {
            text: text.to_owned(),
            echo: text.to_owned(),
            media: MediaFilter::None,
            since: None,
            until: None,
        }
    }
}

/// One message the server matched, with what is needed to show it.
///
/// Global hits live in no open window, so an identifier alone cannot be
/// rendered: the text travels with the hit. `text` is the caption or message
/// body; `media` names the attachment when there is one. Neither is a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalHit {
    /// The chat the message belongs to.
    pub chat_id: i64,

    /// The message's identifier within that chat.
    pub message_id: i64,

    /// The message's text, or its caption; empty for a bare attachment.
    pub text: String,

    /// The attachment the message carries, if it carries one.
    pub media: Option<MediaKind>,

    /// When the message was sent, as unix seconds; `None` when Telegram sent no date.
    pub sent_at: Option<i64>,

    /// Whether the signed-in account sent the message.
    pub outgoing: bool,
}

impl GlobalHit {
    /// The body to show: the text, or the placeholder for the attachment when
    /// there is no text.
    ///
    /// The same rule as [`crate::message::Message::display_body`], so a hit
    /// reads the way the message reads in its own conversation.
    #[must_use]
    pub fn display_body(&self) -> &str {
        if self.text.is_empty() {
            self.media.map_or("", MediaKind::label)
        } else {
            &self.text
        }
    }
}

/// A run of contiguous hits that share a chat, borrowed from the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGroup<'a> {
    /// The chat every hit in the run belongs to.
    pub chat_id: i64,

    /// The position of the run's first hit in the flattened list, so a caller
    /// can map the selection onto the groups.
    pub first_index: usize,

    /// The hits of the run, newest first, in server order.
    pub hits: &'a [GlobalHit],
}

/// The results of the last global search, and where the reader is among them.
///
/// Pure state: no client, and its only clock is the [`Instant`] a pause counts
/// down to. The state owns the spec, so an answer to a spec the reader has since
/// replaced is refused here rather than by every caller.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalSearchState {
    /// The spec being searched for, or `None` when there has been no search.
    spec: Option<SearchSpec>,

    /// Every page accumulated so far, newest first, nothing truncated.
    hits: Vec<GlobalHit>,

    /// The selected hit, an index into `hits`, or zero on an empty list.
    selected: usize,

    /// How many matches the server holds in all, which may exceed `hits`.
    total: usize,

    /// How many hits the loop dropped for being outside the chat list.
    dropped: usize,

    /// How many distinct chats the held hits span; the status line's count.
    chats: usize,

    /// Whether the server said another page exists.
    has_more: bool,

    /// Whether an answer is still on its way.
    in_flight: bool,

    /// When a flood-wait pause ends, if the search is paused.
    paused_until: Option<Instant>,

    /// Why the search failed, if it did.
    failure: Option<String>,
}

impl GlobalSearchState {
    /// Starts a search, or asks for another page of the current one.
    ///
    /// A new spec clears the old hits and marks the answer as pending. The same
    /// spec is a continuation: the rows already fetched stay, the selection
    /// stays, and the answer is awaited. An empty `text` is no search at all, so
    /// it clears the state rather than leaving an active search with nothing to
    /// ask for.
    pub fn begin(&mut self, spec: &SearchSpec) {
        if spec.text.is_empty() {
            self.clear();
            return;
        }

        if self.spec.as_ref() != Some(spec) {
            self.hits.clear();
            self.selected = 0;
            self.total = 0;
            self.dropped = 0;
            self.chats = 0;
            self.has_more = false;
        }

        self.spec = Some(spec.clone());
        self.in_flight = true;
        self.paused_until = None;
        self.failure = None;
    }

    /// Replaces the hits with the server's first answer to `spec`, if still wanted.
    ///
    /// Returns `false` when `spec` is not the one being searched for, or when no
    /// answer is awaited, so an answer to a replaced search cannot overwrite the
    /// current one and a page that outlived its sequence cannot land. Nothing is
    /// truncated; `has_more` is the server's word on whether another page
    /// exists, and the total is never less than what is held.
    pub fn adopt(
        &mut self,
        spec: &SearchSpec,
        hits: Vec<GlobalHit>,
        total: usize,
        dropped: usize,
        has_more: bool,
    ) -> bool {
        if !self.awaits(spec) {
            return false;
        }

        self.hits = hits;
        self.total = total.max(self.hits.len());
        self.dropped = dropped;
        self.chats = self.distinct_chats();
        self.has_more = has_more;
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        self.in_flight = false;
        self.paused_until = None;
        self.failure = None;

        true
    }

    /// Adds the server's next page for `spec`, if still wanted.
    ///
    /// Returns `false` when `spec` is stale or no page is awaited, as
    /// [`GlobalSearchState::adopt`] does. The rows already held and the selection
    /// survive; `dropped` is this page's count and is added to the running one.
    pub fn append(
        &mut self,
        spec: &SearchSpec,
        hits: Vec<GlobalHit>,
        total: usize,
        dropped: usize,
        has_more: bool,
    ) -> bool {
        if !self.awaits(spec) {
            return false;
        }

        self.hits.extend(hits);
        self.total = total.max(self.hits.len());
        self.dropped += dropped;
        self.chats = self.distinct_chats();
        self.has_more = has_more;
        self.in_flight = false;
        self.paused_until = None;
        self.failure = None;

        true
    }

    /// Records that the server asked the search to wait, keeping the rows.
    ///
    /// Returns `false` when `spec` is stale. The wait starts now and is the
    /// server's own figure; nothing is cleared, so a reader who waits out the
    /// pause still has what was already fetched.
    pub fn pause(&mut self, spec: &SearchSpec, retry_after: Duration) -> bool {
        if !self.is_for(spec) {
            return false;
        }

        self.paused_until = Some(Instant::now() + retry_after);
        self.in_flight = false;
        self.failure = None;

        true
    }

    /// Marks the search as no longer in flight, keeping every row.
    ///
    /// What a continuation that failed leaves behind: the page already shown
    /// stays, and the end of the list simply asks again.
    pub fn settle(&mut self, spec: &SearchSpec) -> bool {
        if !self.is_for(spec) {
            return false;
        }

        self.in_flight = false;
        self.paused_until = None;

        true
    }

    /// Records that the search for `spec` failed.
    ///
    /// Returns `false` when `spec` is stale, as [`GlobalSearchState::adopt`]
    /// does. A first page that failed left nothing to show, so the hits are
    /// cleared with it; a continuation calls [`GlobalSearchState::settle`]
    /// instead.
    pub fn fail(&mut self, spec: &SearchSpec, reason: impl Into<String>) -> bool {
        if !self.is_for(spec) {
            return false;
        }

        self.hits.clear();
        self.total = 0;
        self.dropped = 0;
        self.chats = 0;
        self.has_more = false;
        self.selected = 0;
        self.in_flight = false;
        self.paused_until = None;
        self.failure = Some(reason.into());

        true
    }

    /// Forgets the search, which is what closing the overlay does.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether a search has been started and not cleared.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.spec.is_some()
    }

    /// The spec being searched for, if there has been one.
    #[must_use]
    pub fn spec(&self) -> Option<&SearchSpec> {
        self.spec.as_ref()
    }

    /// Whether an answer is still on its way.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// The matches held so far, newest first, in server order.
    #[must_use]
    pub fn hits(&self) -> &[GlobalHit] {
        &self.hits
    }

    /// How many matches are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Whether the search has shown nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// How many matches the server holds in all.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    /// How many hits the loop dropped for being outside the chat list.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Whether the server said another page exists.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// Whether another page may be asked for: the server has one, none is on its
    /// way, and the search is not paused.
    #[must_use]
    pub fn needs_more(&self) -> bool {
        self.has_more && !self.in_flight && !self.is_paused()
    }

    /// When the pause ends, if the search was paused.
    #[must_use]
    pub fn paused_until(&self) -> Option<Instant> {
        self.paused_until
    }

    /// Whether the search is paused and the wait has not run out.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Whether a later page is on its way, so the list already has rows to keep.
    #[must_use]
    pub fn is_paging(&self) -> bool {
        self.in_flight && !self.hits.is_empty()
    }

    /// Where the selection stands among the held hits.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The hit the reader is on, if the list has one.
    #[must_use]
    pub fn selected_hit(&self) -> Option<&GlobalHit> {
        self.hits.get(self.selected)
    }

    /// Moves the selection `delta` hits, wrapping at both ends.
    ///
    /// The list is shown newest first, so `+1` moves toward older hits (down the
    /// list) and `-1` toward newer ones (up the list).
    ///
    /// Wrapping rather than clamping, as in the user search: a reader who has
    /// run off one end is reaching for the other. A move on an empty list does
    /// nothing.
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.hits.len();
        if len == 0 {
            return;
        }

        let steps = delta.unsigned_abs() % len;
        self.selected = if delta >= 0 {
            (self.selected + steps) % len
        } else {
            (self.selected + len - steps) % len
        };
    }

    /// The held hits split into runs of one chat, newest first, in server order.
    #[must_use]
    pub fn groups(&self) -> Vec<ChatGroup<'_>> {
        let mut groups = Vec::new();
        let mut start = 0;

        for end in 1..=self.hits.len() {
            if end == self.hits.len() || self.hits[end].chat_id != self.hits[start].chat_id {
                groups.push(ChatGroup {
                    chat_id: self.hits[start].chat_id,
                    first_index: start,
                    hits: &self.hits[start..end],
                });
                start = end;
            }
        }

        groups
    }

    /// What the status line says about the search, as of now.
    ///
    /// One function owns every wording: `?{echo} — …`, the query echoed with the
    /// `?` that asked for it, as the conversation's search echoes `/`. `offset`
    /// is the reader's UTC offset in seconds: a date filter is named as the day
    /// the reader typed, which is their zone's, not UTC's.
    #[must_use]
    pub fn label(&self, offset: i64) -> String {
        self.label_at(Instant::now(), offset)
    }

    /// [`GlobalSearchState::label`] at a stated instant, so a countdown can be
    /// tested without waiting for it.
    #[must_use]
    pub fn label_at(&self, now: Instant, offset: i64) -> String {
        let Some(spec) = &self.spec else {
            return String::new();
        };

        let description = if self.in_flight {
            if self.hits.is_empty() {
                "searching…".to_owned()
            } else {
                format!("{}, loading more…", self.count_clause(spec, offset))
            }
        } else if let Some(reason) = &self.failure {
            format!("search failed ({reason})")
        } else if let Some(pause) = self.pause_clause(now) {
            if self.hits.is_empty() {
                pause
            } else {
                format!("{}, {pause}", self.count_clause(spec, offset))
            }
        } else {
            self.count_clause(spec, offset)
        };

        format!("?{} — {description}", spec.echo)
    }

    /// The results clause: a count, the chats it spans or the filters that
    /// narrowed it, and how many hits were dropped.
    fn count_clause(&self, spec: &SearchSpec, offset: i64) -> String {
        let mut clause = if self.hits.is_empty() {
            "no matches".to_owned()
        } else {
            let held = self.hits.len();
            let noun = if held == 1 { "result" } else { "results" };

            // A filtered count is pooled, so naming chats would overstate it.
            if let Some(filters) = Self::filter_clause(spec, offset) {
                format!("{filters} — {held} {noun}")
            } else {
                let chats = self.chats;
                let chat_noun = if chats == 1 { "chat" } else { "chats" };
                format!("{held} {noun} in {chats} {chat_noun}")
            }
        };

        if self.dropped > 0 {
            use std::fmt::Write as _;
            let _ = write!(clause, ", {} dropped", self.dropped);
        }

        clause
    }

    /// The reader's filters in reading order, or `None` when nothing is narrowed.
    fn filter_clause(spec: &SearchSpec, offset: i64) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();

        if let Some(media) = spec.media.label() {
            parts.push(media.to_owned());
        }

        match (spec.since, spec.until) {
            (Some(since), Some(until)) => {
                parts.push(format!(
                    "{} to {}",
                    date(since, offset),
                    date(until, offset)
                ));
            }
            (Some(since), None) => parts.push(format!("from {}", date(since, offset))),
            (None, Some(until)) => parts.push(format!("to {}", date(until, offset))),
            (None, None) => {}
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }

    /// The pause clause, while the wait has not run out.
    fn pause_clause(&self, now: Instant) -> Option<String> {
        let remaining = self.paused_until.filter(|until| *until > now)? - now;
        // Round up: a wait with any part of a second left still needs that second.
        let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);

        Some(format!("paused: retry in {seconds}s"))
    }

    /// Whether `spec` is the search and an answer to it is still awaited.
    fn awaits(&self, spec: &SearchSpec) -> bool {
        self.in_flight && self.is_for(spec)
    }

    /// How many distinct chats the held hits span.
    fn distinct_chats(&self) -> usize {
        self.hits
            .iter()
            .map(|hit| hit.chat_id)
            .collect::<HashSet<_>>()
            .len()
    }

    /// Whether `spec` is the search being waited on or shown.
    fn is_for(&self, spec: &SearchSpec) -> bool {
        self.spec.as_ref() == Some(spec)
    }
}

/// The `YYYY-MM-DD` a unix timestamp names on a clock `offset` seconds from UTC.
fn date(seconds: i64, offset: i64) -> String {
    let (year, month, day) = civil_from_days(seconds.saturating_add(offset).div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days`: day 0 is 1970-01-01, and a negative day
/// counts back before it.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text hit in `chat_id`, with the message identifier as its text.
    fn hit(chat_id: i64, message_id: i64) -> GlobalHit {
        GlobalHit {
            chat_id,
            message_id,
            text: format!("m{message_id}"),
            media: None,
            sent_at: None,
            outgoing: false,
        }
    }

    /// `count` hits in chat 1, identified 1 to `count`.
    fn one_chat(count: usize) -> Vec<GlobalHit> {
        (1..=count)
            .map(|id| hit(1, i64::try_from(id).expect("test ids fit in an i64")))
            .collect()
    }

    /// A state with `spec` answered by `hits`.
    fn answered(spec: &SearchSpec, hits: Vec<GlobalHit>, total: usize) -> GlobalSearchState {
        answered_with(spec, hits, total, 0, false)
    }

    /// A state with `spec` answered by `hits`, and the rest of the answer.
    fn answered_with(
        spec: &SearchSpec,
        hits: Vec<GlobalHit>,
        total: usize,
        dropped: usize,
        has_more: bool,
    ) -> GlobalSearchState {
        let mut state = GlobalSearchState::default();
        state.begin(spec);
        assert!(state.adopt(spec, hits, total, dropped, has_more));
        state
    }

    #[test]
    fn hits_group_into_contiguous_runs_in_server_order() {
        let spec = SearchSpec::plain("x");
        let state = answered(
            &spec,
            vec![hit(1, 13), hit(1, 12), hit(2, 11), hit(1, 10)],
            4,
        );

        let groups = state.groups();
        let shape: Vec<(i64, usize, usize)> = groups
            .iter()
            .map(|group| (group.chat_id, group.first_index, group.hits.len()))
            .collect();

        // A chat the server interleaves with another is two groups, not one.
        assert_eq!(shape, vec![(1, 0, 2), (2, 2, 1), (1, 3, 1)]);
        assert_eq!(groups[2].hits[0].message_id, 10);
    }

    #[test]
    fn selection_wraps_at_both_ends() {
        let spec = SearchSpec::plain("x");
        let mut state = answered(&spec, vec![hit(1, 3), hit(1, 2), hit(2, 1)], 3);

        state.move_selection(1);
        assert_eq!(state.selected(), 1);

        state.move_selection(2);
        assert_eq!(state.selected(), 0, "forward past the end wraps to the top");

        state.move_selection(-1);
        assert_eq!(state.selected(), 2, "back past the top wraps to the end");

        state.move_selection(-7);
        assert_eq!(state.selected(), 1, "a move larger than the list wraps too");
        assert_eq!(state.selected_hit().map(|hit| hit.message_id), Some(2));
    }

    #[test]
    fn selection_on_an_empty_list_does_nothing() {
        let spec = SearchSpec::plain("x");
        let mut state = answered(&spec, Vec::new(), 0);
        state.move_selection(3);
        assert_eq!(state.selected(), 0);
        assert!(state.selected_hit().is_none());
    }

    #[test]
    fn an_answer_arrives_with_nothing_searched_for_is_refused() {
        let spec = SearchSpec::plain("a");
        let mut state = GlobalSearchState::default();

        assert!(!state.adopt(&spec, vec![hit(1, 1)], 1, 0, false));
        assert!(!state.append(&spec, vec![hit(1, 1)], 1, 0, false));
        assert!(!state.fail(&spec, "flood"));
        assert!(!state.pause(&spec, Duration::from_secs(9)));
        assert!(!state.is_active());
    }

    #[test]
    fn a_later_answer_replaces_an_earlier_one() {
        let spec = SearchSpec::plain("q");
        let mut state = answered(&spec, vec![hit(1, 1)], 1);

        state.begin(&spec);
        assert!(state.adopt(&spec, vec![hit(2, 5), hit(2, 6)], 2, 0, false));

        assert_eq!(state.hits().len(), 2);
        assert_eq!(state.hits()[0].chat_id, 2);
        assert_eq!(state.total(), 2);
        assert_eq!(state.selected(), 0);
        assert!(!state.in_flight());
    }

    #[test]
    fn the_cap_no_longer_truncates_an_answer() {
        let spec = SearchSpec::plain("q");
        let count = 150;
        let newest_first: Vec<GlobalHit> = one_chat(count).into_iter().rev().collect();
        let state = answered(&spec, newest_first, count);

        assert_eq!(state.len(), count, "every hit the server sent is kept");
        assert_eq!(
            state.hits()[0].message_id,
            i64::try_from(count).expect("the count fits in an i64")
        );
        assert_eq!(state.hits()[count - 1].message_id, 1);
        assert_eq!(state.total(), count);
        assert_eq!(state.label(0), "?q — 150 results in 1 chat");
    }

    #[test]
    fn pages_append_and_keep_the_selection() {
        let spec = SearchSpec::plain("q");
        let mut state = GlobalSearchState::default();
        state.begin(&spec);
        assert!(state.adopt(&spec, one_chat(3), 9, 0, true));
        state.move_selection(1);

        state.begin(&spec);
        assert!(state.append(&spec, one_chat(3), 9, 0, true));
        assert_eq!(state.len(), 6, "the second page is added, not swapped in");
        assert_eq!(state.total(), 9);
        assert_eq!(
            state.selected(),
            1,
            "appending moves nothing under the reader"
        );
        assert_eq!(state.hits()[3].message_id, 1, "page two follows page one");

        state.begin(&spec);
        assert!(state.append(&spec, one_chat(3), 9, 0, false));
        assert_eq!(state.len(), 9, "three pages, all kept");
        assert_eq!(state.hits()[8].message_id, 3);
        assert!(!state.has_more());
        assert!(!state.needs_more());
        assert_eq!(state.label(0), "?q — 9 results in 1 chat");
    }

    #[test]
    fn an_answer_for_a_replaced_spec_is_refused() {
        let old = SearchSpec::plain("a");
        let new = SearchSpec::plain("b");
        let mut state = GlobalSearchState::default();
        state.begin(&old);
        state.begin(&new);

        assert!(!state.adopt(&old, vec![hit(1, 1)], 1, 0, false));
        assert!(!state.append(&old, vec![hit(1, 2)], 2, 0, false));
        assert!(!state.fail(&old, "flood"));
        assert!(!state.pause(&old, Duration::from_secs(9)));

        assert!(state.in_flight());
        assert!(state.hits().is_empty());
        assert_eq!(state.spec(), Some(&new));
        assert_eq!(state.label(0), "?b — searching…");
    }

    #[test]
    fn a_pause_keeps_the_rows_and_counts_down() {
        let spec = SearchSpec::plain("q");
        let mut state = answered(&spec, vec![hit(1, 1), hit(2, 2)], 2);

        assert!(state.pause(&spec, Duration::from_secs(30)));
        assert!(state.is_paused());
        assert_eq!(state.hits().len(), 2, "a flood wait keeps what was fetched");

        let until = state.paused_until().expect("a pause names its end");
        let start = until
            .checked_sub(Duration::from_secs(30))
            .expect("the test clock is after the epoch");

        assert_eq!(
            state.label_at(start, 0),
            "?q — 2 results in 2 chats, paused: retry in 30s",
            "the count clause stays in front of the pause"
        );
        assert_eq!(
            state.label_at(start + Duration::from_secs(30), 0),
            "?q — 2 results in 2 chats",
            "the wait has run out, and the rows are still named"
        );
        assert_eq!(
            state.label_at(start + Duration::from_secs(31), 0),
            "?q — 2 results in 2 chats"
        );

        let mut empty = GlobalSearchState::default();
        empty.begin(&spec);
        assert!(empty.pause(&spec, Duration::from_secs(30)));
        let until = empty.paused_until().expect("a pause names its end");
        let start = until
            .checked_sub(Duration::from_secs(30))
            .expect("the test clock is after the epoch");
        assert_eq!(
            empty.label_at(start, 0),
            "?q — paused: retry in 30s",
            "with no rows there is no count clause"
        );
    }

    #[test]
    fn the_dropped_count_reads_in_the_status_line() {
        let spec = SearchSpec::plain("q");

        let nine_in_three = answered_with(
            &spec,
            vec![
                hit(1, 9),
                hit(1, 8),
                hit(1, 7),
                hit(2, 6),
                hit(2, 5),
                hit(2, 4),
                hit(3, 3),
                hit(3, 2),
                hit(3, 1),
            ],
            12,
            3,
            false,
        );
        assert_eq!(
            nine_in_three.label(0),
            "?q — 9 results in 3 chats, 3 dropped"
        );

        let nothing_kept = answered_with(&spec, Vec::new(), 0, 4, false);
        assert_eq!(nothing_kept.label(0), "?q — no matches, 4 dropped");

        let one_kept = answered_with(&spec, vec![hit(1, 1)], 2, 1, false);
        assert_eq!(one_kept.label(0), "?q — 1 result in 1 chat, 1 dropped");
    }

    #[test]
    fn a_filtered_count_drops_the_chat_count() {
        let media = SearchSpec {
            media: MediaFilter::PhotoVideo,
            ..SearchSpec::plain("q")
        };
        let state = answered(&media, one_chat(9), 9);
        assert_eq!(state.label(0), "?q — photo_video — 9 results");

        let both_dates = SearchSpec {
            media: MediaFilter::PhotoVideo,
            since: Some(1_767_225_600), // 2026-01-01
            until: Some(1_769_904_000), // 2026-02-01
            ..SearchSpec::plain("q")
        };
        let state = answered(&both_dates, one_chat(9), 9);
        assert_eq!(
            state.label(0),
            "?q — photo_video, 2026-01-01 to 2026-02-01 — 9 results"
        );

        let since_only = SearchSpec {
            since: Some(1_767_225_600),
            ..SearchSpec::plain("q")
        };
        assert_eq!(
            answered(&since_only, one_chat(9), 9).label(0),
            "?q — from 2026-01-01 — 9 results"
        );

        let until_only = SearchSpec {
            until: Some(1_769_904_000),
            ..SearchSpec::plain("q")
        };
        assert_eq!(
            answered(&until_only, one_chat(9), 9).label(0),
            "?q — to 2026-02-01 — 9 results"
        );
    }

    #[test]
    fn a_page_nobody_asked_for_is_refused() {
        let spec = SearchSpec::plain("q");
        let mut state = answered_with(&spec, one_chat(2), 5, 0, true);

        // The answer settled the sequence: a straggler from an earlier one
        // carries a cursor that is not this list's, so it must not land.
        assert!(!state.append(&spec, one_chat(2), 5, 0, true));
        assert!(!state.adopt(&spec, vec![hit(9, 9)], 1, 0, false));
        assert_eq!(state.len(), 2);
        assert_eq!(state.hits()[0].chat_id, 1);
    }

    #[test]
    fn a_dropped_count_accumulates_across_pages() {
        let spec = SearchSpec::plain("q");
        let mut state = answered_with(&spec, one_chat(2), 6, 1, true);

        state.begin(&spec);
        assert!(state.append(&spec, one_chat(2), 6, 2, true));

        assert_eq!(state.dropped(), 3);
        assert_eq!(state.label(0), "?q — 4 results in 1 chat, 3 dropped");
    }

    #[test]
    fn the_chat_count_counts_chats_not_runs() {
        let spec = SearchSpec::plain("hi");
        let state = answered(&spec, vec![hit(1, 3), hit(2, 2), hit(1, 1)], 3);

        assert_eq!(state.groups().len(), 3, "three runs are drawn");
        assert_eq!(state.label(0), "?hi — 3 results in 2 chats");
    }

    #[test]
    fn a_date_filter_names_the_day_in_the_readers_zone() {
        const TEHRAN: i64 = 3 * 3_600 + 30 * 60;
        const NEW_YORK: i64 = -5 * 3_600;

        // `since:2026-03-01` in Tehran starts at 2026-02-28T20:30Z.
        let since = SearchSpec {
            since: Some(1_772_310_600),
            ..SearchSpec::plain("q")
        };
        assert_eq!(
            answered(&since, one_chat(2), 2).label(TEHRAN),
            "?q — from 2026-03-01 — 2 results"
        );

        // `until:2026-03-02` in New York ends at 2026-03-03T04:59:59Z.
        let until = SearchSpec {
            until: Some(1_772_513_999),
            ..SearchSpec::plain("q")
        };
        assert_eq!(
            answered(&until, one_chat(2), 2).label(NEW_YORK),
            "?q — to 2026-03-02 — 2 results"
        );
    }

    #[test]
    fn needs_more_is_false_while_a_page_is_in_flight_or_paused() {
        let spec = SearchSpec::plain("q");
        let mut state = answered_with(&spec, one_chat(2), 5, 0, true);
        assert!(state.needs_more());

        state.begin(&spec);
        assert!(state.is_paging(), "the same spec is a continuation");
        assert!(!state.needs_more(), "one page at a time");

        assert!(state.settle(&spec));
        assert!(state.needs_more());

        assert!(state.pause(&spec, Duration::from_secs(30)));
        assert!(
            !state.needs_more(),
            "nothing is asked for during a flood wait"
        );
    }

    #[test]
    fn settle_keeps_the_rows_and_stops_asking() {
        let spec = SearchSpec::plain("q");
        let mut state = answered_with(&spec, one_chat(2), 5, 0, true);
        state.begin(&spec);
        assert!(state.in_flight());

        assert!(state.settle(&spec));

        assert_eq!(state.hits().len(), 2, "an abandoned page keeps what it had");
        assert_eq!(state.total(), 5);
        assert_eq!(state.selected(), 0);
        assert!(!state.in_flight());
        assert_eq!(state.label(0), "?q — 2 results in 1 chat");
        assert!(
            state.needs_more(),
            "a later j at the end of the list asks again"
        );
    }

    #[test]
    fn clearing_forgets_the_search() {
        let spec = SearchSpec::plain("q");
        let mut state = answered(&spec, vec![hit(1, 1)], 1);
        state.clear();

        assert!(!state.is_active());
        assert_eq!(state.spec(), None);
        assert!(state.hits().is_empty());
        assert_eq!(state.label(0), "");
    }

    #[test]
    fn an_empty_query_is_no_search() {
        let spec = SearchSpec::plain("q");
        let mut state = answered(&spec, vec![hit(1, 1)], 1);
        state.begin(&SearchSpec::plain(""));

        assert!(!state.is_active());
        assert_eq!(state.label(0), "");
    }

    #[test]
    fn a_failure_clears_the_hits_and_says_why() {
        let spec = SearchSpec::plain("q");
        let mut state = answered(&spec, Vec::new(), 0);
        state.begin(&spec);
        assert!(state.fail(&spec, "flood wait"));

        assert!(!state.in_flight());
        assert_eq!(state.label(0), "?q — search failed (flood wait)");
    }

    #[test]
    fn label_while_searching() {
        let mut state = GlobalSearchState::default();
        state.begin(&SearchSpec::plain("hi"));
        assert_eq!(state.label(0), "?hi — searching…");
    }

    #[test]
    fn label_with_no_matches() {
        let spec = SearchSpec::plain("hi");
        let state = answered(&spec, Vec::new(), 0);
        assert_eq!(state.label(0), "?hi — no matches");
    }

    #[test]
    fn label_with_one_result_in_one_chat() {
        let spec = SearchSpec::plain("hi");
        let state = answered(&spec, vec![hit(1, 1)], 1);
        assert_eq!(state.label(0), "?hi — 1 result in 1 chat");
    }

    #[test]
    fn label_with_several_results_across_chats() {
        let spec = SearchSpec::plain("hi");
        let state = answered(&spec, vec![hit(1, 1), hit(1, 2), hit(2, 3)], 3);
        assert_eq!(state.label(0), "?hi — 3 results in 2 chats");
    }

    #[test]
    fn display_body_prefers_text_then_the_attachment_label() {
        let text = hit(1, 1);
        assert_eq!(text.display_body(), "m1");

        let photo = GlobalHit {
            chat_id: 1,
            message_id: 2,
            text: String::new(),
            media: Some(MediaKind::Photo),
            sent_at: None,
            outgoing: false,
        };
        assert_eq!(photo.display_body(), "[image]");

        let captioned = GlobalHit {
            text: "at the pier".to_owned(),
            ..photo.clone()
        };
        assert_eq!(captioned.display_body(), "at the pier");

        let bare = GlobalHit {
            media: None,
            ..photo
        };
        assert_eq!(bare.display_body(), "");
    }
}
