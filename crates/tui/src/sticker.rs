//! Static sticker bytes, decoded to a bounded RGBA buffer the panel can paint.
//!
//! The decode lives here — in `tui`, the rendering crate — and never in
//! `domain`: an attachment on a message is a `Copy` flag with no payload, and
//! the bytes behind it are fetched and decoded at paint time. Fetching reuses
//! `ProtoClient::download_media` unchanged — the locator is re-derived from
//! the identifiers at request time — and what arrives is handed to
//! [`StickerCache::insert_bytes`]; a message with no cached image draws
//! `[sticker]` that frame, exactly like a captionless `[image]`.
//!
//! The fit box is DESIGN.md's "Media placeholders": 24 columns by 8 rows, each
//! terminal row carrying two picture rows as half-block cells, so a decoded
//! sticker is at most 24 by 16 pixels. Scale-to-fit keeps the shape and a
//! smaller image is never stretched up.
//!
//! Decoding is synchronous on the event loop — no `spawn_blocking`, no threads
//! — and hostile bytes are an error, never a panic: the header's dimensions
//! are clamped with checked arithmetic before anything is allocated, and the
//! decoder itself gets the same ceiling.
//!
//! [`ProtoClient::download_media`]: the call itself lives with the caller —
//! `tui` must not depend on `proto` (see the crate docs) — and only its
//! outcome crosses here, through [`resolve_fetch`].

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::io::Cursor;

use image_webp::WebPDecoder;

/// Columns of the sticker fit box: the picture is set left-aligned in the body
/// column and never wider than this.
pub const STICKER_FIT_WIDTH: u32 = 24;

/// Rows of the sticker fit box, in terminal rows.
pub const STICKER_FIT_ROWS: u32 = 8;

/// Picture rows of the fit box: each terminal row carries two picture rows as
/// half-block cells.
pub const STICKER_FIT_HEIGHT: u32 = STICKER_FIT_ROWS * 2;

/// The largest decode this module will allocate: a 512-pixel square in RGBA.
///
/// Telegram sends stickers at most 512 pixels on a side, so anything larger is
/// refused before a byte of it is held — 16 MiB of WEBP is not 16 MiB of RGBA.
/// The workspace budget is 50 MB of RSS (see `docs/memory.md`); this ceiling
/// is one fiftieth of it.
pub const MAX_STICKER_DECODE_BYTES: usize = 1024 * 1024;

const _: () = assert!(MAX_STICKER_DECODE_BYTES < 50 * 1024 * 1024);

/// Decoded stickers the open conversation holds, keyed by message identifier.
///
/// Bounded twice: each image is fitted to the 24-by-16 box on the way in, and
/// the whole cache evicts oldest-first under [`STICKER_CACHE_MAX_BYTES`].
/// Cleared on conversation switch — identifiers repeat across conversations,
/// so an entry belongs to the chat on show.
pub const STICKER_CACHE_MAX_BYTES: usize = 32 * 1024;

const _: () = assert!(STICKER_CACHE_MAX_BYTES >= 24 * 16 * 4);
const _: () = assert!(STICKER_CACHE_MAX_BYTES < 1024 * 1024);

/// A sticker decoded and fitted to the paint box: row-major RGBA, `width` by
/// `height` pixels, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSticker {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Why sticker bytes did not become a picture.
#[derive(Debug)]
pub enum StickerError {
    /// The header names dimensions whose RGBA would not fit under
    /// [`MAX_STICKER_DECODE_BYTES`], or whose product does not fit in memory
    /// at all. Refused before allocating.
    TooLarge { width: u32, height: u32 },
    /// An animated image: out of scope (animated stickers stay `File`), so it
    /// decodes to nothing and the message keeps its `[sticker]` token.
    Animated,
    /// A header that parses but names no picture, or a buffer size the decoder
    /// will not stand behind.
    Malformed,
    /// The decoder's own refusal: bad signature, truncation, corrupt chunks.
    Decode(image_webp::DecodingError),
}

