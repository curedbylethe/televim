//! What a message carries, read off the wire — and how to fetch it.
//!
//! # Why the classification is a free function over primitives
//!
//! Both message paths — the hand-built `GetHistory` in [`history`] and the
//! `grammers` update feed in [`updates`] — reach a message through a different
//! type, and neither path can be exercised without a datacenter. What *can* be
//! is the decision: given what the wire says, which kind is it? That is
//! [`classify_raw`] and [`classify_typed`], and they run on every CI job.
//!
//! # Why the answer is never "some media I did not recognise"
//!
//! Telegram adds media kinds faster than a client catches up, and a kind this
//! build has never heard of is still a file somebody expects to be able to open.
//! So anything present that is not modelled here becomes [`MediaKind::File`]
//! rather than disappearing: the residue is visible in the description, and a
//! caller can act on it. Only Telegram saying *no* media produces `None`.
//!
//! # Fetching the bytes
//!
//! [`Client::download_media_to`] re-reads the message by identifier and hands
//! each chunk to a sink as it lands, so a caller writes the file as it arrives
//! rather than holding it whole; [`Client::download_media`] is the buffered
//! convenience over it, for a caller with nowhere to stream to. The locator is
//! re-derived from the message on the way rather than kept on the description,
//! because a description is rebuilt on every history page and a stale access
//! hash is worse than none.
//!
//! A message's **embedded** thumbnail — the small preview it already carries —
//! is readable with no download at all: [`Client::embedded_thumbnail`] re-reads
//! the message the way the download path does and hands back the bytes, not a
//! locator. [`embedded_thumbnail`] is the extraction itself, a free function
//! over a media value so that "which sizes are embedded" is tested on every CI
//! job rather than only against a live account.
//!
//! Everything the transfer decides — whether the media can be fetched at all,
//! and how a chunk ends the transfer — sits in [`fetchable_media`] and
//! [`feed_chunk`], free functions over values rather than over a client, so
//! that the counting and abort rules are tested on every CI job rather than
//! only against a live account.

use grammers_client::media::{Downloadable, Media};

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::tl;

/// The kind of thing a message carries.
///
/// This is the framework's reading of the wire, not a type the rest of the
/// workspace shares: it is deliberately as small as the interface can be, and
/// says nothing about *where* the bytes are — that locator is re-derived from
/// the raw message when one is fetched.
///
/// A kind this build does not model is reported as [`MediaKind::File`]. The
/// alternative — dropping it to `None` — would make a message that visibly
/// carries something arrive as if it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// A photo.
    Photo,

    /// A video.
    Video,

    /// An animated GIF.
    Gif,

    /// A voice note.
    Voice,

    /// A static sticker.
    Sticker,

    /// An animated sticker: a TGS or `WebM` file, which is not a GIF.
    AnimatedSticker,

    /// Anything else the message carries: a plain document, or a media kind
    /// this build does not model.
    File,
}

/// Classifies the media of a raw `tl` message.
///
/// `None` means the message carries no media at all — the field is absent, or
/// it is [`MessageMedia::Empty`](tl::enums::MessageMedia::Empty). Every other
/// answer is `Some`, including for media this build does not model. The second
/// half is the media's stable id, `None` where the media has no record to read
/// one from.
pub(crate) fn classify_raw(
    media: Option<&tl::enums::MessageMedia>,
) -> Option<(MediaKind, Option<i64>)> {
    Some(match media? {
        tl::enums::MessageMedia::Empty => return None,
        tl::enums::MessageMedia::Photo(photo) => (MediaKind::Photo, photo_id(photo)),
        tl::enums::MessageMedia::Document(document) => {
            (classify_document(document), document_id(document))
        }
        // A contact, a poll, a location, a web page, a paid post — and anything
        // Telegram adds after this build. All of them are something the message
        // carries, and none of them is nothing.
        _ => (MediaKind::File, None),
    })
}

/// Classifies the media of a `grammers` message.
///
/// Agrees with [`classify_raw`] by construction: a document is decided by one
/// shared function, and the sticker `grammers` reads off a document's
/// attributes is answered here — a static sticker is [`MediaKind::Sticker`],
/// and an animated one is [`MediaKind::AnimatedSticker`]. Everything else
/// `grammers` flattens away or builds that this crate does not read — a
/// contact, a poll, a location, a web page — all land on [`MediaKind::File`],
/// the same answer the raw path gives them.
///
/// The one residue is a kind `grammers` itself declines to build: its
/// `Media::from_raw` returns `None` for a handful of variants, so on this path
/// such a message arrives here as `None`. The raw history path does not have
/// that gap.
pub(crate) fn classify_typed(media: Option<&Media>) -> Option<(MediaKind, Option<i64>)> {
    Some(match media? {
        Media::Photo(photo) => (MediaKind::Photo, photo_id(&photo.raw)),
        Media::Document(document) => (classify_document(&document.raw), document_id(&document.raw)),
        // `grammers` reads a document with a sticker attribute as a sticker, so
        // this is where a sticker arrives: the shared document classifier splits
        // it by mime, the same way the raw path does, so the two agree. Everything
        // `grammers` flattens away or builds that this crate does not read — a
        // contact, a poll, a location, a web page, and any kind added after
        // this build — is one arm, deliberately: all of them carry something,
        // and none of them is nothing.
        Media::Sticker(sticker) => (
            classify_document(&sticker.document.raw),
            document_id(&sticker.document.raw),
        ),
        _ => (MediaKind::File, None),
    })
}

