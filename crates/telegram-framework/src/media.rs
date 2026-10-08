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
//! [`Client::download_media`] re-reads the message by identifier and hands the
//! bytes over in one piece. The locator is re-derived from the message on the
//! way rather than kept on the description, because a description is rebuilt on
//! every history page and a stale access hash is worse than none.
//!
//! Everything the transfer decides — whether the media can be fetched at all,
//! and whether it fits under [`MEDIA_LIMIT`] — sits in [`Download`] and
//! [`fetchable_media`], free functions over values rather than over a client,
//! so that the refusals are tested on every CI job rather than only against a
//! live account.

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

    /// A sticker.
    Sticker,

    /// Anything else the message carries: a plain document, or a media kind
    /// this build does not model.
    File,
}

/// Classifies the media of a raw `tl` message.
///
/// `None` means the message carries no media at all — the field is absent, or
/// it is [`MessageMedia::Empty`](tl::enums::MessageMedia::Empty). Every other
/// answer is `Some`, including for media this build does not model.
pub(crate) fn classify_raw(media: Option<&tl::enums::MessageMedia>) -> Option<MediaKind> {
    Some(match media? {
        tl::enums::MessageMedia::Empty => return None,
        tl::enums::MessageMedia::Photo(_) => MediaKind::Photo,
        tl::enums::MessageMedia::Document(document) => classify_document(document),
        // A contact, a poll, a location, a web page, a paid post — and anything
        // Telegram adds after this build. All of them are something the message
        // carries, and none of them is nothing.
        _ => MediaKind::File,
    })
}

/// Classifies the media of a `grammers` message.
///
/// Agrees with [`classify_raw`] by construction: a document is decided by one
/// shared function, and the sticker `grammers` reads off a document's
/// attributes is answered here — a static sticker is [`MediaKind::Sticker`],
/// and an animated one stays [`MediaKind::File`]. Everything else `grammers`
/// flattens away or builds that this crate does not read — a contact, a poll,
/// a location, a web page — all land on [`MediaKind::File`], the same answer
/// the raw path gives them.
///
/// The one residue is a kind `grammers` itself declines to build: its
/// `Media::from_raw` returns `None` for a handful of variants, so on this path
/// such a message arrives here as `None`. The raw history path does not have
/// that gap.
pub(crate) fn classify_typed(media: Option<&Media>) -> Option<MediaKind> {
    Some(match media? {
        Media::Photo(_) => MediaKind::Photo,
        Media::Document(document) => classify_document(&document.raw),
        // `grammers` reads a document with a sticker attribute as a sticker, so
        // this is where a sticker arrives: a static one is its own kind, and an
        // animated one stays a file — animated stickers are out of scope, and
        // the raw path answers a sticker that moves as a GIF by attribute
        // order. Everything `grammers` flattens away or builds that this crate
        // does not read — a contact, a poll, a location, a web page, and any
        // kind added after this build — is one arm, deliberately: all of them
        // carry something, and none of them is nothing.
        Media::Sticker(sticker) => {
            if sticker.is_animated() {
                MediaKind::File
            } else {
                MediaKind::Sticker
            }
        }
        _ => MediaKind::File,
    })
}

/// Decides a document's kind from what the document itself says.
///
/// The document's own `voice` flag is Telegram's statement that this is a voice
/// note, so it is asked first. Otherwise the attributes are read in the order
/// they can be told apart in: an animation is a GIF whatever else it carries, a
/// video attribute names a video, and an audio attribute only counts as a voice
/// note when it says so — a music file carries one too, and is a file.
///
/// The sticker attribute is asked about last, deliberately: a sticker carrying
/// another attribute keeps its more specific kind — a sticker that moves was
/// already answered as a GIF above, as any animation is. Only a document that
/// is nothing but a sticker is one.
///
/// Everything else is [`MediaKind::File`], including a document with no
/// attributes at all: it still carries bytes.
fn classify_document(document: &tl::types::MessageMediaDocument) -> MediaKind {
    if document.voice {
        return MediaKind::Voice;
    }

    let attributes = match &document.document {
        Some(tl::enums::Document::Document(document)) => document.attributes.as_slice(),
        Some(tl::enums::Document::Empty(_)) | None => &[],
    };

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

    // Last, so that a sticker carrying another attribute keeps its more
    // specific kind: the arms above already answered a sticker that moves, or
    // one that names a video or a voice note.
    if attributes
        .iter()
        .any(|a| matches!(a, tl::enums::DocumentAttribute::Sticker(_)))
    {
        return MediaKind::Sticker;
    }

    MediaKind::File
}

