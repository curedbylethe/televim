//! Debug-only scenario feeder for the offline PTY tests.
//!
//! An offline run has no feed, so the two states that only the network can
//! produce — a message arriving and a page in flight — cannot happen on their
//! own. The test writes a small script and hands over its path in
//! `TELEVIM_SCENARIO`; this module waits for the sibling `<path>.fire` and then
//! replays the script into the loop's own channel, where each directive is
//! handled exactly as the network's answer would be.
//!
//! The gate is `#[cfg(debug_assertions)]`, so the shipped binary carries no
//! `TELEVIM_SCENARIO` string and no module.
//!
//! Script grammar, one directive per line, blank lines ignored:
//!
//! ```text
//! arrival <chat_id> <message_id> <text...>
//! fetch older|newer
//! upload-progress <chat_id> <temp_id> <sent> <total>
//! upload-sent <chat_id> <temp_id> <real_id> image|file
//! upload-failed <chat_id> <temp_id> <reason...>
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use domain::message::{MediaKind, Message, MessageStatus};
use tokio::sync::mpsc;
use tui::FetchDirection;

use crate::net::Event as NetEvent;
use crate::runtime::AppEvent;
use proto::ProtoError;
use telegram_framework::{FrameworkError, RequestError};

/// One directive from the script.
pub enum Event {
    /// A message arrived in the conversation its `chat_id` names.
    Arrival(Message),

    /// A page was asked for, and is now in flight.
    Fetch(FetchDirection),

    /// A synthetic answer from the network path.
    Net(NetEvent),
}

/// How long the feeder waits before looking for the trigger file again.
const POLL: Duration = Duration::from_millis(25);

/// Sends the script's directives once the test writes `<path>.fire`.
///
/// A plain thread with a poll rather than a timer: the test writes the trigger
/// only once the screen shows the state the script must not disturb, so the
/// ordering is a file's existence rather than a guess about elapsed time.
pub fn spawn(tx: mpsc::UnboundedSender<AppEvent>) {
    let Some(path) = std::env::var_os("TELEVIM_SCENARIO").map(PathBuf::from) else {
        return;
    };
    std::thread::spawn(move || {
        let script = match std::fs::read_to_string(&path) {
            Ok(script) => script,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "scenario script unreadable");
                return;
            }
        };
        let fire = fire_path(&path);
        while !fire.exists() {
            std::thread::sleep(POLL);
        }
        for line in script.lines() {
            let Some(event) = parse(line) else {
                if !line.trim().is_empty() {
                    tracing::warn!(line, "scenario line ignored");
                }
                continue;
            };
            if tx.send(AppEvent::Scenario(event)).is_err() {
                break;
            }
        }
    });
}

/// The trigger file's path: the script's own, with `.fire` appended.
fn fire_path(path: &Path) -> PathBuf {
    let mut fire = path.as_os_str().to_os_string();
    fire.push(".fire");
    PathBuf::from(fire)
}

