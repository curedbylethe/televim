//! Fixtures shared by the crate's unit tests.
//!
//! These are the values `grammers` builds for itself when it talks to Telegram.
//! Rebuilding them by hand is what lets the wrapper's mapping and bookkeeping
//! code be exercised without a datacenter — and therefore on every CI run,
//! rather than only in the opt-in integration suite.
//!
//! Nothing here reaches a release build: the module is behind `cfg(test)` and,
//! because it names `grammers` types, behind `live` as well.

use grammers_client::client::PasswordToken;
use grammers_client::media::Media;
use grammers_mtsender::{InvocationError, RpcError};

use crate::tl;

/// Builds the RPC error Telegram answers a misused request with.
///
/// `name` is what Telegram sends once the trailing digits have been stripped
/// out, which is why `FLOOD_WAIT_31` arrives as `name: "FLOOD_WAIT"` with
/// `value: Some(31)`.
pub(crate) fn rpc(code: i32, name: &str, value: Option<u32>) -> InvocationError {
    InvocationError::Rpc(RpcError {
        code,
        name: name.to_owned(),
        value,
        caused_by: None,
    })
}

/// Builds the two-factor challenge a password prompt carries.
pub(crate) fn password_token(hint: Option<&str>) -> PasswordToken {
    use tl::enums::{PasswordKdfAlgo, SecurePasswordKdfAlgo};
    use tl::types::account::Password;

    PasswordToken::new(Password {
        has_recovery: false,
        has_secure_values: false,
        has_password: true,
        current_algo: None,
        srp_b: None,
        srp_id: None,
        hint: hint.map(str::to_owned),
        email_unconfirmed_pattern: None,
        new_algo: PasswordKdfAlgo::Unknown,
        new_secure_algo: SecurePasswordKdfAlgo::Unknown,
        secure_random: Vec::new(),
        pending_reset_date: None,
        login_email_pattern: None,
    })
}

/// Builds the media field of a message that carries nothing.
///
/// `MessageMedia::Empty` is a value Telegram actually sends, not an absence —
/// which is why it is worth a fixture of its own: the classification has to tell
/// the two apart.
pub(crate) fn media_empty() -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Empty
}

/// Builds a photo's media.
///
/// The photo itself is left out. Whether a photo is present is the only fact
/// the classification reads, and a real photo brings a thumbnail tree that would
/// say nothing more.
pub(crate) fn media_photo() -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
        spoiler: false,
        live_photo: false,
        photo: None,
        ttl_seconds: None,
        video: None,
    })
}

/// Builds a media kind this build does not model.
///
/// A dice — Telegram's built-in sticker media, sitting in the same enum as the
/// kinds this crate reads and having nothing to do with them. It is the
/// cheapest such kind `grammers` will still build, so the residue it leaves on
/// the typed path is the only one a test can reach without a datacenter.
pub(crate) fn media_unmodelled() -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Dice(tl::types::MessageMediaDice {
        value: 6,
        emoticon: "\u{1F3B2}".to_owned(),
        game_outcome: None,
    })
}

/// Builds a document's media, carrying the given attributes.
///
/// `voice` is Telegram's own flag on the media rather than an attribute, so it
/// is a parameter here; everything else about a document is an attribute.
pub(crate) fn media_document(
    attributes: Vec<tl::enums::DocumentAttribute>,
    voice: bool,
) -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Document(tl::types::MessageMediaDocument {
        nopremium: false,
        spoiler: false,
        video: false,
        round: false,
        voice,
        document: Some(tl::enums::Document::Document(tl::types::Document {
            id: 1,
            access_hash: 1,
            file_reference: Vec::new(),
            date: 0,
            mime_type: String::new(),
            size: 0,
            thumbs: None,
            video_thumbs: None,
            dc_id: 2,
            attributes,
        })),
        alt_documents: None,
        video_cover: None,
        video_timestamp: None,
        ttl_seconds: None,
    })
}

/// Builds a document's media whose record carries `thumbs`.
///
/// The thumbnail tree is what the embedded-thumbnail accessor reads, so a
/// fixture has to be able to say what a document ships with.
pub(crate) fn media_document_with_thumbs(
    attributes: Vec<tl::enums::DocumentAttribute>,
    thumbs: Vec<tl::enums::PhotoSize>,
) -> tl::enums::MessageMedia {
    let mut media = media_document(attributes, false);
    if let tl::enums::MessageMedia::Document(message) = &mut media
        && let Some(tl::enums::Document::Document(record)) = &mut message.document
    {
        record.thumbs = Some(thumbs);
    }
    media
}