impl fmt::Display for StickerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { width, height } => {
                write!(
                    f,
                    "a {width}x{height} sticker would not fit the decode ceiling"
                )
            }
            Self::Animated => write!(f, "an animated sticker is not decoded"),
            Self::Malformed => write!(f, "a sticker header names no picture"),
            Self::Decode(error) => write!(f, "a sticker would not decode: {error}"),
        }
    }
}

impl std::error::Error for StickerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::TooLarge { .. } | Self::Animated | Self::Malformed => None,
        }
    }
}

impl From<image_webp::DecodingError> for StickerError {
    fn from(error: image_webp::DecodingError) -> Self {
        Self::Decode(error)
    }
}

/// The RGBA byte count of a `width` by `height` picture, refused when it would
/// not fit under [`MAX_STICKER_DECODE_BYTES`].
///
/// Checked arithmetic throughout: a hostile header names absurd dimensions,
/// and the product must not wrap on the way to the comparison.
fn decoded_byte_size(width: u32, height: u32) -> Result<usize, StickerError> {
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4));

    let Some(bytes) = bytes else {
        return Err(StickerError::TooLarge { width, height });
    };

    usize::try_from(bytes)
        .ok()
        .filter(|bytes| *bytes <= MAX_STICKER_DECODE_BYTES)
        .ok_or(StickerError::TooLarge { width, height })
}

/// The dimensions a `width` by `height` picture paints at: scaled to fit the
/// 24-by-16 box keeping its shape, and never scaled up.
///
/// Integer math on the cross products, so the tighter side wins exactly; each
/// side is at least one pixel, so a panoramic sticker is a line and never
/// nothing. `width` and `height` must both be nonzero.
fn fit_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width <= STICKER_FIT_WIDTH && height <= STICKER_FIT_HEIGHT {
        return (width, height);
    }

    let wide = u64::from(width) * u64::from(STICKER_FIT_HEIGHT);
    let tall = u64::from(height) * u64::from(STICKER_FIT_WIDTH);

    if wide > tall {
        let height = (u64::from(height) * u64::from(STICKER_FIT_WIDTH) / u64::from(width))
            .max(1)
            .min(u64::from(STICKER_FIT_HEIGHT));
        (
            STICKER_FIT_WIDTH,
            u32::try_from(height).expect("a fitted side fits the box"),
        )
    } else {
        let width = (u64::from(width) * u64::from(STICKER_FIT_HEIGHT) / u64::from(height))
            .max(1)
            .min(u64::from(STICKER_FIT_WIDTH));
        (
            u32::try_from(width).expect("a fitted side fits the box"),
            STICKER_FIT_HEIGHT,
        )
    }
}

/// Nearest-neighbour resample of RGBA `src` (`width` by `height`) to
/// (`out_width` by `out_height`).
///
/// The box is 24 by 16 at most, so each output pixel reading one input pixel
/// is exact enough — and there is no resampling dependency to carry for it.
fn resample_rgba(src: &[u8], width: u32, height: u32, out_width: u32, out_height: u32) -> Vec<u8> {
    let width = u64::from(width);
    let height = u64::from(height);
    let out_width = u64::from(out_width);
    let out_height = u64::from(out_height);

    let mut out = Vec::with_capacity((out_width * out_height * 4) as usize);
    for oy in 0..out_height {
        let sy = oy * height / out_height;
        for ox in 0..out_width {
            let sx = ox * width / out_width;
            let at = (sy * width + sx) * 4;
            out.extend_from_slice(&src[at as usize..at as usize + 4]);
        }
    }
    out
}