/// One line of the script, or `None` when it is not a directive.
fn parse(line: &str) -> Option<Event> {
    let mut words = line.split_whitespace();
    match words.next()? {
        "arrival" => {
            let chat_id = words.next()?.parse().ok()?;
            let id = words.next()?.parse().ok()?;
            let text = words.collect::<Vec<_>>().join(" ");
            Some(Event::Arrival(Message {
                id,
                chat_id,
                text: text.into(),
                // The seeded history stamps a message with its id, and an
                // arrival that does the same leaves the day separator in place.
                timestamp: id,
                status: MessageStatus::Received,
                is_outgoing: false,
                reply_to: None,
                media: None,
                media_id: None,
            }))
        }
        "fetch" => match words.next()? {
            "older" => Some(Event::Fetch(FetchDirection::Older)),
            "newer" => Some(Event::Fetch(FetchDirection::Newer)),
            _ => None,
        },
        "upload-progress" => {
            let chat_id = words.next()?.parse().ok()?;
            let temp_id = words.next()?.parse().ok()?;
            let sent = words.next()?.parse().ok()?;
            let total = words.next()?.parse().ok()?;
            Some(Event::Net(NetEvent::UploadProgress {
                chat_id,
                temp_id,
                sent,
                total,
            }))
        }
        "upload-sent" => {
            let chat_id = words.next()?.parse().ok()?;
            let temp_id = words.next()?.parse().ok()?;
            let real_id = words.next()?.parse().ok()?;
            let media = match words.next()? {
                "image" => MediaKind::Photo,
                "file" => MediaKind::File,
                _ => return None,
            };
            Some(Event::Net(NetEvent::Sent {
                chat_id,
                temp_id,
                result: Ok(Message {
                    id: real_id,
                    chat_id,
                    text: "".into(),
                    timestamp: real_id,
                    status: MessageStatus::Sent,
                    is_outgoing: true,
                    reply_to: None,
                    media: Some(media),
                    media_id: None,
                }),
            }))
        }
        "upload-failed" => {
            let chat_id = words.next()?.parse().ok()?;
            let temp_id = words.next()?.parse().ok()?;
            let reason = words.collect::<Vec<_>>().join(" ");
            if reason.is_empty() {
                return None;
            }
            let error = if reason == "cancelled" {
                FrameworkError::UploadAborted
            } else {
                FrameworkError::Request(RequestError::Network(reason))
            };
            Some(Event::Net(NetEvent::Sent {
                chat_id,
                temp_id,
                result: Err(ProtoError::Framework(error)),
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrival_line_builds_the_message_it_names() {
        let Some(Event::Arrival(message)) = parse("arrival 1 41 hello there") else {
            panic!("the line is a directive");
        };

        assert_eq!(message.chat_id, 1);
        assert_eq!(message.id, 41);
        assert_eq!(message.text, "hello there");
        assert_eq!(message.timestamp, 41, "the seed's rule: the id is the time");
        assert!(!message.is_outgoing);
    }

    #[test]
    fn a_fetch_line_names_its_direction() {
        assert!(matches!(
            parse("fetch older"),
            Some(Event::Fetch(FetchDirection::Older))
        ));
        assert!(matches!(
            parse("fetch newer"),
            Some(Event::Fetch(FetchDirection::Newer))
        ));
    }

    #[test]
    fn upload_progress_line_builds_the_progress_event() {
        assert!(matches!(
            parse("upload-progress 1 -1 42 100"),
            Some(Event::Net(NetEvent::UploadProgress {
                chat_id: 1,
                temp_id: -1,
                sent: 42,
                total: 100,
            }))
        ));
    }

    #[test]
    fn upload_sent_line_builds_the_confirmed_photo_or_file() {
        let Some(Event::Net(NetEvent::Sent {
            chat_id,
            temp_id,
            result: Ok(message),
        })) = parse("upload-sent 1 -1 9 image")
        else {
            panic!("the line is an upload success");
        };
        assert_eq!((chat_id, temp_id), (1, -1));
        assert_eq!(message.id, 9);
        assert_eq!(message.media, Some(MediaKind::Photo));
        assert!(message.is_outgoing);

        assert!(matches!(
            parse("upload-sent 1 -1 9 file"),
            Some(Event::Net(NetEvent::Sent {
                result: Ok(Message {
                    media: Some(MediaKind::File),
                    ..
                }),
                ..
            }))
        ));
    }

    #[test]
    fn upload_failed_line_preserves_reason_and_maps_cancel_exactly() {
        assert!(matches!(
            parse("upload-failed 1 -1 cancelled"),
            Some(Event::Net(NetEvent::Sent {
                result: Err(ProtoError::Framework(FrameworkError::UploadAborted)),
                ..
            }))
        ));
        assert!(matches!(
            parse("upload-failed 1 -1 server failed"),
            Some(Event::Net(NetEvent::Sent {
                result: Err(ProtoError::Framework(FrameworkError::Request(
                    RequestError::Network(reason)
                ))),
                ..
            })) if reason == "server failed"
        ));
    }

    #[test]
    fn lines_that_are_not_directives_are_ignored() {
        assert!(parse("").is_none());
        assert!(parse("fetch sideways").is_none());
        assert!(parse("arrival 1 not-a-number text").is_none());
        assert!(parse("depart 1 2").is_none());
        assert!(parse("upload-progress 1 -1 bad 100").is_none());
        assert!(parse("upload-sent 1 -1 9 video").is_none());
        assert!(parse("upload-failed 1 -1").is_none());
    }
}
