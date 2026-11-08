//! Best-effort desktop notification delivery.
#![expect(dead_code, reason = "stage 06 wires the notification trigger")]

use domain::message::{MediaKind, Message};

/// One desktop notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub title: String,
    pub body: String,
}

/// A notification sink that never blocks its caller.
pub(crate) trait Notifier: Send + Sync {
    fn notify(&self, notice: Notice);
}

/// Delivers notices through the platform notification service.
pub(crate) struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&self, notice: Notice) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::debug!("desktop notification skipped outside the runtime");
            return;
        };

        drop(runtime.spawn_blocking(move || {
            if let Err(error) = notify_rust::Notification::new()
                .summary(&notice.title)
                .body(&notice.body)
                .show()
            {
                tracing::debug!(%error, "desktop notification was not shown");
            }
        }));
    }
}

/// Builds a single-line notification body, or `None` when the message has no body.
pub(crate) fn body_line(message: &Message) -> Option<String> {
    let text = message.text.trim();
    let body = if text.is_empty() {
        message.media.map(MediaKind::label)?
    } else {
        text.lines().next()?
    };
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = body.chars();
    let mut line: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        line.push('…');
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use domain::message::MessageStatus;

    use super::*;

    fn message(text: &'static str, media: Option<MediaKind>) -> Message {
        Message {
            id: 1,
            chat_id: 1,
            text: Cow::Borrowed(text),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media,
            media_id: None,
        }
    }

    #[test]
    fn body_uses_first_line_and_collapses_whitespace() {
        assert_eq!(
            body_line(&message(" hello   there\nsecond", None)).as_deref(),
            Some("hello there")
        );
    }

    #[test]
    fn a_caption_is_used_instead_of_its_media_label() {
        assert_eq!(
            body_line(&message("caption", Some(MediaKind::Photo))).as_deref(),
            Some("caption")
        );
    }

    #[test]
    fn captionless_media_uses_its_label() {
        assert_eq!(
            body_line(&message("", Some(MediaKind::Photo))).as_deref(),
            Some("[image]")
        );
    }

    #[test]
    fn a_whitespace_only_message_without_media_has_no_body() {
        assert_eq!(body_line(&message("  \n ", None)), None);
    }

    #[test]
    fn long_bodies_truncate_on_a_character_boundary() {
        let text = format!("{}é", "x".repeat(160));
        let message = Message {
            text: Cow::Owned(text),
            ..message("", None)
        };
        let line = body_line(&message).expect("text has a body");
        assert_eq!(line.chars().count(), 161);
        assert!(line.ends_with('…'));
    }
}
