//! What a message carries, read off the wire.
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

use grammers_client::media::Media;

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

    /// Anything else the message carries: a plain document, a sticker, or a
    /// media kind this build does not model.
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
        // A sticker, a contact, a poll, a location, a web page, a paid post —
        // and anything Telegram adds after this build. All of them are
        // something the message carries, and none of them is nothing.
        _ => MediaKind::File,
    })
}

/// Classifies the media of a `grammers` message.
///
/// Agrees with [`classify_raw`] by construction: a document is decided by one
/// shared function, and the variants `grammers` already flattens — a sticker, a
/// contact, a poll, a location, a web page — all land on [`MediaKind::File`],
/// the same answer the raw path gives them.
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
        // this is where a sticker arrives — answered `File` here, as the raw
        // path answers the document behind it. Everything `grammers` flattens
        // away or builds that this crate does not read — a sticker, a contact,
        // a poll, a location, a web page, and any kind added after this build
        // — is one arm, deliberately: all of them carry something, and none of
        // them is nothing.
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

    MediaKind::File
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

    /// Sticker media is CUR-13, so it is a file rather than a kind of its own
    /// — and `grammers` reads it off the document's attributes, so the two
    /// paths reach the answer from different places.
    #[test]
    fn a_sticker_is_a_file_on_both_paths() {
        let sticker = media_document(vec![attribute_sticker()], false);
        let expected = Some(MediaKind::File);

        assert_eq!(raw(&sticker.clone()), expected);
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
}