/// The largest media this crate will fetch into memory in one piece.
///
/// A download here is held whole, and the workspace's budget is 50 MB of RSS
/// (see `docs/memory.md`), so an unbounded `Vec` is a licence to spend most of
/// it on one attachment. Telegram's own media ceiling is far above this, so the
/// refusal is real rather than theoretical — and it is a refusal, not a
/// truncation: a caller that is told a download was too large can say so, and
/// one handed a short file would not.
///
/// The value is not tuned. Streaming and a bounded cache replace this whole
/// shape, and they do not need this number to have been right first.
pub const MEDIA_LIMIT: usize = 16 * 1024 * 1024;

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

/// A download being collected, with the ceiling applied as it fills.
///
/// Its own type rather than a `Vec` and a running length, because the refusal
/// is the interesting half: a download that has already fetched four chunks and
/// is over the limit has to give the bytes back rather than hand on a file that
/// is quietly short.
#[derive(Debug)]
struct Download {
    bytes: Vec<u8>,
    peer_id: i64,
    message_id: i64,
}

impl Download {
    /// Starts an empty download of one message.
    fn new(peer_id: i64, message_id: i64) -> Self {
        Self {
            bytes: Vec::new(),
            peer_id,
            message_id,
        }
    }

    /// Refuses before a chunk is transferred, when Telegram has already said how
    /// big the media is.
    ///
    /// [`DownloadIter::size`](grammers_client::client::DownloadIter) knows the
    /// length for a document, and a download that is going to be refused is
    /// better refused before the first request than on the last.
    fn check_declared(&self, declared: Option<usize>) -> Result<(), FrameworkError> {
        match declared {
            Some(size) if size > MEDIA_LIMIT => Err(FrameworkError::MediaTooLarge {
                peer_id: self.peer_id,
                message_id: self.message_id,
                size,
                limit: MEDIA_LIMIT,
            }),
            _ => Ok(()),
        }
    }

    /// Adds one chunk, or refuses the whole download if it would cross the
    /// ceiling.
    ///
    /// A photo or a sticker reports no size, so this is the only thing standing
    /// between them and an unbounded buffer — which is why it checks the total
    /// rather than the chunk.
    fn push(&mut self, chunk: Vec<u8>) -> Result<(), FrameworkError> {
        let size = self.bytes.len().saturating_add(chunk.len());

        if size > MEDIA_LIMIT {
            return Err(FrameworkError::MediaTooLarge {
                peer_id: self.peer_id,
                message_id: self.message_id,
                size,
                limit: MEDIA_LIMIT,
            });
        }

        self.bytes.extend(chunk);

        Ok(())
    }

