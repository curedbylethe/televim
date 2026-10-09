//! Internal DTOs. Never expose `grammers` types from here.
//!
//! "Internal" is enforced rather than merely intended: this module is private
//! to the crate and neither DTO is re-exported, so everything `proto` hands out
//! is a `domain` type or this crate's own error.
//!
//! A DTO exists because a conversion between the framework's description and a
//! `domain` type cannot be written in one step — both sides are foreign to this
//! crate — so the DTO is the one place the two can meet.
//!
//! Each DTO has two conversions: one from the description `telegram-framework`
//! produced, and one into the `domain` type. The first can only be compiled
//! when the framework's client is — it is `live`-gated — while the second needs
//! nothing but `domain`, so it is always compiled and always tested.
//!
//! # One mapping here is deliberately not exhaustive
//!
//! [`chat_kind`] matches the framework's peer taxonomy exhaustively, so a new
//! peer kind stops the build. [`media_kind`] does not, and that is the point: a
//! media kind this build has never heard of must still reach the user as
//! something to open, so the catch-all is [`MediaKind::File`]. The choice of
//! vocabulary belongs to the interface, not to this crate.

use std::borrow::Cow;

use domain::chat::{Chat, ChatKind};
use domain::message::{MediaKind, Message, MessageStatus};
use domain::presence::Presence;
use telegram_framework::UserPresence;

#[cfg(feature = "live")]
use telegram_framework::{DialogInfo, DialogKind, MessageInfo};

/// A conversation on its way from the framework into `domain`.
///
/// The fields line up with [`Chat`] one for one. The preview is owned because
/// it arrives over the network and there is nothing in this process to borrow
/// it from; `Chat` takes a `Cow` so that a caller building one from its own
/// constant can still avoid the copy.
#[derive(Debug, Clone)]
pub(crate) struct ProtoChat {
    pub id: i64,
    pub title: String,

    /// What the peer is.
    ///
    /// A [`ChatKind`] rather than a "is this private" flag, because Telegram
    /// has four answers to that question and a flag has two. A bot is neither a
    /// person nor a group, so folding it into either loses the distinction
    /// here — before the one layer that decides what televim displays has had a
    /// chance to look at it.
    pub kind: ChatKind,

    pub unread_count: u32,

    /// Identifier of the message the preview came from, if there is one.
    ///
    /// Carried through so that the domain can match an edit to the message the
    /// conversation is actually showing, rather than inferring it from the
    /// timestamp — which two messages can share.
    pub last_message_id: Option<i64>,

    pub last_timestamp: Option<i64>,
    pub last_message: Option<String>,

    /// Whether the account has pinned this chat to the top of the list.
    pub pinned: bool,

    /// Whether the peer is a deleted account. Carried through so the forward
    /// picker can leave it out; the chat list still shows it.
    pub deleted: bool,
}

/// A message on its way from the framework into `domain`.
///
/// There is no status field, because Telegram does not report one. A message
/// has either reached the account or it has not, so the only thing that
/// separates "sent" from "received" is who wrote it — that is
/// [`is_outgoing`](ProtoMessage::is_outgoing), and [`Message::status`] is
/// derived from it.
#[derive(Debug, Clone)]
pub(crate) struct ProtoMessage {
    pub id: i64,
    pub chat_id: i64,
    pub text: String,
    pub timestamp: i64,
    pub is_outgoing: bool,

    /// Identifier of the message this one replies to, if it is a reply.
    ///
    /// A reply's target is another message of the same conversation, so this is
    /// already in the identifier space [`Message`] uses and needs no adjustment.
    pub reply_to: Option<i64>,

    /// What the message carries, if anything.
    ///
    /// Already the domain's own kind rather than the framework's: this is the
    /// one place the two are reconciled, and nothing downstream of the DTO
    /// should have to know there was another vocabulary. `None` means Telegram
    /// said the message carries no media, which is a different thing from a
    /// kind this build does not model — that arrives as
    /// [`MediaKind::File`].
    pub media: Option<MediaKind>,
}

impl From<ProtoChat> for Chat {
    fn from(chat: ProtoChat) -> Self {
        Self {
            id: chat.id,
            title: chat.title,
            kind: chat.kind,
            last_message: chat.last_message.map(Cow::Owned),
            unread_count: chat.unread_count,
            last_message_id: chat.last_message_id,
            last_timestamp: chat.last_timestamp,
            pinned: chat.pinned,
            deleted: chat.deleted,
            presence: None,
        }
    }
}

