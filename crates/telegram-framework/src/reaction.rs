//! Sending reactions, and reading the counts a message carries.
//!
//! `grammers` stops at this module's boundary. [`Reaction`] and
//! [`ReactionCount`] are built from an emoticon, a document id or the word
//! "paid" and a number, so `proto` can translate them without ever naming a
//! `grammers` type — which is what `make boundary` asserts for the whole crate.
//!
//! # Why the counts are read out of the raw message
//!
//! `grammers` exposes `Message::reaction_count()`, and it is a total: how many
//! reactions of any kind the message has, and nothing about which. Per-emoticon
//! counts — what a strip of chips is drawn from, and which of them is the
//! reader's own — are only in `Message.raw`, so [`reaction_counts`] reads them
//! from there. That is the one place this crate reads a `tl` value field by
//! field for something the accessors do not carry; the same argument as
//! `history::message_from_raw`, and the same consequence: a message arriving
//! over the feed and the same message read from a page are described by the
//! same function.
//!
//! # What can be sent, and what can only be shown
//!
//! [`Reaction`] names everything Telegram can report, because a reader has to
//! see all of it. Only an emoticon can be *sent* through this crate: a custom
//! emoji would need its document resolved first, and a paid reaction is bought
//! rather than chosen. [`Client::set_reaction`] therefore takes an emoticon or
//! nothing, and the two read-only kinds never reach it.

use grammers_client::message::InputReactions;

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::tl;

/// One reaction someone can leave on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// A standard emoji, as the emoticon it is drawn from.
    Emoticon(String),

    /// A custom emoji, as the document that carries it.
    ///
    /// Only the identifier: the document itself, and whether this account may
    /// use it, are Telegram's to answer when it is fetched.
    CustomEmoji(i64),

    /// A paid reaction, which carries no emoticon at all.
    Paid,
}

/// How many people left one reaction on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionCount {
    /// Which reaction this counts.
    pub reaction: Reaction,

    /// How many left it.
    pub count: u32,

    /// Whether the logged-in account is one of them.
    ///
    /// Telegram says this with an ordering number rather than a flag — the
    /// reader's own reaction sorts first — and the flag is the whole of what
    /// that means here.
    pub chosen: bool,
}

/// The reactions a raw message carries, in the order Telegram sent them.
///
/// Empty for a message nobody has reacted to, which is the same answer a
/// message that carries no reaction field gives: both are "nothing to draw".
pub(crate) fn reaction_counts(raw: &tl::enums::Message) -> Vec<ReactionCount> {
    match raw {
        tl::enums::Message::Message(message) => message
            .reactions
            .as_ref()
            .map_or_else(Vec::new, counts_from),
        tl::enums::Message::Service(message) => message
            .reactions
            .as_ref()
            .map_or_else(Vec::new, counts_from),
        // An empty message is a message Telegram has nothing to say about, and
        // nobody can react to one.
        tl::enums::Message::Empty(_) => Vec::new(),
    }
}

/// The counts inside a raw `MessageReactions` value.
///
/// Shared with the update feed, which carries the same value as the message
/// does: one extractor, so a strip drawn from a page and one redrawn from an
/// update cannot disagree.
pub(crate) fn counts_from(reactions: &tl::enums::MessageReactions) -> Vec<ReactionCount> {
    // One constructor today, so this is a destructuring rather than a match —
    // and a refutable pattern the moment Telegram grows a second, which is the
    // build error wanted.
    let tl::enums::MessageReactions::Reactions(list) = reactions;

    list.results
        .iter()
        .filter_map(|count| {
            let tl::enums::ReactionCount::Count(count) = count;

            Some(ReactionCount {
                reaction: reaction(&count.reaction)?,
                count: u32::try_from(count.count).unwrap_or(0),
                chosen: count.chosen_order.is_some(),
            })
        })
        .collect()
}

/// One raw reaction, or `None` when the wire is naming an absence.
///
/// `Reaction::Empty` is what a reaction field carries when it names none, so
/// dropping it here is what keeps a zero-count chip off the screen. Every other
/// kind is described, and a kind added to the schema stops this compiling.
fn reaction(reaction: &tl::enums::Reaction) -> Option<Reaction> {
    match reaction {
        tl::enums::Reaction::Emoji(emoji) => Some(Reaction::Emoticon(emoji.emoticon.clone())),
        tl::enums::Reaction::CustomEmoji(custom) => Some(Reaction::CustomEmoji(custom.document_id)),
        tl::enums::Reaction::Paid => Some(Reaction::Paid),
        tl::enums::Reaction::Empty => None,
    }
}

