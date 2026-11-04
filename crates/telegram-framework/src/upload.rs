//! Uploads a local attachment and sends it as a message.
//!
//! `grammers` exposes the upload stream but no progress or cancellation hook.
//! [`ProgressReader`] supplies both by wrapping the file reader: progress counts
//! bytes read by the upload, and cancellation takes effect at the next read.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use grammers_client::message::InputMessage;
use tokio::io::{AsyncRead, ReadBuf};

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::media::{MediaKind, classify_message};
use crate::updates::{MessageInfo, message_info};

impl Client {
    /// Uploads and sends a local photo or document to a conversation.
    ///
    /// Progress reports bytes read by the upload stream, not bytes acknowledged
    /// by Telegram. Cancellation is cooperative: a part already in flight can
    /// still reach Telegram.
    pub async fn send_attachment(
        &self,
        peer_id: i64,
        path: &Path,
        kind: MediaKind,
        progress: impl Fn(u64, u64) + Send,
        cancel: Arc<AtomicBool>,
    ) -> Result<MessageInfo, FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "an attachment was sent to a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|error| unreadable(path, &error))?;
        if !metadata.is_file() {
            return Err(unreadable(
                path,
                &io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"),
            ));
        }
        let total = metadata.len();
        let size = usize::try_from(total).map_err(|_| {
            unreadable(
                path,
                &io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file is too large on this platform",
                ),
            )
        })?;
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|error| unreadable(path, &error))?;
        let mut reader = ProgressReader::new(file, total, progress, cancel);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let uploaded = self
            .inner()
            .upload_stream(&mut reader, size, name)
            .await
            .map_err(|error| map_upload_error(path, &error))?;
        let input = if sends_as_photo(kind) {
            InputMessage::new().photo(uploaded)
        } else {
            InputMessage::new().file(uploaded)
        };
        let message = Box::pin(self.inner().send_message(peer, input))
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        self.flush_session();

        tracing::debug!(peer_id, message_id = message.id(), "sent an attachment");

        Ok(message_info(
            message.id(),
            peer_id,
            message.text(),
            message.date().timestamp(),
            message.outgoing(),
            message.reply_to_message_id(),
            classify_message(&message.raw),
        ))
    }
}

fn unreadable(path: &Path, error: &io::Error) -> FrameworkError {
    FrameworkError::AttachmentUnreadable {
        path: path.to_string_lossy().into_owned(),
        reason: error.to_string(),
    }
}

fn map_upload_error(path: &Path, error: &io::Error) -> FrameworkError {
    if let Some(source) = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<AttachmentReadError>())
    {
        FrameworkError::AttachmentUnreadable {
            path: path.to_string_lossy().into_owned(),
            reason: source.0.to_string(),
        }
    } else if error.kind() == io::ErrorKind::Interrupted {
        FrameworkError::UploadAborted
    } else {
        FrameworkError::Request(RequestError::Network(error.to_string()))
    }
}

fn sends_as_photo(kind: MediaKind) -> bool {
    kind == MediaKind::Photo
}

#[derive(Debug)]
struct AttachmentReadError(io::Error);

impl fmt::Display for AttachmentReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl Error for AttachmentReadError {}

struct ProgressReader<S, F> {
    inner: S,
    total: u64,
    sent: u64,
    progress: Pin<Box<F>>,
    cancel: Arc<AtomicBool>,
}

impl<S, F> ProgressReader<S, F> {
    fn new(inner: S, total: u64, progress: F, cancel: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            total,
            sent: 0,
            progress: Box::pin(progress),
            cancel,
        }
    }
}

