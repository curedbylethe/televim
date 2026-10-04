//! Message entity.

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    Sending,
    Sent,
    Failed,
    Received,
}

/// What a message carries, when it carries something other than text.
///
/// The variants are deliberately coarse: `televim` distinguishes them only to
/// give a message with no caption something to show, so the exact set can follow
/// what the interface turns out to need rather than what Telegram can express.
/// `File` is the catch-all — a sticker, a video note, a contact, or anything this
/// build does not model — so that an unrecognised attachment still degrades to
/// something visible instead of vanishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Photo,
    Video,
    Gif,
    Voice,
    File,
}

impl MediaKind {
    /// The placeholder shown for an attachment that has no caption.
    ///
    /// The words are decided here; how they are painted is the interface's
    /// business, not this model's.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Photo => "[image]",
            Self::Video => "[video]",
            Self::Gif => "[gif]",
            Self::Voice => "[voice]",
            Self::File => "[file]",
        }
    }
}

/// A message in a conversation.
///
/// `text` uses `Cow<'static, str>` so callers can borrow when possible and
/// own only when necessary ("Zero-Copy Where Possible").
#[derive(Debug, Clone)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub text: Cow<'static, str>,
    pub timestamp: i64,
    pub status: MessageStatus,
    pub is_outgoing: bool,

    /// Identifier of the message this one replies to, if it is a reply.
    ///
    /// The target is another message of the same conversation, named by the
    /// same identifier space as [`Message::id`]. A locally composed reply
    /// carries this from the moment it is written, before the server has
    /// assigned the message an identifier of its own.
    pub reply_to: Option<i64>,

    /// The kind of attachment this message carries, if it carries one.
    ///
    /// This is a fact about the message itself, not a fact about the
    /// conversation, which is why it lives here rather than beside the window.
    /// It is a plain `Copy` flag with no payload: no filename, no path, no
    /// locator. Nothing can be fetched from here yet, and keeping it
    /// payload-free means adding a field later costs a literal and no memory.
    /// A locally composed placeholder is outgoing text and carries `None`.
    pub media: Option<MediaKind>,
}

impl Message {
    #[must_use]
    pub fn is_from_self(&self) -> bool {
        self.is_outgoing
    }

    /// The body to show for this message: its caption, or a placeholder for the
    /// attachment it carries.
    ///
    /// This is derived rather than baked into `text` so that `text` stays the
    /// true caption — an edit or a fetch that carries a caption must see the
    /// caption and nothing else, and a placeholder written into `text` would
    /// read as though Telegram had sent those words. Deriving it also means the
    /// label can change without touching stored messages.
    #[must_use]
    pub fn display_body(&self) -> &str {
        if self.text.is_empty() {
            self.media.map_or("", MediaKind::label)
        } else {
            &self.text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_flag() {
        let m = Message {
            id: 1,
            chat_id: 1,
            text: Cow::Borrowed("hi"),
            timestamp: 0,
            status: MessageStatus::Sent,
            is_outgoing: true,
            reply_to: None,
            media: None,
        };
        assert!(m.is_from_self());
    }

    fn attachment(media: Option<MediaKind>, text: &'static str) -> Message {
        Message {
            id: 7,
            chat_id: 1,
            text: Cow::Borrowed(text),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media,
        }
    }

    #[test]
    fn a_captionless_attachment_degrades_to_its_label() {
        let message = attachment(Some(MediaKind::File), "");
        assert_eq!(message.display_body(), "[file]");
    }

    #[test]
    fn a_caption_is_never_replaced_by_the_label() {
        let message = attachment(Some(MediaKind::Photo), "at the pier");
        assert_eq!(message.display_body(), "at the pier");
    }

    #[test]
    fn an_empty_text_message_still_has_an_empty_body() {
        assert_eq!(attachment(None, "").display_body(), "");
    }
}