impl Client {
    /// Sets this account's own reaction to a message, or clears it.
    ///
    /// `peer_id` is the conversation's *bare* identifier — the one a
    /// [`DialogInfo`](crate::DialogInfo) reports — and `emoticon` is the
    /// reaction to leave, or `None` to remove the one this account left.
    ///
    /// Nothing comes back, and nothing is applied locally: the reaction that
    /// lands is the server's, and the strip is redrawn from the update it sends
    /// back. Optimism here would draw a chip the server might refuse.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UnknownPeer`] when the conversation is not in
    /// the session's peer cache, and [`FrameworkError::Request`] when Telegram
    /// rejects the request or the connection fails.
    pub async fn set_reaction(
        &self,
        peer_id: i64,
        message_id: i32,
        emoticon: Option<&str>,
    ) -> Result<(), FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "a reaction was sent to a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        let reactions = match emoticon {
            Some(emoticon) => InputReactions::emoticon(emoticon),
            None => InputReactions::remove(),
        };

        self.inner()
            .send_reactions(peer, message_id, reactions)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        self.flush_session();

        tracing::debug!(
            peer_id,
            message_id,
            removing = emoticon.is_none(),
            "changed a reaction"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::raw_message;

    /// Puts `results` on a raw message, the way `GetHistory` hands one over.
    fn with_counts(results: Vec<tl::enums::ReactionCount>) -> tl::enums::Message {
        let mut raw = raw_message(None);
        let tl::enums::Message::Message(message) = &mut raw else {
            panic!("the fixture is an ordinary message");
        };
        message.reactions = Some(tl::enums::MessageReactions::Reactions(
            tl::types::MessageReactions {
                min: false,
                can_see_list: false,
                reactions_as_tags: false,
                results,
                recent_reactions: None,
                top_reactors: None,
            },
        ));
        raw
    }

    fn count(
        reaction: tl::enums::Reaction,
        count: i32,
        chosen_order: Option<i32>,
    ) -> tl::enums::ReactionCount {
        tl::enums::ReactionCount::Count(tl::types::ReactionCount {
            chosen_order,
            reaction,
            count,
        })
    }

    fn emoji(emoticon: &str) -> tl::enums::Reaction {
        tl::enums::Reaction::Emoji(tl::types::ReactionEmoji {
            emoticon: emoticon.to_owned(),
        })
    }

    #[test]
    fn a_message_without_reactions_carries_none() {
        assert!(reaction_counts(&raw_message(None)).is_empty());
    }

    /// The reader's own reaction is the one the wire marks with an ordering
    /// number, and that number is the only thing that says so.
    #[test]
    fn every_reaction_kind_is_read_out_of_a_raw_message() {
        let raw = with_counts(vec![
            count(emoji("👍"), 3, None),
            count(
                tl::enums::Reaction::CustomEmoji(tl::types::ReactionCustomEmoji {
                    document_id: 5_000_000_000,
                }),
                2,
                None,
            ),
            count(tl::enums::Reaction::Paid, 1, Some(0)),
        ]);

        let counts = reaction_counts(&raw);

        assert_eq!(
            counts,
            vec![
                ReactionCount {
                    reaction: Reaction::Emoticon("👍".to_owned()),
                    count: 3,
                    chosen: false,
                },
                ReactionCount {
                    reaction: Reaction::CustomEmoji(5_000_000_000),
                    count: 2,
                    chosen: false,
                },
                ReactionCount {
                    reaction: Reaction::Paid,
                    count: 1,
                    chosen: true,
                },
            ],
            "a chip is its kind, its count, and whether it is the reader's"
        );
    }

    /// `reactionEmpty` names no reaction, so it is not a chip with a zero on it.
    #[test]
    fn an_empty_reaction_names_nothing() {
        let raw = with_counts(vec![count(tl::enums::Reaction::Empty, 0, None)]);

        assert!(reaction_counts(&raw).is_empty());
    }
}