impl<S, F> AsyncRead for ProgressReader<S, F>
where
    S: AsyncRead + Unpin,
    F: Fn(u64, u64),
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.cancel.load(Ordering::Relaxed) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "attachment upload cancelled",
            )));
        }

        let before = buffer.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buffer) {
            Poll::Ready(Ok(())) => {
                let read = buffer.filled().len() - before;
                if read > 0 {
                    this.sent += read as u64;
                    (this.progress.as_ref().get_ref())(this.sent, this.total);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => {
                Poll::Ready(Err(io::Error::other(AttachmentReadError(error))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Mutex;
    use std::task::{Context, Poll};

    use tokio::io::AsyncReadExt;

    use super::*;

    struct FailedReader {
        kind: io::ErrorKind,
        message: &'static str,
    }

    impl AsyncRead for FailedReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(self.kind, self.message)))
        }
    }

    #[tokio::test]
    async fn progress_counts_bytes_monotonically_until_end() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let progress_seen = Arc::clone(&seen);
        let cancel = Arc::new(AtomicBool::new(false));
        let bytes = b"upload bytes".to_vec();
        let total = bytes.len() as u64;
        let mut reader = ProgressReader::new(
            Cursor::new(bytes),
            total,
            move |sent, total| {
                progress_seen
                    .lock()
                    .expect("progress lock")
                    .push((sent, total));
            },
            cancel,
        );
        let mut output = Vec::new();

        reader.read_to_end(&mut output).await.expect("read cursor");

        assert_eq!(output, b"upload bytes");
        let seen = seen.lock().expect("progress lock");
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|(sent, seen_total)| *sent <= *seen_total));
        assert!(seen.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(seen.last(), Some(&(total, total)));
    }

    #[tokio::test]
    async fn cancel_makes_the_next_read_interrupted_and_maps_to_upload_aborted() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut reader = ProgressReader::new(Cursor::new(b"payload"), 7, |_, _| {}, cancel);
        let mut buffer = [0; 7];

        let error = reader.read(&mut buffer).await.expect_err("cancelled read");

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(matches!(
            map_upload_error(Path::new("payload"), &error),
            FrameworkError::UploadAborted
        ));
    }

    #[test]
    fn only_photo_kind_uses_photo_upload() {
        assert!(sends_as_photo(MediaKind::Photo));
        assert!(!sends_as_photo(MediaKind::File));
        assert!(!sends_as_photo(MediaKind::Sticker));
    }

    #[tokio::test]
    async fn an_unreadable_path_error_keeps_the_path() {
        let path = Path::new("missing-attachment.png");
        let error = tokio::fs::metadata(path)
            .await
            .expect_err("fixture path does not exist");

        let mapped = unreadable(path, &error);

        assert!(matches!(
            mapped,
            FrameworkError::AttachmentUnreadable { path, .. }
                if path == "missing-attachment.png"
        ));
    }

    #[tokio::test]
    async fn a_local_read_failure_keeps_its_path_and_reason() {
        let mut reader = ProgressReader::new(
            FailedReader {
                kind: io::ErrorKind::PermissionDenied,
                message: "disk read denied",
            },
            10,
            |_, _| {},
            Arc::new(AtomicBool::new(false)),
        );
        let mut buffer = [0; 10];
        let error = reader
            .read(&mut buffer)
            .await
            .expect_err("disk read failure");

        let mapped = map_upload_error(Path::new("attachment.pdf"), &error);

        assert!(matches!(
            mapped,
            FrameworkError::AttachmentUnreadable { path, reason }
                if path == "attachment.pdf" && reason == "disk read denied"
        ));
    }

    #[tokio::test]
    async fn an_interrupted_local_read_is_not_an_upload_cancellation() {
        let mut reader = ProgressReader::new(
            FailedReader {
                kind: io::ErrorKind::Interrupted,
                message: "disk read interrupted",
            },
            10,
            |_, _| {},
            Arc::new(AtomicBool::new(false)),
        );
        let mut buffer = [0; 10];
        let error = reader
            .read(&mut buffer)
            .await
            .expect_err("interrupted disk read");

        let mapped = map_upload_error(Path::new("attachment.pdf"), &error);

        assert!(matches!(
            mapped,
            FrameworkError::AttachmentUnreadable { path, reason }
                if path == "attachment.pdf" && reason == "disk read interrupted"
        ));
    }

    #[test]
    fn upload_io_error_becomes_a_request_failure() {
        let error = io::Error::other("server failed to store uploaded data");
        let mapped = map_upload_error(Path::new("upload.bin"), &error);

        assert!(matches!(
            mapped,
            FrameworkError::Request(RequestError::Network(_))
        ));
    }
}