    /// The bytes, once the whole transfer has landed.
    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl Client {
    /// Fetches the bytes of the media a message carries.
    ///
    /// `peer_id` is the bare identifier of the conversation, the same number
    /// [`fetch_history`](crate::Client::fetch_history) takes, and the message
    /// must be in that conversation's peer cache.
    ///
    /// # Why the message is fetched again
    ///
    /// Addressing a file takes the `access_hash` Telegram handed out with it,
    /// and [`MessageInfo`] does not carry one: it is rebuilt on every history
    /// page, so any locator kept on it would be stale the moment the window
    /// moved. Re-reading the message costs one request and is always right — and
    /// it keeps this a typed method, so `proto` never builds a request.
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
    /// - [`FrameworkError::MediaTooLarge`] — the media is larger than
    ///   [`MEDIA_LIMIT`], reported rather than truncated.
    /// - [`FrameworkError::Request`] — Telegram refused a request, the
    ///   connection failed, or a chunk could not be decoded.
    pub async fn download_media(
        &self,
        peer_id: i64,
        message_id: i64,
    ) -> Result<Vec<u8>, FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                message_id,
                "a download was asked for in a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        // Telegram numbers messages with an `i32`, so an identifier outside that
        // range names no message — and there is then nothing to fetch, which is
        // the same answer a message with no attachment gets.
        let Ok(offset_id) = i32::try_from(message_id) else {
            tracing::warn!(
                peer_id,
                message_id,
                "a download was asked for a message identifier telegram could not have numbered"
            );
            return Err(FrameworkError::MediaUnavailable {
                peer_id,
                message_id,
            });
        };

        // The raw route `fetch_history` uses, rather than a `grammers` accessor:
        // the media's own record — the photo, the document and its access hash —
        // is only on the raw message, and it is the thing being fetched.
        let request = tl::functions::messages::GetHistory {
            peer: peer.into(),
            offset_id,
            offset_date: 0,
            // Zero in both directions and a page of one: Telegram counts a page
            // from an anchor downwards, so this is exactly the named message and
            // nothing before it.
            add_offset: 0,
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

        let media = raw
            .iter()
            .find_map(|message| match message {
                tl::enums::Message::Message(message) => Some(message.media.as_ref()),
                tl::enums::Message::Empty(_) | tl::enums::Message::Service(_) => None,
            })
            .and_then(fetchable_media);

        let Some(media) = media else {
            tracing::debug!(peer_id, message_id, "the message has no media to fetch");
            return Err(FrameworkError::MediaUnavailable {
                peer_id,
                message_id,
            });
        };

        let mut download = Download::new(peer_id, message_id);
        download.check_declared(media.size())?;

        let mut chunks = self.inner().iter_download(&media);

        while let Some(chunk) = chunks
            .next()
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?
        {
            download.push(chunk)?;
        }

        let bytes = download.into_bytes();

        tracing::debug!(
            peer_id,
            message_id,
            bytes = bytes.len(),
            "downloaded a message's media"
        );

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        attribute_audio, attribute_filename, attribute_sticker, attribute_video, media_document,
        media_empty, media_photo, media_refused_by_grammers, media_typed, media_unmodelled,
    };

    use tl::enums::DocumentAttribute;

    /// Classifies a media value the raw path way. Borrowing keeps the fixture
    /// alive, which is what lets both paths be asserted on one value.
    fn raw(media: &tl::enums::MessageMedia) -> Option<MediaKind> {
        classify_raw(Some(media))
    }

    fn typed(raw: tl::enums::MessageMedia) -> Option<MediaKind> {
        classify_typed(media_typed(raw).as_ref())
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

    /// Animated stickers are out of scope, so one stays a file. `grammers` keys
    /// `animated` off the document's own Animated attribute, which is what the
    /// typed path asks; the raw path answers the same document as a GIF by
    /// attribute order, an animation winning there whatever else the document
    /// carries.
    #[test]
    fn an_animated_sticker_stays_a_file() {
        let animated = media_document(
            vec![attribute_sticker(), DocumentAttribute::Animated],
            false,
        );

        assert_eq!(typed(animated), Some(MediaKind::File));
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

    /// A refusal, not a truncation: the caller has to be able to say the
    /// download was too large rather than write out a file that is quietly
    /// short.
    #[test]
    fn a_declared_size_over_the_ceiling_is_refused_before_any_chunk() {
        let download = Download::new(42, 7);

        assert!(
            download.check_declared(Some(MEDIA_LIMIT)).is_ok(),
            "exactly the limit is within it"
        );
        assert!(
            download.check_declared(None).is_ok(),
            "an unknown size is not a refusal"
        );
        assert!(download.check_declared(Some(0)).is_ok());

        let refused = download
            .check_declared(Some(MEDIA_LIMIT + 1))
            .expect_err("one byte over the limit is over it");

        assert!(
            matches!(
                refused,
                FrameworkError::MediaTooLarge {
                    peer_id: 42,
                    message_id: 7,
                    size,
                    limit,
                } if size == MEDIA_LIMIT + 1 && limit == MEDIA_LIMIT
            ),
            "the caller is told what was too big and what the limit was"
        );
    }

    /// A photo or a sticker reports no size at all, so the ceiling has to be
    /// applied to what has been collected — otherwise an unknown size is the way
    /// round it.
    #[test]
    fn chunks_are_measured_as_they_arrive_not_only_up_front() {
        let mut download = Download::new(42, 7);

        let chunk = vec![0_u8; MEDIA_LIMIT];
        download
            .push(chunk.clone())
            .expect("a chunk up to the limit lands");
        assert_eq!(download.into_bytes().len(), MEDIA_LIMIT);

        let mut download = Download::new(42, 7);
        let refused = download
            .push(vec![0_u8; MEDIA_LIMIT + 1])
            .expect_err("one byte over the limit is over it");

        assert!(
            matches!(
                refused,
                FrameworkError::MediaTooLarge {
                    size,
                    limit,
                    ..
                } if size == MEDIA_LIMIT + 1 && limit == MEDIA_LIMIT
            ),
            "the size reported is the total, not the chunk"
        );
    }

    /// The ceiling is a real number, not a placeholder: Telegram's media is far
    /// larger, so this refuses downloads a client would happily make.
    #[test]
    fn the_ceiling_leaves_room_inside_the_memory_budget() {
        const { assert!(MEDIA_LIMIT < 50 * 1024 * 1024) };
    }
}