/// Decodes static WEBP `bytes` to a fitted RGBA buffer.
///
/// Hostile input is `Err`, never a panic: absurd dimensions are refused before
/// allocating, and truncation or corruption is the decoder's own error. An
/// animated image is refused outright — it stays a file, decoded nowhere.
pub fn decode_sticker(bytes: &[u8]) -> Result<DecodedSticker, StickerError> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(StickerError::Decode)?;
    let (width, height) = decoder.dimensions();

    // Clamp before allocating: the product is checked, and the decoder gets
    // the same ceiling for whatever it holds while reading.
    let _fits = decoded_byte_size(width, height)?;
    decoder.set_memory_limit(MAX_STICKER_DECODE_BYTES);

    if decoder.is_animated() {
        return Err(StickerError::Animated);
    }
    if width == 0 || height == 0 {
        return Err(StickerError::Malformed);
    }

    let size = decoder
        .output_buffer_size()
        .filter(|size| *size <= MAX_STICKER_DECODE_BYTES)
        .ok_or(StickerError::Malformed)?;
    let mut buf = vec![0_u8; size];
    decoder.read_image(&mut buf)?;

    // RGBA when the picture carries alpha, RGB when it does not; the panel
    // paints RGBA either way, so an opaque alpha is filled in.
    let rgba = if decoder.has_alpha() {
        buf
    } else {
        let mut rgba = Vec::with_capacity(buf.len() / 3 * 4);
        for pixel in buf.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        rgba
    };

    let (out_width, out_height) = fit_dimensions(width, height);
    if out_width == width && out_height == height {
        Ok(DecodedSticker {
            width,
            height,
            rgba,
        })
    } else {
        let rgba = resample_rgba(&rgba, width, height, out_width, out_height);
        Ok(DecodedSticker {
            width: out_width,
            height: out_height,
            rgba,
        })
    }
}

#[derive(Debug)]
struct CacheEntry {
    message_id: i64,
    image: DecodedSticker,
}

/// The open conversation's decoded stickers. See [`STICKER_CACHE_MAX_BYTES`].
///
/// The request queue beside it is what the panel asks for: rendering is a
/// shared borrow, so a miss cannot fetch — it records `(chat_id,
/// message_id)` here instead, and the drain (which owns the network) takes the
/// batch with [`StickerCache::take_pending`] and settles each outcome through
/// [`resolve_fetch`]. A request is recorded once: while it waits in either
/// queue, further frames do not repeat it.
#[derive(Debug, Default)]
pub struct StickerCache {
    entries: VecDeque<CacheEntry>,
    bytes: usize,
    pending: RefCell<Vec<(i64, i64)>>,
    in_flight: RefCell<Vec<(i64, i64)>>,
}

impl StickerCache {
    /// Looks up the decoded image for `message_id`, if it is cached.
    #[must_use]
    pub fn get(&self, message_id: i64) -> Option<&DecodedSticker> {
        self.entries
            .iter()
            .find(|entry| entry.message_id == message_id)
            .map(|entry| &entry.image)
    }

    /// Decodes `bytes` and caches the picture under `message_id`.
    ///
    /// A previous picture for the same message is replaced; anything that has
    /// to go to stay under the byte cap is evicted oldest-first. A picture
    /// that fails to decode is neither cached nor kept: the message draws
    /// `[sticker]` that frame.
    pub fn insert_bytes(&mut self, message_id: i64, bytes: &[u8]) -> Result<(), StickerError> {
        let image = decode_sticker(bytes)?;
        self.insert_decoded(message_id, image);
        Ok(())
    }

