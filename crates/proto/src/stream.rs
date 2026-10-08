//! The update feed, as `domain` types.
//!
//! The framework hands over events it has already narrowed to private
//! conversations and described in primitives and strings. All that is left is
//! to put each one into the `domain` type that applies it, which is what this
//! module does — no clock, no allocation beyond what the event itself carries.

use std::borrow::Cow;

use domain::message::Message;
use domain::updates::UpdateEvent;
use telegram_framework::{MessageInfo, UpdateKind};

use crate::error::ProtoError;
use crate::types::{ProtoMessage, to_presence};

/// A live feed of the updates for the conversations televim displays.
///
/// Obtained from
/// [`ProtoClient::subscribe_updates`](crate::ProtoClient::subscribe_updates),
/// which can be called once. The subscription owns the stream underneath, so
/// dropping it stops delivery: hold it for as long as updates are wanted.
#[derive(Debug)]
pub struct UpdateStream {
    inner: telegram_framework::UpdateSubscription,
}

impl UpdateStream {
    /// Wraps the framework's subscription.
    pub(crate) fn new(inner: telegram_framework::UpdateSubscription) -> Self {
        Self { inner }
    }

    /// Awaits the next update.
    ///
    /// `None` means the feed has ended, which only happens once the client is
    /// shutting down; a caller that sees it should stop polling rather than
    /// retry. An `Err` is a request that failed while resolving a gap in the
    /// sequence, and the feed is still usable afterwards.
    ///
    /// Updates televim does not display never surface here — the framework
    /// discards them — so this can await for a long time without producing
    /// anything, and a caller should not read a delay as a fault.
    pub async fn next(&mut self) -> Option<Result<UpdateEvent, ProtoError>> {
        match self.inner.next().await? {
            Ok(kind) => Some(Ok(to_event(kind))),
            Err(error) => Some(Err(error.into())),
        }
    }

    /// How many updates the framework has discarded so far.
    ///
    /// Everything Telegram sends that is not a message in a private
    /// conversation lands there, so a count that climbs while the account is
    /// quiet is the filter doing its job — and a count that stays where it was
    /// while something is known to be arriving says the filter is not running.
    /// It is the only thing here that reports on what the feed threw away
    /// rather than on what it kept.
    pub fn dropped(&self) -> u64 {
        self.inner.dropped()
    }

    /// Records how far the feed got, and writes the session back.
    ///
    /// Call this once the feed has been read to its end. It is explicit because
    /// the position is synchronised through an `async` call that a destructor
    /// cannot make, so a stream dropped without it leaves the session pointing at
    /// an older position and the next launch replays updates that were already
    /// seen. Consumes the stream, because there is nothing left to read.
    pub async fn finish(self) -> Result<(), ProtoError> {
        self.inner.finish().await?;
        Ok(())
    }
}

/// Translates an event the framework described into the domain's vocabulary.
fn to_event(kind: UpdateKind) -> UpdateEvent {
    match kind {
        UpdateKind::NewMessage(message) => UpdateEvent::NewMessage(to_message(message)),

        // An edit names a message rather than carrying a whole one, so it is
        // taken apart here instead of being routed through `ProtoMessage`:
        // there is no status to derive and no direction to keep.
        UpdateKind::MessageEdited(message) => UpdateEvent::MessageEdited {
            chat_id: message.chat_peer_id,
            message_id: message.id,
            new_text: Cow::Owned(message.text),
        },

        // The event names no conversation, and neither does the domain's copy
        // of it, so the identifiers pass straight through.
        UpdateKind::MessagesDeleted { message_ids } => UpdateEvent::MessagesDeleted { message_ids },

        // Both halves are already the domain's own units — a conversation
        // identifier and a watermark — so this is a rename rather than a
        // conversion: `chat_peer_id` is what `domain` calls `chat_id`.
        UpdateKind::ReadReceipt {
            chat_peer_id,
            max_id,
        } => UpdateEvent::ReadReceipt {
            chat_id: chat_peer_id,
            max_id,
        },

        // A rename again, and the flag is the whole event: there is no message
        // to convert and no position to keep, so nothing else happens here.
        UpdateKind::PeerTyping {
            chat_peer_id,
            typing,
        } => UpdateEvent::PeerTyping {
            chat_id: chat_peer_id,
            typing,
        },

        // A rename plus the presence mapping: the timestamp inside `Offline`
        // rides through `to_presence` untouched.
        UpdateKind::PeerStatus {
            chat_peer_id,
            presence,
        } => UpdateEvent::PeerStatus {
            chat_id: chat_peer_id,
            presence: to_presence(presence),
        },
    }
}