/// Builds a photo's media whose record carries `sizes`.
///
/// A photo's preview tree lives in the same place a document's does — the same
/// `PhotoSize` values — so the accessor reads both the same way.
pub(crate) fn media_photo_with_sizes(sizes: Vec<tl::enums::PhotoSize>) -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
        spoiler: false,
        live_photo: false,
        photo: Some(tl::enums::Photo::Photo(tl::types::Photo {
            has_stickers: false,
            id: 88,
            access_hash: 1,
            file_reference: Vec::new(),
            date: 0,
            sizes,
            video_sizes: None,
            dc_id: 2,
        })),
        ttl_seconds: None,
        video: None,
    })
}

/// Builds an embedded thumbnail: `photoCachedSize`, whose bytes ride on the
/// message, so reading it is no request.
pub(crate) fn thumbnail_cached(bytes: &[u8]) -> tl::enums::PhotoSize {
    tl::enums::PhotoSize::PhotoCachedSize(tl::types::PhotoCachedSize {
        r#type: "m".to_owned(),
        w: 320,
        h: 320,
        bytes: bytes.to_vec(),
    })
}

/// Builds a thumbnail that names a file instead: `photoSize`, whose bytes live
/// behind a locator, so reading it *is* a request.
pub(crate) fn thumbnail_fetchable() -> tl::enums::PhotoSize {
    tl::enums::PhotoSize::Size(tl::types::PhotoSize {
        r#type: "m".to_owned(),
        w: 320,
        h: 320,
        size: 4096,
    })
}

/// Builds an audio document's attribute.
///
/// `voice` is what separates a voice note from a music file, and the two share
/// this one attribute — so a fixture has to be able to say which it is.
pub(crate) fn attribute_audio(voice: bool) -> tl::enums::DocumentAttribute {
    tl::enums::DocumentAttribute::Audio(tl::types::DocumentAttributeAudio {
        voice,
        duration: 1,
        title: None,
        performer: None,
        waveform: None,
    })
}

/// Builds a video document's attribute.
pub(crate) fn attribute_video() -> tl::enums::DocumentAttribute {
    tl::enums::DocumentAttribute::Video(tl::types::DocumentAttributeVideo {
        round_message: false,
        supports_streaming: true,
        nosound: false,
        duration: 1.0,
        w: 16,
        h: 16,
        preload_prefix_size: None,
        video_start_ts: None,
        video_codec: None,
    })
}

/// Builds a file name attribute — the one a plain document carries.
pub(crate) fn attribute_filename(name: &str) -> tl::enums::DocumentAttribute {
    tl::enums::DocumentAttribute::Filename(tl::types::DocumentAttributeFilename {
        file_name: name.to_owned(),
    })
}

/// Builds the sticker attribute, which is what makes `grammers` read a document
/// as a sticker rather than as a document.
pub(crate) fn attribute_sticker() -> tl::enums::DocumentAttribute {
    tl::enums::DocumentAttribute::Sticker(tl::types::DocumentAttributeSticker {
        mask: false,
        alt: String::new(),
        stickerset: tl::enums::InputStickerSet::Empty,
        mask_coords: None,
    })
}

/// Builds a document's media whose record carries `mime`.
///
/// The mime is what splits an animated sticker from a static one; every other
/// kind is read from attributes, but the wire says what the file is, and a
/// fixture that names the format is the one a later reader will recognise.
pub(crate) fn media_document_with_mime(
    attributes: Vec<tl::enums::DocumentAttribute>,
    voice: bool,
    mime: &str,
) -> tl::enums::MessageMedia {
    let mut media = media_document(attributes, voice);
    if let tl::enums::MessageMedia::Document(message) = &mut media
        && let Some(tl::enums::Document::Document(record)) = &mut message.document
    {
        mime.clone_into(&mut record.mime_type);
    }
    media
}

/// Builds an animated sticker as Telegram sends a TGS: the sticker attribute and
/// the TGS mime, with no animation attribute — the mime is what makes it one.
pub(crate) fn media_animated_sticker() -> tl::enums::MessageMedia {
    media_document_with_mime(
        vec![attribute_sticker()],
        false,
        "application/x-tgsticker",
    )
}