    fn insert_decoded(&mut self, message_id: i64, image: DecodedSticker) {
        if let Some(at) = self
            .entries
            .iter()
            .position(|entry| entry.message_id == message_id)
            && let Some(old) = self.entries.remove(at)
        {
            self.bytes = self.bytes.saturating_sub(old.image.rgba.len());
        }

        // The fit box bounds every picture at 24 by 16 RGBA, far under the
        // cap, so eviction always makes room: one picture always fits.
        while !self.entries.is_empty() && self.bytes + image.rgba.len() > STICKER_CACHE_MAX_BYTES {
            if let Some(old) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.image.rgba.len());
            }
        }

        self.bytes += image.rgba.len();
        self.entries.push_back(CacheEntry { message_id, image });
    }

    /// Forgets every cached picture. Called on conversation switch: entries
    /// belong to the chat on show.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.pending.borrow_mut().clear();
        self.in_flight.borrow_mut().clear();
    }

    /// How many pictures are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Cached bytes, always at or under [`STICKER_CACHE_MAX_BYTES`].
    #[must_use]
    pub fn bytes_used(&self) -> usize {
        self.bytes
    }

    /// Asks for the bytes of `message_id` in `chat_id`.
    ///
    /// A no-op when the picture is cached or already asked for: the panel
    /// calls this on every miss of every frame, and without the guard one
    /// visible sticker would be a download per frame.
    pub fn request(&self, chat_id: i64, message_id: i64) {
        if self.get(message_id).is_some() {
            return;
        }
        let key = (chat_id, message_id);
        if self.in_flight.borrow().contains(&key) {
            return;
        }
        let mut pending = self.pending.borrow_mut();
        if !pending.contains(&key) {
            pending.push(key);
        }
    }

    /// Takes the requested batch for the drain, marking each in flight.
    ///
    /// The drain owns the network: it downloads each pair and settles the
    /// outcome through [`resolve_fetch`], which releases the in-flight mark.
    #[must_use]
    pub fn take_pending(&self) -> Vec<(i64, i64)> {
        let batch = std::mem::take(&mut *self.pending.borrow_mut());
        self.in_flight.borrow_mut().extend(batch.iter().copied());
        batch
    }
}

/// Settles a sticker fetch into the cache: the outcome of the download the
/// caller ran — `ProtoClient::download_media` itself, unchanged, called with
/// the chat and message identifiers — decoded and stored, or logged and
/// dropped.
///
/// Fetch errors log via `tracing`, which the binary routes to the log file
/// beside the configuration, never the terminal: a failed sticker must not
/// take the interface apart. Reports whether the message has a picture now —
/// on `false` the message draws `[sticker]` that frame.
pub fn resolve_fetch<E: fmt::Display>(
    cache: &mut StickerCache,
    chat_id: i64,
    message_id: i64,
    fetched: Result<Vec<u8>, E>,
) -> bool {
    cache
        .in_flight
        .borrow_mut()
        .retain(|flight| *flight != (chat_id, message_id));

    match fetched {
        Ok(bytes) => match cache.insert_bytes(message_id, &bytes) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    chat_id,
                    message_id,
                    bytes = bytes.len(),
                    ?error,
                    "a sticker's bytes would not decode"
                );
                false
            }
        },
        Err(error) => {
            tracing::warn!(chat_id, message_id, %error, "a sticker fetch failed");
            false
        }
    }
}