/// Builds the domain's message from the framework's description of one.
///
/// The trip goes through [`ProtoMessage`] rather than straight across, and it
/// has to: neither crate can implement the conversion on its own. `domain`
/// cannot name a framework type, and this crate cannot write a `From` between
/// two types that are both foreign to it. The DTO in the middle is the one
/// place the whole thing can meet, which is also why it is the seam the other
/// conversions use.
fn to_message(info: MessageInfo) -> Message {
    ProtoMessage::from(info).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::message::MessageStatus;
    use domain::presence::Presence;
    use telegram_framework::MessageInfo;
    use telegram_framework::UserPresence;

    /// A message as the framework describes one.
    fn info(chat_peer_id: i64) -> MessageInfo {
        MessageInfo {
            id: 7,
            chat_peer_id,
            text: "hello".to_owned(),
            timestamp: 1_700_000_000,
            is_outgoing: false,
            reply_to_msg_id: None,
            media: None,
        }
    }

    #[test]
    fn an_arrival_becomes_a_message_that_names_its_conversation() {
        let mut source = info(42);
        source.media = Some(telegram_framework::media::MediaKind::Photo);
        let event = to_event(UpdateKind::NewMessage(source));

        let UpdateEvent::NewMessage(message) = event else {
            panic!("an arrival is a new message");
        };

        assert_eq!(message.chat_id, 42);
        assert_eq!(message.id, 7);
        assert_eq!(message.text, "hello");
        assert_eq!(message.timestamp, 1_700_000_000);
        assert!(matches!(message.status, MessageStatus::Received));
        assert_eq!(
            message.media,
            Some(domain::message::MediaKind::Photo),
            "an arrival that carries something must not lose what it carries on the way in"
        );
    }

    #[test]
    fn an_arrival_the_account_sent_comes_back_as_sent() {
        let mut source = info(42);
        source.is_outgoing = true;

        let UpdateEvent::NewMessage(message) = to_event(UpdateKind::NewMessage(source)) else {
            panic!("an arrival is a new message");
        };

        assert!(message.is_outgoing);
        assert!(matches!(message.status, MessageStatus::Sent));
    }

    #[test]
    fn an_edit_becomes_the_message_it_changed_and_the_text_that_replaced_it() {
        let mut source = info(42);
        source.text = "after".to_owned();

        let UpdateEvent::MessageEdited {
            chat_id,
            message_id,
            new_text,
        } = to_event(UpdateKind::MessageEdited(source))
        else {
            panic!("an edit names the message it changed");
        };

        assert_eq!(chat_id, 42);
        assert_eq!(message_id, 7);
        assert_eq!(new_text, "after");
    }

    #[test]
    fn a_deletion_passes_its_identifiers_through() {
        let event = to_event(UpdateKind::MessagesDeleted {
            message_ids: vec![7, 8],
        });

        let UpdateEvent::MessagesDeleted { message_ids } = event else {
            panic!("a deletion carries identifiers and nothing else");
        };

        assert_eq!(message_ids, vec![7, 8]);
    }

    /// The framework names the conversation the way it names one everywhere else
    /// and the domain calls it `chat_id`; the watermark is the same number in both
    /// layers, so the whole translation is that one rename.
    #[test]
    fn a_read_acknowledgement_becomes_the_conversation_and_the_watermark() {
        let event = to_event(UpdateKind::ReadReceipt {
            chat_peer_id: 42,
            max_id: 7,
        });

        let UpdateEvent::ReadReceipt { chat_id, max_id } = event else {
            panic!("a read acknowledgement is a conversation and a watermark");
        };

        assert_eq!(chat_id, 42);
        assert_eq!(max_id, 7);
    }

    /// The same rename carries the typing flag in both directions: a composing
    /// action arrives as `true` and its cancel as `false`, and neither loses the
    /// conversation it was about on the way.
    #[test]
    fn a_typing_update_becomes_the_conversation_and_the_flag() {
        for typing in [true, false] {
            let event = to_event(UpdateKind::PeerTyping {
                chat_peer_id: 42,
                typing,
            });

            let UpdateEvent::PeerTyping {
                chat_id,
                typing: flag,
            } = event
            else {
                panic!("a typing update is a conversation and a flag");
            };

            assert_eq!(chat_id, 42);
            assert_eq!(flag, typing);
        }
    }

    /// Every framework presence reaches its domain twin, and the timestamp in
    /// `Offline` is the same number on both sides of the trip.
    #[test]
    fn a_status_update_becomes_the_conversation_and_its_presence() {
        let cases = [
            (UserPresence::Online, Presence::Online),
            (
                UserPresence::Offline {
                    was_online: 1_700_000_000,
                },
                Presence::Offline {
                    was_online: 1_700_000_000,
                },
            ),
            (UserPresence::Recently, Presence::Recently),
            (UserPresence::LastWeek, Presence::LastWeek),
            (UserPresence::LastMonth, Presence::LastMonth),
            (UserPresence::Hidden, Presence::Hidden),
        ];

        for (presence, expected) in cases {
            let event = to_event(UpdateKind::PeerStatus {
                chat_peer_id: 42,
                presence,
            });

            let UpdateEvent::PeerStatus {
                chat_id,
                presence: mapped,
            } = event
            else {
                panic!("a status update is a conversation and a presence");
            };

            assert_eq!(chat_id, 42);
            assert_eq!(mapped, expected, "{presence:?}");
        }
    }
}