/// The stable id of a photo, read off its record.
///
/// This is what `grammers` reports as `Photo::id`, read off the raw record
/// instead: that accessor unwraps the record, and a photo the response did not
/// carry has none — the case [`fetchable_media`] refuses. Only the id is kept.
/// The access hash and file reference are never stored, because they go stale.
fn photo_id(photo: &tl::types::MessageMediaPhoto) -> Option<i64> {
    match photo.photo.as_ref()? {
        tl::enums::Photo::Photo(photo) => Some(photo.id),
        tl::enums::Photo::Empty(_) => None,
    }
}

/// The stable id of a document, read off its record. The same rule as
/// [`photo_id`]: the id only, never the access hash or file reference.
fn document_id(document: &tl::types::MessageMediaDocument) -> Option<i64> {
    match document.document.as_ref()? {
        tl::enums::Document::Document(document) => Some(document.id),
        tl::enums::Document::Empty(_) => None,
    }
}

/// Decides a document's kind from what the document itself says.
///
/// The document's own `voice` flag is Telegram's statement that this is a voice
/// note, so it is asked first. The sticker attribute is asked next, because it is
/// the most specific answer: it says the document is a sticker whatever else the
/// document carries, and a `WebM` sticker also carries a video attribute while an
/// animated one also carries the animation. Only then are the remaining
/// attributes read in the order they can be told apart in: an animation without
/// a sticker attribute is a GIF, a video attribute names a video, and an audio
/// attribute only counts as a voice note when it says so — a music file carries
/// one too, and is a file.
///
/// An animated sticker is [`MediaKind::AnimatedSticker`] rather than a GIF: the
/// sticker attribute is what makes it a sticker, and the mime says how it moves.
/// Telegram names a sticker's format by the document's mime, `application/x-tgsticker`
/// for a TGS and `video/webm` for a video sticker; the animation bit is not
/// that signal, because Telegram does not send it on a TGS. A document with
/// [`DocumentAttribute::Animated`] and no sticker attribute stays [`MediaKind::Gif`].
///
/// Everything else is [`MediaKind::File`], including a document with no
/// attributes at all: it still carries bytes.
fn classify_document(document: &tl::types::MessageMediaDocument) -> MediaKind {
    if document.voice {
        return MediaKind::Voice;
    }

    let (attributes, mime) = match &document.document {
        Some(tl::enums::Document::Document(document)) => {
            (document.attributes.as_slice(), document.mime_type.as_str())
        }
        Some(tl::enums::Document::Empty(_)) | None => (&[][..], ""),
    };

    // First, so that a sticker carrying a video or an animation keeps the more
    // specific kind: both of those are how a sticker moves, not what it is.
    if attributes
        .iter()
        .any(|a| matches!(a, tl::enums::DocumentAttribute::Sticker(_)))
    {
        return if matches!(mime, "application/x-tgsticker" | "video/webm") {
            MediaKind::AnimatedSticker
        } else {
            MediaKind::Sticker
        };
    }

    if attributes
        .iter()
        .any(|a| matches!(a, tl::enums::DocumentAttribute::Animated))
    {
        return MediaKind::Gif;
    }

    if attributes
        .iter()
        .any(|a| matches!(a, tl::enums::DocumentAttribute::Video(_)))
    {
        return MediaKind::Video;
    }

    if attributes
        .iter()
        .any(|a| matches!(a, tl::enums::DocumentAttribute::Audio(audio) if audio.voice))
    {
        return MediaKind::Voice;
    }

    MediaKind::File
}

/// The media a message carries, if it is something that can be fetched.
///
/// `None` covers all three ways a fetch ends before a byte moves, and the caller
/// reports one answer for them because they are one thing to a reader: this
/// message has no attachment to open. They are:
///
/// - there is no media at all, or Telegram sent [`MessageMedia::Empty`];
/// - the media is a kind `grammers` will not build, so there is no way to ask
///   for its bytes; and
/// - the media builds but names no file — a contact, a poll, a web page, or a
///   photo whose own record the response did not carry.
///
/// The last is what `grammers` reports as `PreFailed`, and it is asked here
/// instead of there: the question has an answer before a client exists, so it
/// can be tested without a datacenter.
fn fetchable_media(media: Option<&tl::enums::MessageMedia>) -> Option<Media> {
    let media = match media {
        Some(tl::enums::MessageMedia::Empty) | None => return None,
        Some(media) => Media::from_raw(media.clone())?,
    };

    // The two things `iter_download` accepts, asked in the order it asks them:
    // an embedded thumbnail needs no request, and a locator is the request.
    (media.to_data().is_some() || media.to_raw_input_location().is_some()).then_some(media)
}