/// Test bytes shared across the crate: a 4-by-3 lossless WEBP with twelve
/// distinct pixels (see the decode test for the layout). Encoded once with
/// `image-webp`'s own lossless encoder and embedded, so widget and geometry
/// tests run without a datacenter or a network.
#[cfg(test)]
pub(crate) const STICKER_TEST_WEBP: &[u8] = &[
    82, 73, 70, 70, 204, 0, 0, 0, 87, 69, 66, 80, 86, 80, 56, 76, 192, 0, 0, 0, 47, 3, 128, 0, 16,
    205, 85, 32, 34, 2, 30, 136, 32, 0, 0, 0, 0, 128, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 6, 0, 0, 0, 0, 0, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 12, 0, 0, 224, 129, 64, 27, 0, 0, 0, 0,
    156, 127, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 32, 15, 4, 18, 0, 0, 0, 0, 224, 252, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 156, 7, 34, 1, 0, 0, 0, 0, 112, 254, 1,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0,
    72, 225, 175, 116, 71, 14, 136, 8, 170, 19, 211, 249, 8,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared crate fixture (see above for the pixel layout).
    const FIXTURE_WEBP: &[u8] = STICKER_TEST_WEBP;

    fn pixel(image: &DecodedSticker, x: u32, y: u32) -> [u8; 4] {
        let at = (u64::from(y) * u64::from(image.width) + u64::from(x)) as usize * 4;
        [
            image.rgba[at],
            image.rgba[at + 1],
            image.rgba[at + 2],
            image.rgba[at + 3],
        ]
    }

    #[test]
    fn a_known_webp_decodes_to_its_pixels() {
        let image = decode_sticker(FIXTURE_WEBP).expect("the fixture is valid webp");

        assert_eq!((image.width, image.height), (4, 3));
        assert_eq!(image.rgba.len(), 4 * 3 * 4);
        assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 2, 1), [255, 0, 255, 128]);
        assert_eq!(pixel(&image, 2, 2), [0, 128, 128, 64]);
    }

    /// Garbage in is an error out — and the release profile aborts on panic,
    /// so reaching the error is the whole of this test.
    #[test]
    fn truncated_and_garbage_bytes_are_an_error_not_a_panic() {
        assert!(decode_sticker(&[]).is_err(), "empty input");
        assert!(
            decode_sticker(&[0xDE, 0xAD, 0xBE, 0xEF]).is_err(),
            "four junk bytes"
        );
        assert!(
            decode_sticker(&[82, 73, 70, 70, 0, 0, 0, 0]).is_err(),
            "a bare RIFF head with no picture"
        );
        assert!(
            decode_sticker(&FIXTURE_WEBP[..40]).is_err(),
            "a truncated sticker"
        );
        assert!(
            decode_sticker(&FIXTURE_WEBP[..FIXTURE_WEBP.len() - 1]).is_err(),
            "a sticker missing its last byte"
        );
    }

    #[test]
    fn absurd_dimensions_are_refused_before_any_allocation() {
        assert!(
            decoded_byte_size(u32::MAX, u32::MAX).is_err(),
            "the product must not wrap past the check"
        );
        assert!(
            decoded_byte_size(u32::MAX, 1).is_err(),
            "one absurd side is enough"
        );
        assert!(
            decoded_byte_size(512, 512).is_ok(),
            "a full-size sticker decodes"
        );
        assert_eq!(
            decoded_byte_size(512, 512).expect("a full-size sticker decodes"),
            512 * 512 * 4
        );
    }

    #[test]
    fn fitting_keeps_shape_never_upscales_and_never_vanishes() {
        assert_eq!(fit_dimensions(4, 3), (4, 3), "small pictures are untouched");
        assert_eq!(
            fit_dimensions(24, 16),
            (24, 16),
            "exactly the box is untouched"
        );
        assert_eq!(fit_dimensions(512, 512), (16, 16), "a square fits the rows");
        assert_eq!(
            fit_dimensions(100, 10),
            (24, 2),
            "a panorama is a line, keeping its shape"
        );
        assert_eq!(
            fit_dimensions(10, 100),
            (1, 16),
            "a column keeps its shape too"
        );
    }

    #[test]
    fn a_wide_sticker_arrives_fitted() {
        // A 48-by-8 picture: wider than the box, shorter than it.
        let mut rgba = vec![0_u8; 48 * 8 * 4];
        for (i, byte) in rgba.iter_mut().enumerate() {
            *byte = (i % 251) as u8;
        }

        let fitted = resample_rgba(
            &rgba,
            48,
            8,
            fit_dimensions(48, 8).0,
            fit_dimensions(48, 8).1,
        );

        assert_eq!(fit_dimensions(48, 8), (24, 4));
        assert_eq!(fitted.len(), 24 * 4 * 4);
    }

    #[test]
    fn the_cache_holds_what_it_was_given() {
        let mut cache = StickerCache::default();

        cache
            .insert_bytes(7, FIXTURE_WEBP)
            .expect("the fixture decodes");

        let image = cache.get(7).expect("a stored sticker is found");
        assert_eq!((image.width, image.height), (4, 3));
        assert!(cache.get(8).is_none(), "another message is not");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes_used(), 4 * 3 * 4);
    }

    #[test]
    fn undecodable_bytes_are_not_cached() {
        let mut cache = StickerCache::default();

        assert!(cache.insert_bytes(7, &[]).is_err());
        assert!(cache.get(7).is_none(), "a failure stores nothing");
        assert!(cache.is_empty());
    }

    #[test]
    fn the_cache_evicts_oldest_first_under_its_cap() {
        let mut cache = StickerCache::default();

        let pictures =
            i64::try_from(STICKER_CACHE_MAX_BYTES / (4 * 3 * 4) + 4).expect("a few hundred fits");
        for id in 0..pictures {
            cache
                .insert_bytes(id, FIXTURE_WEBP)
                .expect("the fixture decodes");
        }

        assert!(
            cache.bytes_used() <= STICKER_CACHE_MAX_BYTES,
            "the cap holds: {} bytes used",
            cache.bytes_used()
        );
        assert!(cache.get(0).is_none(), "the oldest picture went first");
        assert!(
            cache.get(pictures - 1).is_some(),
            "the newest picture stays"
        );
    }

    #[test]
    fn storing_again_replaces_and_clearing_forgets() {
        let mut cache = StickerCache::default();
        cache
            .insert_bytes(7, FIXTURE_WEBP)
            .expect("the fixture decodes");

        cache
            .insert_bytes(7, FIXTURE_WEBP)
            .expect("the fixture decodes");
        assert_eq!(cache.len(), 1, "a message holds one picture");
        assert_eq!(
            cache.bytes_used(),
            4 * 3 * 4,
            "replacing does not double-count"
        );

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes_used(), 0);
    }

    #[test]
    fn a_settled_fetch_reports_whether_there_is_a_picture_now() {
        let mut cache = StickerCache::default();

        assert!(resolve_fetch::<&str>(
            &mut cache,
            42,
            7,
            Ok(FIXTURE_WEBP.to_vec())
        ));
        assert!(cache.get(7).is_some());

        assert!(
            !resolve_fetch::<&str>(&mut cache, 42, 8, Err("the peer is gone")),
            "a failed download stores nothing"
        );
        assert!(cache.get(8).is_none());

        assert!(
            !resolve_fetch::<&str>(&mut cache, 42, 9, Ok(Vec::new())),
            "undecodable bytes store nothing either"
        );
        assert!(cache.get(9).is_none());
    }

    #[test]
    fn a_miss_is_requested_once_and_released_on_settle() {
        let mut cache = StickerCache::default();

        cache.request(42, 7);
        cache.request(42, 7);
        cache.request(42, 8);
        assert_eq!(
            cache.take_pending(),
            vec![(42, 7), (42, 8)],
            "one request per message, in order"
        );

        cache.request(42, 9);
        assert_eq!(
            cache.take_pending(),
            vec![(42, 9)],
            "a taken batch does not come back"
        );
        cache.request(42, 7);
        assert!(
            cache.take_pending().is_empty(),
            "a message in flight is not asked for again"
        );

        assert!(resolve_fetch::<&str>(
            &mut cache,
            42,
            7,
            Ok(FIXTURE_WEBP.to_vec())
        ));
        cache.request(42, 7);
        assert!(
            cache.take_pending().is_empty(),
            "and neither is one that has settled into a picture"
        );
    }

    #[test]
    fn clearing_forgets_requests_with_the_pictures() {
        let cache = StickerCache::default();
        cache.request(42, 7);

        let mut cleared = cache;
        cleared.clear();
        cleared.request(42, 7);
        assert_eq!(
            cleared.take_pending(),
            vec![(42, 7)],
            "a switch drops the old chat's queue with its pictures"
        );
    }

    /// The ceilings are real numbers, not placeholders: the decode ceiling is
    /// a fiftieth of the RSS budget, and the cache holds kilobytes.
    #[test]
    fn the_ceilings_leave_room_inside_the_memory_budget() {
        const { assert!(MAX_STICKER_DECODE_BYTES * 50 <= 50 * 1024 * 1024) };
        const { assert!(STICKER_CACHE_MAX_BYTES <= 64 * 1024) };
    }
}