/// Builds an animated sticker in the other format Telegram sends: a `WebM`, which
/// also carries the video attribute its container needs.
pub(crate) fn media_webm_sticker() -> tl::enums::MessageMedia {
    media_document_with_mime(
        vec![
            attribute_sticker(),
            tl::enums::DocumentAttribute::Animated,
            attribute_video(),
        ],
        false,
        "video/webm",
    )
}

/// Builds a media value `grammers` declines to build at all.
///
/// `grammers` refuses a handful of raw media kinds — a game, an invoice, a
/// story, a giveaway — and hands back `None` for them, so on the typed feed
/// path such a message reaches the classification as no media at all. That
/// residue is the one gap the raw history path does not have.
pub(crate) fn media_refused_by_grammers() -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Unsupported
}

/// Builds the `grammers` form of a raw media value.
pub(crate) fn media_typed(raw: tl::enums::MessageMedia) -> Option<Media> {
    Media::from_raw(raw)
}

/// Builds a raw message, as `GetHistory` returns one.
///
/// The field list is `tl`'s, and it is long because the type is generated with
/// no `Default` to lean on. Only the fields a test reads are worth setting.
pub(crate) fn raw_message(media: Option<tl::enums::MessageMedia>) -> tl::enums::Message {
    tl::enums::Message::Message(tl::types::Message {
        out: false,
        mentioned: false,
        media_unread: false,
        silent: false,
        post: false,
        from_scheduled: false,
        legacy: false,
        edit_hide: false,
        pinned: false,
        noforwards: false,
        invert_media: false,
        offline: false,
        video_processing_pending: false,
        paid_suggested_post_stars: false,
        paid_suggested_post_ton: false,
        id: 1,
        from_id: None,
        from_boosts_applied: None,
        from_rank: None,
        peer_id: tl::enums::Peer::User(tl::types::PeerUser { user_id: 42 }),
        saved_peer_id: None,
        fwd_from: None,
        via_bot_id: None,
        via_business_bot_id: None,
        guestchat_via_from: None,
        reply_to: None,
        date: 1_700_000_000,
        message: String::new(),
        media,
        reply_markup: None,
        entities: None,
        views: None,
        forwards: None,
        replies: None,
        edit_date: None,
        post_author: None,
        grouped_id: None,
        reactions: None,
        restriction_reason: None,
        ttl_period: None,
        quick_reply_shortcut_id: None,
        effect: None,
        factcheck: None,
        report_delivery_until_date: None,
        paid_message_stars: None,
        suggested_post: None,
        schedule_repeat_period: None,
        summary_from_language: None,
        rich_message: None,
    })
}

/// A full user with the given name, handle and bot flag.
///
/// The field list is `tl`'s and it is long because the type is generated
/// with no `Default` to lean on; only the fields a test reads are worth
/// setting.
pub(crate) fn user(
    id: i64,
    first: Option<&str>,
    last: Option<&str>,
    username: Option<&str>,
    bot: bool,
) -> tl::enums::User {
    tl::enums::User::User(tl::types::User {
        is_self: false,
        contact: false,
        mutual_contact: false,
        deleted: false,
        bot,
        bot_chat_history: false,
        bot_nochats: false,
        verified: false,
        restricted: false,
        min: false,
        bot_inline_geo: false,
        support: false,
        scam: false,
        apply_min_photo: false,
        fake: false,
        bot_attach_menu: false,
        premium: false,
        attach_menu_enabled: false,
        bot_can_edit: false,
        close_friend: false,
        stories_hidden: false,
        stories_unavailable: false,
        contact_require_premium: false,
        bot_business: false,
        bot_has_main_app: false,
        bot_forum_view: false,
        bot_forum_can_manage_topics: false,
        bot_can_manage_bots: false,
        bot_guestchat: false,
        bot_guard: false,
        id,
        access_hash: Some(7),
        first_name: first.map(str::to_owned),
        last_name: last.map(str::to_owned),
        username: username.map(str::to_owned),
        phone: None,
        photo: None,
        status: None,
        bot_info_version: None,
        restriction_reason: None,
        bot_inline_placeholder: None,
        lang_code: None,
        emoji_status: None,
        usernames: None,
        stories_max_id: None,
        color: None,
        profile_color: None,
        bot_active_users: None,
        bot_verification_icon: None,
        send_paid_messages_stars: None,
    })
}