impl From<ProtoMessage> for Message {
    fn from(message: ProtoMessage) -> Self {
        Self {
            id: message.id,
            chat_id: message.chat_id,
            text: Cow::Owned(message.text),
            timestamp: message.timestamp,
            status: status_of(message.is_outgoing),
            is_outgoing: message.is_outgoing,
            reply_to: message.reply_to,
            media: message.media,
        }
    }
}

/// The status a message has once Telegram has delivered it.
///
/// [`MessageStatus::Sending`] and [`MessageStatus::Failed`] describe a local
/// attempt that has not been acknowledged, which is not a state anything
/// arriving over the feed can be in. So the direction is the whole of the
/// answer: a message the account wrote and that has come back is
/// [`MessageStatus::Sent`], and everything else came from someone else and is
/// [`MessageStatus::Received`].
fn status_of(is_outgoing: bool) -> MessageStatus {
    if is_outgoing {
        MessageStatus::Sent
    } else {
        MessageStatus::Received
    }
}

/// Turns a conversation the framework described into this crate's DTO.
///
/// Two fields are renamed on the way through — the framework says `peer_id`
/// where `domain` says `id`, and `last_text` where it says `last_message` — so
/// this is a conversion rather than a shared struct, and the renaming is the
/// part worth a test.
#[cfg(feature = "live")]
impl From<DialogInfo> for ProtoChat {
    fn from(dialog: DialogInfo) -> Self {
        Self {
            id: dialog.peer_id,
            title: dialog.title,
            kind: chat_kind(dialog.kind),
            unread_count: dialog.unread_count,
            last_message_id: dialog.last_message_id,
            last_timestamp: dialog.last_timestamp,
            last_message: dialog.last_text,
            pinned: dialog.pinned,
            deleted: dialog.deleted,
        }
    }
}

/// Turns a message the framework described into this crate's DTO.
///
/// The conversation is the message's peer, which the framework calls
/// `chat_peer_id`; everything else carries over unchanged.
#[cfg(feature = "live")]
impl From<MessageInfo> for ProtoMessage {
    fn from(message: MessageInfo) -> Self {
        Self {
            id: message.id,
            chat_id: message.chat_peer_id,
            text: message.text,
            timestamp: message.timestamp,
            is_outgoing: message.is_outgoing,
            // Telegram reports a reply target as an `i32`; the domain counts in
            // `i64`, and widening here is what keeps the two spaces joined.
            reply_to: message.reply_to_msg_id.map(i64::from),
            media: message.media.map(media_kind),
        }
    }
}

/// Maps the framework's media taxonomy onto the domain's, degrading rather than
/// failing.
///
/// This is the one mapping in the crate that is **not** exhaustive, and the
/// contrast with [`chat_kind`] is deliberate. A peer kind added to the framework
/// should stop this compiling, because every peer kind has a consequence the
/// client cannot avoid. A media kind added to the framework has none to avoid:
/// the file is still a file, the user can still open it, and the only question
/// is what to call it. So an unrecognised kind becomes [`MediaKind::File`]
/// instead of `None` — a message that visibly carries something must not arrive
/// looking as though it carries nothing — and the build stays green.
#[cfg(feature = "live")]
// The catch-all is the requirement here: an unrecognised kind must degrade to
// a file, and a build failure is the one answer that cannot degrade. The lint
// this silences asks for exhaustiveness, which is exactly what is wrong.
#[allow(clippy::match_wildcard_for_single_variants)]
fn media_kind(kind: telegram_framework::media::MediaKind) -> MediaKind {
    match kind {
        telegram_framework::media::MediaKind::Photo => MediaKind::Photo,
        telegram_framework::media::MediaKind::Video => MediaKind::Video,
        telegram_framework::media::MediaKind::Gif => MediaKind::Gif,
        telegram_framework::media::MediaKind::Voice => MediaKind::Voice,
        telegram_framework::media::MediaKind::Sticker => MediaKind::Sticker,
        // The framework's own `File`, and every kind it has not heard of either:
        // all of them are "a thing this message carries".
        _ => MediaKind::File,
    }
}