/// The fetchable media of the message named by `message_id` within a page.
///
/// Matched by identifier rather than taken from the first message: a page
/// may hold neighbours of the named message, and one of them may carry media
/// that is not the media asked for. Anything that is not that message — an
/// older sibling, an empty or a service entry — answers `None`.
fn select_media(page: &[tl::enums::Message], message_id: i64) -> Option<Media> {
    page.iter()
        .find_map(|message| match message {
            tl::enums::Message::Message(message) if i64::from(message.id) == message_id => {
                Some(message.media.as_ref())
            }
            _ => None,
        })
        .and_then(fetchable_media)
}

/// The bytes of the thumbnail a message's media already carries, if it has one.
///
/// Telegram embeds a small preview inside most photos and documents, and
/// `grammers` reconstructs the bytes from it: a cached size carries them whole,
/// a stripped size rebuilds a JPEG around them, and a vector size becomes an SVG
/// path — none of which is a request. A size that names a file instead — a
/// plain, fetchable size, or a progressive one — is not embedded, so it is
/// skipped rather than fetched; when several embedded sizes are present the
/// largest is read, which is the best this build can show without asking.
///
/// `None` for media with no thumbnail, for one whose only sizes must be
/// fetched, and for every kind this build does not read: a contact, a poll, a
/// location, a web page. Stickers are documents on the wire, so their preview —
/// a still image, a vector outline for an animated one — is read here too.
///
/// This is the function the promise "an embedded thumbnail needs no request"
/// rests on, so it is pure over the media value and touches nothing.
fn embedded_thumbnail(media: &Media) -> Option<Vec<u8>> {
    let sizes = match media {
        Media::Photo(photo) => photo.thumbs(),
        Media::Document(document) => document.thumbs(),
        Media::Sticker(sticker) => sticker.document.thumbs(),
        _ => return None,
    };

    sizes
        .into_iter()
        .filter_map(|size| size.to_data())
        .filter(|bytes| !bytes.is_empty())
        .max_by_key(Vec::len)
}

/// Feeds one chunk to `sink`, counting what has arrived and turning a `false`
/// answer into an abort.
///
/// Free of the client so the counting and abort rules are tested on every CI
/// job. `downloaded` is the running total **including** this chunk, which is
/// what the sink reports as progress: `sink` sees the chunk and the total so
/// far, and answers whether to keep going.
fn feed_chunk(
    chunk: &[u8],
    downloaded: &mut usize,
    declared: Option<usize>,
    sink: &mut impl FnMut(&[u8], usize, Option<usize>) -> bool,
    peer_id: i64,
    message_id: i64,
) -> Result<(), FrameworkError> {
    *downloaded = downloaded.saturating_add(chunk.len());

    if sink(chunk, *downloaded, declared) {
        return Ok(());
    }

    Err(FrameworkError::DownloadAborted {
        peer_id,
        message_id,
    })
}

impl Client {
    /// Re-reads the message by identifier and returns the media it carries,
    /// when that media can be fetched.
    ///
    /// The one read both [`Client::download_media_to`] and
    /// [`Client::embedded_thumbnail`] begin with. The locator is re-derived
    /// from the message on the way rather than kept on the description, because
    /// a description is rebuilt on every history page and a stale access hash is
    /// worse than none.
    ///
    /// `Ok(None)` for a message Telegram could not have numbered and for one
    /// that carries nothing fetchable; the caller decides which of those it
    /// reports as a refusal.
    async fn refetch_page_media(
        &self,
        peer_id: i64,
        message_id: i64,
    ) -> Result<Option<Media>, FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                message_id,
                "a message was re-read in a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        // Telegram numbers messages with an `i32`, so an identifier outside that
        // range names no message, and there is then nothing to read.
        let Ok(offset_id) = i32::try_from(message_id) else {
            tracing::warn!(
                peer_id,
                message_id,
                "a message was re-read with an identifier telegram could not have numbered"
            );
            return Ok(None);
        };

        // The raw route `fetch_history` uses, rather than a `grammers` accessor:
        // the media's own record — the photo, the document and its access hash —
        // is only on the raw message, and it is the thing being read.
        let request = tl::functions::messages::GetHistory {
            peer: peer.into(),
            offset_id,
            offset_date: 0,
            // A page of one starting one step toward newer messages from the
            // anchor, which includes the anchor itself: `offset_id` alone
            // excludes it, so a zero shift returns the older neighbour instead.
            add_offset: -1,
            limit: 1,
            max_id: 0,
            min_id: 0,
            hash: 0,
        };

        let response = self
            .inner()
            .invoke(&request)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        // Only reachable with a non-zero `hash`, and this request always sends
        // zero. Reported rather than unwrapped: the release profile aborts on a
        // panic, so an answer this build cannot read must not be the end of the
        // process.
        let (raw, _users, _chats) = match response {
            tl::enums::messages::Messages::Messages(page) => {
                (page.messages, page.users, page.chats)
            }
            tl::enums::messages::Messages::Slice(page) => (page.messages, page.users, page.chats),
            tl::enums::messages::Messages::ChannelMessages(page) => {
                (page.messages, page.users, page.chats)
            }
            tl::enums::messages::Messages::NotModified(_) => {
                return Err(FrameworkError::Request(RequestError::Deserialize(
                    "telegram answered a media fetch with NotModified".to_owned(),
                )));
            }
        };

        // Reading a message can cache a peer or move the datacenter, and that
        // only reaches the store if it is written back.
        self.flush_session();

        Ok(select_media(&raw, message_id))
    }

    /// Streams the media a message carries to `sink`, one chunk at a time.
    ///
    /// `peer_id` is the bare identifier of the conversation, the same number
    /// [`fetch_history`](crate::Client::fetch_history) takes, and the message
    /// must be in that conversation's peer cache.
    ///
    /// `sink` is called with each chunk, the total bytes so far including this
    /// chunk, and the size Telegram declared when it declared one. Returning
    /// `false` stops the transfer: the call returns
    /// [`FrameworkError::DownloadAborted`] and whatever the sink wrote is its
    /// own to discard. Nothing is held here between chunks, so a caller can
    /// stream an attachment far larger than the program's memory budget.
    ///
    /// # Why the message is fetched again
    ///
    /// Addressing a file takes the `access_hash` Telegram handed out with it,
    /// and [`MessageInfo`](crate::MessageInfo) does not carry one: it is rebuilt
    /// on every history page, so any locator kept on it would be stale the
    /// moment the window moved. Re-reading the message costs one request and is
    /// always right — and it keeps this a typed method, so `proto` never builds
    /// a request.
    ///
    /// `iter_download` is used rather than `download_media`, which is behind
    /// `grammers-client`'s `fs` feature: turning that on would be a
    /// workspace-level dependency change for a method that writes to a path,
    /// which nothing here wants.
    ///
    /// # Errors
    ///
    /// - [`FrameworkError::UnknownPeer`] — the conversation is not in the peer
    ///   cache, so no request can address it.
    /// - [`FrameworkError::MediaUnavailable`] — the message carries no media,
    ///   carries media that cannot be fetched, or names no message at all.
    /// - [`FrameworkError::DownloadAborted`] — `sink` returned `false` on a
    ///   chunk, so the transfer stopped there.
    /// - [`FrameworkError::Request`] — Telegram refused a request, the
    ///   connection failed, or a chunk could not be decoded.
    pub async fn download_media_to(
        &self,
        peer_id: i64,
        message_id: i64,
        mut sink: impl FnMut(&[u8], usize, Option<usize>) -> bool,
    ) -> Result<(), FrameworkError> {
        let Some(media) = self.refetch_page_media(peer_id, message_id).await? else {
            tracing::debug!(peer_id, message_id, "the message has no media to fetch");
            return Err(FrameworkError::MediaUnavailable {
                peer_id,
                message_id,
            });
        };

        let declared = media.size();
        let mut downloaded = 0_usize;
        let mut chunks = self.inner().iter_download(&media);

        while let Some(chunk) = chunks
            .next()
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?
        {
            feed_chunk(
                &chunk,
                &mut downloaded,
                declared,
                &mut sink,
                peer_id,
                message_id,
            )?;
        }

        tracing::debug!(
            peer_id,
            message_id,
            bytes = downloaded,
            "downloaded a message's media"
        );

        Ok(())
    }

    /// [`download_media_to`](Self::download_media_to) collected into one buffer.
    ///
    /// For a caller with nowhere to stream to — a sticker decoded in memory once
    /// it is whole. Deliberately not the file path: the media cache streams to
    /// disk so a large attachment is never held whole.
    ///
    /// # Errors
    ///
    /// Everything [`download_media_to`](Self::download_media_to) returns; its
    /// sink here never returns `false`, so `DownloadAborted` is not among them.
    pub async fn download_media(
        &self,
        peer_id: i64,
        message_id: i64,
    ) -> Result<Vec<u8>, FrameworkError> {
        let mut bytes = Vec::new();
        self.download_media_to(peer_id, message_id, |chunk, _, _| {
            bytes.extend_from_slice(chunk);
            true
        })
        .await?;
        Ok(bytes)
    }

    /// The bytes of the thumbnail a message's media already carries, if it has
    /// one — read off the message Telegram answers with, with no request of its
    /// own.
    ///
    /// This is the preview a document or photo ships with, and reading it costs
    /// no download: the message is re-read the way
    /// [`download_media_to`](Self::download_media_to) re-reads it — one request
    /// for the message, none for the thumbnail — so its locator is never stale.
    ///
    /// `Ok(None)` is an ordinary answer, not a failure: a photo whose preview
    /// names a file to fetch, or media of a kind without an embedded one. A
    /// message that cannot be read at all is a [`FrameworkError`], the same one
    /// the download path reports for it.
    ///
    /// # Errors
    ///
    /// - [`FrameworkError::UnknownPeer`] — the conversation is not in the peer
    ///   cache, so no request can address it.
    /// - [`FrameworkError::Request`] — Telegram refused the read, the connection
    ///   failed, or a message could not be decoded.
    pub async fn embedded_thumbnail(
        &self,
        peer_id: i64,
        message_id: i64,
    ) -> Result<Option<Vec<u8>>, FrameworkError> {
        let Some(media) = self.refetch_page_media(peer_id, message_id).await? else {
            tracing::debug!(
                peer_id,
                message_id,
                "the message has no media with an embedded thumbnail"
            );
            return Ok(None);
        };

        let thumbnail = embedded_thumbnail(&media);

        tracing::debug!(
            peer_id,
            message_id,
            bytes = thumbnail.as_ref().map_or(0, Vec::len),
            "read a message's embedded thumbnail"
        );

        Ok(thumbnail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        attribute_audio, attribute_filename, attribute_sticker, attribute_video,
        media_animated_sticker, media_document, media_document_with_thumbs, media_empty,
        media_photo, media_photo_with_sizes, media_refused_by_grammers, media_typed,
        media_unmodelled, media_webm_sticker, raw_message, thumbnail_cached, thumbnail_fetchable,
    };

    use tl::enums::DocumentAttribute;

    /// Classifies a media value the raw path way. Borrowing keeps the fixture
    /// alive, which is what lets both paths be asserted on one value.
    fn raw(media: &tl::enums::MessageMedia) -> Option<MediaKind> {
        classify_raw(Some(media)).map(|(kind, _)| kind)
    }

    fn typed(raw: tl::enums::MessageMedia) -> Option<MediaKind> {
        classify_typed(media_typed(raw).as_ref()).map(|(kind, _)| kind)
    }

    /// A document named `notes.txt` whose record carries `id`.
    fn document_with_id(id: i64) -> tl::enums::MessageMedia {
        let mut media = media_document(vec![attribute_filename("notes.txt")], false);
        let tl::enums::MessageMedia::Document(message) = &mut media else {
            panic!("media_document builds a document");
        };
        let Some(tl::enums::Document::Document(document)) = &mut message.document else {
            panic!("media_document carries a record");
        };
        document.id = id;
        media
    }

    /// The id is read off the message on both paths, and only the id: the same
    /// value comes out of the raw record and out of the `grammers` form of it.
    #[test]
    fn a_media_id_is_carried_from_the_message_on_both_paths() {
        let document = document_with_id(77);

        assert_eq!(
            classify_raw(Some(&document)),
            Some((MediaKind::File, Some(77)))
        );
        assert_eq!(
            classify_typed(media_typed(document.clone()).as_ref()),
            Some((MediaKind::File, Some(77)))
        );

        let photo = tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
            spoiler: false,
            live_photo: false,
            photo: Some(tl::enums::Photo::Photo(tl::types::Photo {
                has_stickers: false,
                id: 88,
                access_hash: 1,
                file_reference: Vec::new(),
                date: 0,
                sizes: Vec::new(),
                video_sizes: None,
                dc_id: 2,
            })),
            ttl_seconds: None,
            video: None,
        });

        assert_eq!(
            classify_raw(Some(&photo)),
            Some((MediaKind::Photo, Some(88)))
        );
    }

    /// No media, and media with no record to name it by, carry no id.
    #[test]
    fn media_without_a_record_carries_no_id() {
        assert_eq!(classify_raw(None), None, "no media is no answer at all");
        assert_eq!(
            classify_raw(Some(&media_photo())),
            Some((MediaKind::Photo, None)),
            "a photo the response did not carry has no id to read"
        );
        assert_eq!(
            classify_raw(Some(&media_unmodelled())),
            Some((MediaKind::File, None)),
            "a kind this build does not model is a file with no id"
        );
    }

    /// Both paths are asserted on the same fixture, so a disagreement between
    /// them fails here rather than in whichever caller happened to hit it first.
    #[test]
    fn a_photo_is_a_photo_on_both_paths() {
        let expected = Some(MediaKind::Photo);

        assert_eq!(raw(&media_photo()), expected);
        assert_eq!(typed(media_photo()), expected);
    }

    #[test]
    fn an_animated_document_is_a_gif_on_both_paths() {
        let gif = media_document(vec![DocumentAttribute::Animated], false);
        let expected = Some(MediaKind::Gif);

        assert_eq!(raw(&gif.clone()), expected);
        assert_eq!(typed(gif), expected);
    }

    /// An animated GIF is also a video on the wire, and the animation has to
    /// win: a GIF rendered as a silent video loses the fact it is one.
    #[test]
    fn an_animated_document_with_a_video_attribute_is_still_a_gif() {
        let attributes = vec![
            DocumentAttribute::Animated,
            attribute_video(),
            attribute_filename("cat.gif"),
        ];

        assert_eq!(
            raw(&media_document(attributes.clone(), false)),
            Some(MediaKind::Gif)
        );
        assert_eq!(
            typed(media_document(attributes, false)),
            Some(MediaKind::Gif)
        );
    }

    #[test]
    fn a_video_document_is_a_video_on_both_paths() {
        let video = media_document(vec![attribute_video()], false);
        let expected = Some(MediaKind::Video);

        assert_eq!(raw(&video.clone()), expected);
        assert_eq!(typed(video), expected);
    }

    #[test]
    fn a_voice_flag_on_the_document_makes_it_a_voice_note_on_both_paths() {
        let note = media_document(vec![attribute_audio(true)], true);
        let expected = Some(MediaKind::Voice);

        assert_eq!(raw(&note.clone()), expected);
        assert_eq!(typed(note), expected);
    }

    #[test]
    fn a_voice_attribute_alone_is_a_voice_note_on_both_paths() {
        // Telegram marks a voice note twice: a flag on the media and `voice` on
        // the audio attribute. Either one on its own is enough to read.
        let note = media_document(vec![attribute_audio(true)], false);
        let expected = Some(MediaKind::Voice);

        assert_eq!(raw(&note.clone()), expected);
        assert_eq!(typed(note), expected);
    }

    #[test]
    fn an_audio_attribute_that_is_not_a_voice_note_is_a_file_on_both_paths() {
        // The same attribute with `voice: false` is a music file, and it still
        // carries bytes rather than a spoken message.
        let music = media_document(vec![attribute_audio(false)], false);
        let expected = Some(MediaKind::File);

        assert_eq!(raw(&music.clone()), expected);
        assert_eq!(typed(music), expected);
    }

    #[test]
    fn a_named_document_is_a_file_on_both_paths() {
        let named = media_document(vec![attribute_filename("notes.txt")], false);
        let expected = Some(MediaKind::File);

        assert_eq!(raw(&named.clone()), expected);
        assert_eq!(typed(named), expected);
    }

    #[test]
    fn a_document_with_no_attributes_at_all_is_still_a_file() {
        let bare = media_document(Vec::new(), false);
        let expected = Some(MediaKind::File);

        assert_eq!(raw(&bare.clone()), expected);
        assert_eq!(typed(bare), expected);
    }

    /// A sticker is its own kind on both paths — and `grammers` reads it off the
    /// document's attributes, so the two paths reach the answer from different
    /// places.
    #[test]
    fn a_sticker_is_a_sticker_on_both_paths() {
        let sticker = media_document(vec![attribute_sticker()], false);
        let expected = Some(MediaKind::Sticker);

        assert_eq!(raw(&sticker.clone()), expected);
        assert_eq!(typed(sticker), expected);
    }

    /// An animated sticker is its own kind on both paths. The two classifiers
    /// must agree: the defect this replaces was one path answering a GIF and the
    /// other a file for the same document.
    #[test]
    fn an_animated_sticker_is_an_animated_sticker_on_both_paths() {
        let sticker = media_animated_sticker();
        let expected = Some(MediaKind::AnimatedSticker);

        assert_eq!(raw(&sticker), expected);
        assert_eq!(typed(sticker), expected);
    }

    /// A `WebM` sticker names the video attribute its container needs, and it is
    /// still a sticker: the sticker attribute is the most specific answer, so it
    /// is asked before the video one.
    #[test]
    fn a_webm_sticker_is_an_animated_sticker_not_a_video() {
        let sticker = media_webm_sticker();
        let expected = Some(MediaKind::AnimatedSticker);

        assert_eq!(raw(&sticker), expected);
        assert_eq!(typed(sticker), expected);
    }

    /// The whole reason the classification has a catch-all. A kind this build
    /// has never heard of is still something the message carries, and answering
    /// `None` would make it read as carrying nothing.
    #[test]
    fn media_this_build_does_not_model_is_a_file_not_nothing() {
        let unknown = media_unmodelled();

        assert_eq!(raw(&unknown.clone()), Some(MediaKind::File));
        assert_eq!(typed(unknown), Some(MediaKind::File));
    }

    #[test]
    fn an_empty_media_is_no_media_at_all() {
        assert_eq!(raw(&media_empty()), None);
        assert_eq!(classify_raw(None), None);
        assert_eq!(typed(media_empty()), None);
        assert_eq!(classify_typed(None), None);
    }

    /// The one gap between the two paths, pinned down rather than left as a
    /// surprise: `grammers` will not build some raw media at all, so the feed
    /// path cannot see a kind the history path reports as a file.
    #[test]
    fn a_kind_grammers_refuses_to_build_reaches_the_raw_path_as_a_file() {
        let refused = media_refused_by_grammers();

        assert_eq!(raw(&refused.clone()), Some(MediaKind::File));
        assert_eq!(
            media_typed(refused),
            None,
            "this is the residue the raw path does not have"
        );
    }

    /// The three ways a fetch ends before a byte moves, all of which are one
    /// thing to a reader: this message has no attachment to open. Each is
    /// refused rather than panicked on, which the release profile makes the
    /// only option — it aborts, so nothing here may unwind.
    #[test]
    fn a_message_with_nothing_to_fetch_is_refused_not_panicked_on() {
        assert_eq!(fetchable_media(None), None, "no media field at all");
        assert_eq!(
            fetchable_media(Some(&media_empty())),
            None,
            "and the empty media value is the same answer"
        );
        assert_eq!(
            fetchable_media(Some(&media_refused_by_grammers())),
            None,
            "a kind grammers will not build has no bytes to ask for"
        );
        assert_eq!(
            fetchable_media(Some(&media_unmodelled())),
            None,
            "and neither has one that builds but names no file — a dice reads as \
             a file and still has nothing to fetch, which is the gap between what \
             the description says and what the wire can hand over"
        );
        assert_eq!(
            fetchable_media(Some(&media_photo())),
            None,
            "a photo whose own record the response did not carry names no file — \
             which is the case grammers reports as PreFailed"
        );
    }

    /// The case a download exists for: media that names a file is fetchable,
    /// and it is asked for off the media rather than off the client.
    #[test]
    fn a_document_that_names_a_file_can_be_fetched() {
        let document = media_document(vec![attribute_filename("notes.txt")], false);

        let media = fetchable_media(Some(&document)).expect("a document names a file");

        assert!(
            media.to_raw_input_location().is_some(),
            "the locator is what the request is built from"
        );
    }

    /// The preview a document already carries is read off the media with no
    /// request: the bytes are embedded in the message the read produced.
    #[test]
    fn an_embedded_document_thumbnail_is_read_without_a_request() {
        let document = media_document_with_thumbs(
            vec![attribute_filename("clip.mp4")],
            vec![thumbnail_cached(b"embedded preview")],
        );
        let media = media_typed(document).expect("a document builds");

        assert_eq!(
            embedded_thumbnail(&media),
            Some(b"embedded preview".to_vec())
        );
    }

    /// A photo's preview tree lives in the same place, so it is read the same
    /// way — the accessor is not a document-only path.
    #[test]
    fn an_embedded_photo_thumbnail_is_read_too() {
        let photo = media_photo_with_sizes(vec![thumbnail_cached(b"photo preview")]);
        let media = media_typed(photo).expect("a photo builds");

        assert_eq!(embedded_thumbnail(&media), Some(b"photo preview".to_vec()));
    }

    /// A preview that names a file is *not* embedded: reading it would be a
    /// request, which is the thing this accessor must never make. It is skipped
    /// rather than fetched.
    #[test]
    fn a_thumbnail_that_names_a_file_is_not_read() {
        let document = media_document_with_thumbs(
            vec![attribute_filename("clip.mp4")],
            vec![thumbnail_fetchable()],
        );
        let media = media_typed(document).expect("a document builds");

        assert_eq!(
            embedded_thumbnail(&media),
            None,
            "its bytes live behind a locator, so there is nothing embedded to read"
        );
    }

    /// Several embedded sizes, and the largest is the one worth reading: it is
    /// the best this build can show without asking for one.
    #[test]
    fn the_largest_embedded_thumbnail_is_the_one_read() {
        let document = media_document_with_thumbs(
            vec![attribute_filename("clip.mp4")],
            vec![
                thumbnail_cached(b"small"),
                thumbnail_cached(b"the biggest embedded one"),
            ],
        );
        let media = media_typed(document).expect("a document builds");

        assert_eq!(
            embedded_thumbnail(&media),
            Some(b"the biggest embedded one".to_vec())
        );
    }

    /// A sticker is a document on the wire, so its still preview is read too;
    /// and a kind with no thumbnail at all is an ordinary `None`.
    #[test]
    fn a_sticker_reads_its_preview_and_other_kinds_have_none() {
        let sticker = media_document_with_thumbs(
            vec![attribute_sticker()],
            vec![thumbnail_cached(b"sticker preview")],
        );
        let media = media_typed(sticker).expect("a sticker builds");
        assert_eq!(
            embedded_thumbnail(&media),
            Some(b"sticker preview".to_vec())
        );

        let dice = media_typed(media_unmodelled()).expect("a dice builds");
        assert_eq!(embedded_thumbnail(&dice), None);
    }

    /// The bytes reach the sink one chunk at a time, never as one buffer: the
    /// point of the streaming shape is that no single `Vec` holds the payload.
    #[test]
    fn each_chunk_reaches_the_sink_on_its_own() {
        let sizes = [4_usize, 4, 4];
        let mut seen = Vec::new();
        let mut downloaded = 0_usize;

        for &size in &sizes {
            let chunk = vec![0_u8; size];
            let mut sink = |chunk: &[u8], _downloaded: usize, _total: Option<usize>| {
                seen.push(chunk.len());
                true
            };
            feed_chunk(&chunk, &mut downloaded, None, &mut sink, 42, 7).expect("nothing aborts");
        }

        assert_eq!(seen, vec![4, 4, 4], "one call and one chunk per arrival");
        assert_eq!(downloaded, 12, "the running total is the sum of the chunks");
    }

    /// A raw message with `id` and `media`, the page entry `select_media` reads.
    fn message_at(id: i32, media: Option<tl::enums::MessageMedia>) -> tl::enums::Message {
        let mut message = raw_message(media);
        if let tl::enums::Message::Message(message) = &mut message {
            message.id = id;
        }
        message
    }

    /// The page `GetHistory` answers with when the named message is `target`
    /// and its older neighbour is text: the case the old request returned.
    #[test]
    fn the_named_message_is_selected_when_an_older_sibling_comes_first() {
        let page = [
            message_at(9, None),
            message_at(10, Some(media_document(vec![attribute_sticker()], false))),
        ];

        assert!(select_media(&page, 10).is_some());
    }

    #[test]
    fn a_named_message_absent_from_the_page_selects_nothing() {
        let page = [message_at(9, Some(media_document(vec![], false)))];

        assert!(select_media(&page, 10).is_none());
    }

    /// Telegram sends `Empty` for a message that was deleted, and the page
    /// answers with it rather than omitting it, so it must not be selected.
    #[test]
    fn a_page_of_empty_entries_selects_nothing() {
        let empty = tl::enums::Message::Empty(tl::types::MessageEmpty {
            id: 10,
            peer_id: None,
        });

        assert!(select_media(&[empty], 10).is_none());
    }

    /// The id is the guard: a sibling that carries fetchable media is not the
    /// message asked for, so its bytes must never be returned for this id.
    #[test]
    fn a_sibling_with_media_is_not_fetched_for_another_id() {
        let page = [message_at(
            11,
            Some(media_document(vec![attribute_sticker()], false)),
        )];

        assert!(select_media(&page, 10).is_none());
    }

    /// What the sink saw, one `(downloaded, total)` per chunk.
    type Seen = Vec<(usize, Option<usize>)>;

    /// Drives chunk sizes through the streaming seam, recording what the sink
    /// saw. `stop_after` makes the sink answer `false` at that count of chunks.
    /// Returns the running total on success.
    fn run_chunks(
        sizes: &[usize],
        declared: Option<usize>,
        stop_after: Option<usize>,
    ) -> (Result<usize, FrameworkError>, Seen) {
        let mut downloaded = 0_usize;
        let mut seen = Seen::new();

        for &size in sizes {
            let chunk = vec![0_u8; size];
            let mut sink = |_chunk: &[u8], downloaded: usize, total: Option<usize>| {
                seen.push((downloaded, total));
                stop_after.is_none_or(|stop| seen.len() < stop)
            };
            if let Err(error) = feed_chunk(&chunk, &mut downloaded, declared, &mut sink, 42, 7) {
                return (Err(error), seen);
            }
        }

        (Ok(downloaded), seen)
    }

    /// The bar advances by what landed, never backwards, and finishes at the
    /// full length — with the declared total passed through unchanged.
    #[test]
    fn progress_counts_each_chunk_and_carries_the_declared_total() {
        let (result, seen) = run_chunks(&[3, 4, 5], Some(12), None);

        assert_eq!(result.expect("nothing aborted"), 12);
        assert_eq!(
            seen,
            vec![(3, Some(12)), (7, Some(12)), (12, Some(12))],
            "monotone counts, ending at the length, with the total on every call"
        );
    }

    /// A size Telegram did not declare still reports progress; the total is
    /// just unknown, which is the case the byte-count fallback exists for.
    #[test]
    fn progress_reports_an_unknown_total_as_none() {
        let (_, seen) = run_chunks(&[2, 2], None, None);

        assert_eq!(seen, vec![(2, None), (4, None)]);
    }

    /// Abort stops the transfer on the chunk it was asked to, and says it was an
    /// abort rather than a failure.
    #[test]
    fn an_aborted_download_stops_the_transfer_on_the_chunk_it_was_asked_to() {
        let (result, seen) = run_chunks(&[3, 4, 5], Some(12), Some(2));

        assert_eq!(
            seen,
            vec![(3, Some(12)), (7, Some(12))],
            "the third chunk is never handed over once the sink said stop"
        );
        assert!(
            matches!(
                result,
                Err(FrameworkError::DownloadAborted {
                    peer_id: 42,
                    message_id: 7
                })
            ),
            "a stopped transfer is an abort, not a result"
        );
    }
}