/// Maps the framework's peer taxonomy onto the domain's.
///
/// The two line up one for one and the match says so: a kind added to the
/// framework stops this compiling instead of falling through to a wrong answer
/// at runtime. That is the guarantee a "this peer kind is unsupported" error
/// would have stood in for, and it is the stronger of the two — a build that
/// fails cannot be missed, and there is no branch left to leave untested.
///
/// Visible to the rest of the crate because the fetch uses it as its filter:
/// the kind a dialog maps to is what decides whether a `Chat` is worth building
/// at all, and that has to be the same mapping the conversion uses.
#[cfg(feature = "live")]
pub(crate) fn chat_kind(kind: DialogKind) -> ChatKind {
    match kind {
        DialogKind::PrivateUser => ChatKind::Private,
        DialogKind::Bot => ChatKind::Bot,
        DialogKind::Group => ChatKind::Group,
        DialogKind::Channel => ChatKind::Channel,
    }
}

/// Translates the framework's presence into the domain's.
///
/// Total and exhaustive on purpose: a new framework variant stops the build
/// here rather than arriving in `domain` as a silent default. `was_online` is
/// carried through unchanged.
pub(crate) fn to_presence(presence: UserPresence) -> Presence {
    match presence {
        UserPresence::Online => Presence::Online,
        UserPresence::Offline { was_online } => Presence::Offline { was_online },
        UserPresence::Recently => Presence::Recently,
        UserPresence::LastWeek => Presence::LastWeek,
        UserPresence::LastMonth => Presence::LastMonth,
        UserPresence::Hidden => Presence::Hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_framework_presence_maps_to_its_domain_twin_keeping_the_timestamp() {
        assert_eq!(to_presence(UserPresence::Online), Presence::Online);
        assert_eq!(
            to_presence(UserPresence::Offline {
                was_online: 1_700_000_000
            }),
            Presence::Offline {
                was_online: 1_700_000_000
            }
        );
        assert_eq!(to_presence(UserPresence::Recently), Presence::Recently);
        assert_eq!(to_presence(UserPresence::LastWeek), Presence::LastWeek);
        assert_eq!(to_presence(UserPresence::LastMonth), Presence::LastMonth);
        assert_eq!(to_presence(UserPresence::Hidden), Presence::Hidden);
    }

    /// A conversation with everything but the fields under test filled in.
    fn proto_chat(id: i64, kind: ChatKind) -> ProtoChat {
        ProtoChat {
            id,
            title: format!("chat {id}"),
            kind,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            last_message: None,
            deleted: false,
        }
    }

    /// A message with everything but the fields under test filled in.
    fn proto_message(is_outgoing: bool) -> ProtoMessage {
        ProtoMessage {
            id: 1,
            chat_id: 42,
            text: "hello".to_owned(),
            timestamp: 1_700_000_000,
            is_outgoing,
            reply_to: None,
            media: None,
        }
    }

    #[test]
    fn every_kind_reaches_the_domain_unchanged() {
        for kind in [
            ChatKind::Private,
            ChatKind::Bot,
            ChatKind::Group,
            ChatKind::Channel,
        ] {
            let chat: Chat = proto_chat(1, kind).into();
            assert_eq!(
                chat.kind, kind,
                "a bot is not a group, and neither is a channel"
            );
        }
    }

    #[test]
    fn a_preview_becomes_owned_text_or_stays_absent() {
        let mut source = proto_chat(1, ChatKind::Private);

        source.last_message = Some("see you at six".to_owned());
        let chat: Chat = source.into();
        assert_eq!(chat.last_message.as_deref(), Some("see you at six"));

        let bare: Chat = proto_chat(2, ChatKind::Private).into();
        assert_eq!(bare.last_message, None, "a chat need not have a preview");
    }

    #[test]
    fn counts_and_timestamps_survive_the_trip() {
        let mut source = proto_chat(1, ChatKind::Private);
        source.unread_count = 3;
        source.last_message_id = Some(7);
        source.last_timestamp = Some(1_700_000_000);

        let chat: Chat = source.into();
        assert_eq!(chat.id, 1);
        assert_eq!(chat.title, "chat 1");
        assert_eq!(chat.unread_count, 3);
        assert_eq!(chat.last_message_id, Some(7));
        assert_eq!(chat.last_timestamp, Some(1_700_000_000));

        let dateless: Chat = proto_chat(2, ChatKind::Private).into();
        assert_eq!(
            dateless.last_timestamp, None,
            "a conversation with no messages has no timestamp, not the epoch"
        );
        assert_eq!(
            dateless.last_message_id, None,
            "and nothing to point a preview at"
        );
    }

    #[test]
    fn a_message_reports_its_direction_as_its_status() {
        let outgoing: Message = proto_message(true).into();
        assert!(
            matches!(outgoing.status, MessageStatus::Sent),
            "the account wrote it and telegram gave it back, so it went out"
        );
        assert!(outgoing.is_outgoing);

        let incoming: Message = proto_message(false).into();
        assert!(
            matches!(incoming.status, MessageStatus::Received),
            "nobody has to acknowledge a message someone else wrote"
        );
        assert!(!incoming.is_outgoing);
    }

    #[test]
    fn a_message_keeps_its_text_and_place() {
        let message: Message = proto_message(false).into();

        assert_eq!(message.id, 1);
        assert_eq!(message.chat_id, 42);
        assert_eq!(message.text, "hello");
        assert_eq!(message.timestamp, 1_700_000_000);
    }

    #[test]
    fn a_reply_target_survives_the_trip_intact() {
        let mut source = proto_message(false);
        source.reply_to = Some(9);

        let message: Message = source.into();
        assert_eq!(
            message.reply_to,
            Some(9),
            "a reply names the message it answers, and the target is not renumbered"
        );

        let bare: Message = proto_message(true).into();
        assert_eq!(bare.reply_to, None, "an ordinary message answers nothing");
    }

    #[test]
    fn a_media_kind_survives_the_hop_into_the_domain() {
        let mut source = proto_message(false);
        source.media = Some(MediaKind::Photo);

        let message: Message = source.into();
        assert_eq!(
            message.media,
            Some(MediaKind::Photo),
            "a message that carries something must not arrive reading as one that does not"
        );

        let bare: Message = proto_message(false).into();
        assert_eq!(
            bare.media, None,
            "no media said is no media, which is not the same as an unknown kind"
        );
    }

    /// The two halves have to compose: a kind that reaches `domain` intact is
    /// what lets the filter drop a bot rather than mistake it for a person.
    #[test]
    fn only_people_survive_the_filter() {
        let chats: Vec<Chat> = vec![
            proto_chat(1, ChatKind::Private).into(),
            proto_chat(2, ChatKind::Bot).into(),
            proto_chat(3, ChatKind::Group).into(),
            proto_chat(4, ChatKind::Channel).into(),
            proto_chat(5, ChatKind::Private).into(),
        ];

        let visible = domain::chat::filter_private(chats);

        assert_eq!(
            visible.iter().map(|chat| chat.id).collect::<Vec<_>>(),
            vec![1, 5],
            "televim displays people and nobody else"
        );
    }
}

/// The conversions from the framework's descriptions.
///
/// Gated with the types they convert from. The rest of this module's tests need
/// no datacenter and no feature, and run on every job; these run whenever the
/// client does.
#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;

    /// A conversation as the framework describes it.
    fn dialog(peer_id: i64, kind: DialogKind) -> DialogInfo {
        DialogInfo {
            peer_id,
            title: format!("chat {peer_id}"),
            kind,
            deleted: false,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            last_text: None,
        }
    }

    /// A message as the framework describes it.
    fn message_info(chat_peer_id: i64) -> MessageInfo {
        MessageInfo {
            id: 7,
            chat_peer_id,
            text: "hello".to_owned(),
            timestamp: 1_700_000_000,
            is_outgoing: false,
            reply_to_msg_id: None,
            media: None,
            media_id: None,
        }
    }

    #[test]
    fn every_dialog_kind_reaches_the_domain() {
        let cases = [
            (DialogKind::PrivateUser, ChatKind::Private),
            (DialogKind::Bot, ChatKind::Bot),
            (DialogKind::Group, ChatKind::Group),
            (DialogKind::Channel, ChatKind::Channel),
        ];

        for (source, expected) in cases {
            let chat: Chat = ProtoChat::from(dialog(1, source)).into();
            assert_eq!(chat.kind, expected, "{source:?} was mistranslated");
        }
    }

    #[test]
    fn a_deleted_account_stays_deleted_in_the_domain() {
        let mut source = dialog(1, DialogKind::PrivateUser);
        source.deleted = true;

        let chat: Chat = ProtoChat::from(source).into();

        assert!(chat.deleted);
        assert!(!Chat::from(ProtoChat::from(dialog(2, DialogKind::PrivateUser))).deleted);
    }

    #[test]
    fn a_dialog_becomes_a_chat_without_losing_or_swapping_a_field() {
        let mut source = dialog(42, DialogKind::PrivateUser);
        source.title = "Ada".to_owned();
        source.unread_count = 3;
        source.last_message_id = Some(7);
        source.last_timestamp = Some(1_700_000_000);
        source.last_text = Some("see you at six".to_owned());
        source.pinned = true;

        let chat: Chat = ProtoChat::from(source).into();

        assert_eq!(
            chat.id, 42,
            "the peer identifier is the identifier of the conversation"
        );
        assert_eq!(chat.title, "Ada");
        assert_eq!(chat.unread_count, 3);
        assert_eq!(
            chat.last_message_id,
            Some(7),
            "the preview's identifier has to survive, or an edit cannot reach it"
        );
        assert_eq!(chat.last_timestamp, Some(1_700_000_000));
        assert_eq!(
            chat.last_message.as_deref(),
            Some("see you at six"),
            "the framework's last_text is the domain's last_message"
        );
        assert!(
            chat.pinned,
            "the pin has to reach the list or it cannot sort"
        );
    }

    #[test]
    fn a_dialog_with_no_messages_has_neither_a_preview_nor_a_timestamp() {
        let chat: Chat = ProtoChat::from(dialog(42, DialogKind::PrivateUser)).into();

        assert_eq!(chat.last_message_id, None);
        assert_eq!(chat.last_timestamp, None);
        assert_eq!(chat.last_message, None);
    }

    #[test]
    fn a_message_becomes_a_message_whose_conversation_is_its_peer() {
        let mut source = message_info(42);
        source.is_outgoing = true;
        source.reply_to_msg_id = Some(5);

        let message: Message = ProtoMessage::from(source).into();

        assert_eq!(message.id, 7);
        assert_eq!(
            message.chat_id, 42,
            "the framework's chat_peer_id is the domain's chat_id"
        );
        assert_eq!(message.text, "hello");
        assert_eq!(message.timestamp, 1_700_000_000);
        assert!(message.is_outgoing);
        assert!(matches!(message.status, MessageStatus::Sent));
        assert_eq!(
            message.reply_to,
            Some(5),
            "a reply target has to survive the widening, or the excerpt cannot find its message"
        );
    }

    /// Every kind the framework can name reaches the domain as itself.
    ///
    /// The framework's `File` is also what it reports for a kind this build does
    /// not model, so the row below is the catch-all's observable behaviour: a
    /// message carrying an unrecognised thing still arrives as a file rather
    /// than as nothing.
    #[test]
    fn every_media_kind_reaches_the_domain_unchanged() {
        let cases = [
            (
                telegram_framework::media::MediaKind::Photo,
                MediaKind::Photo,
            ),
            (
                telegram_framework::media::MediaKind::Video,
                MediaKind::Video,
            ),
            (telegram_framework::media::MediaKind::Gif, MediaKind::Gif),
            (
                telegram_framework::media::MediaKind::Voice,
                MediaKind::Voice,
            ),
            (
                telegram_framework::media::MediaKind::Sticker,
                MediaKind::Sticker,
            ),
            (
                // The catch-all, for every kind Telegram adds and this build has
                // never heard of.
                telegram_framework::media::MediaKind::File,
                MediaKind::File,
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(media_kind(source), expected, "{source:?} was mistranslated");
        }
    }

    /// A message that carries no media says so, and an unknown kind does not get
    /// to make the same claim.
    #[test]
    fn only_a_message_without_media_arrives_without_a_kind() {
        let mut bare = message_info(42);
        bare.media = None;
        let message: Message = ProtoMessage::from(bare).into();
        assert_eq!(message.media, None, "telegram said there was nothing there");

        let mut unknown = message_info(42);
        unknown.media = Some(telegram_framework::media::MediaKind::File);
        let message: Message = ProtoMessage::from(unknown).into();
        assert_eq!(
            message.media,
            Some(MediaKind::File),
            "an unmodelled kind degrades to a file; it must not degrade to nothing"
        );
        assert_eq!(
            message.display_body(),
            "hello",
            "a caption still wins over the placeholder, which is what the label is for"
        );
    }

    /// The whole path the chat list takes, on the framework's own shapes: an
    /// unfiltered list, translated, then filtered down to what televim shows.
    #[test]
    fn only_people_survive_a_translated_dialog_list() {
        let dialogs = vec![
            dialog(1, DialogKind::PrivateUser),
            dialog(2, DialogKind::Bot),
            dialog(3, DialogKind::Group),
            dialog(4, DialogKind::Channel),
            dialog(5, DialogKind::PrivateUser),
        ];

        let chats: Vec<Chat> = dialogs
            .into_iter()
            .map(|dialog| ProtoChat::from(dialog).into())
            .collect();

        let visible = domain::chat::filter_private(chats);

        assert_eq!(
            visible.iter().map(|chat| chat.id).collect::<Vec<_>>(),
            vec![1, 5],
            "a bot and a channel are translated faithfully only to be dropped here"
        );
    }
}
