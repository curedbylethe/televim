//! The kitty graphics protocol: the bytes that transmit a decoded sticker and
//! place it over its block.
//!
//! Pure functions over already-decoded RGBA — nothing here writes to a
//! terminal. The loop writes what [`frame`] returns after each draw.

use std::fmt::Write as _;

use crate::app::App;
use crate::sticker::DecodedSticker;

/// Base64 characters per escape chunk. A multiple of four, as the protocol
/// requires for every chunk but the last.
const CHUNK: usize = 4096;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, RFC 4648 §4.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let b = [
            group[0],
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= group.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Transmits `image` under `id` without displaying it: RGBA, 32 bits a pixel,
/// in as many chunks as its base64 needs.
///
/// `q=2` silences the terminal's replies, which would otherwise arrive as
/// input on the next read.
#[must_use]
pub fn transmit(id: u32, image: &DecodedSticker) -> String {
    let data = base64(&image.rgba);
    let mut out = String::with_capacity(data.len() + 64);
    let mut rest = data.as_str();
    let mut first = true;
    loop {
        let take = rest.len().min(CHUNK);
        let (now, later) = rest.split_at(take);
        let more = u8::from(!later.is_empty());
        if first {
            let _ = write!(
                out,
                "\x1b_Ga=t,f=32,s={},v={},i={id},q=2,m={more};{now}\x1b\\",
                image.width, image.height
            );
            first = false;
        } else {
            let _ = write!(out, "\x1b_Gm={more};{now}\x1b\\");
        }
        if later.is_empty() {
            break;
        }
        rest = later;
    }
    out
}

/// Displays the transmitted image `id` with its top-left at the terminal cell
/// `(x, y)`, scaled to `cols` by `rows` cells.
///
/// The cursor is moved there first and `C=1` keeps it where the move left it,
/// so the frame's own cursor state is untouched. Cell coordinates are
/// zero-based; the escape counts from one.
#[must_use]
pub fn place(id: u32, x: u16, y: u16, cols: u16, rows: u16) -> String {
    format!(
        "\x1b[{};{}H\x1b_Ga=p,i={id},c={cols},r={rows},C=1,q=2\x1b\\",
        u32::from(y) + 1,
        u32::from(x) + 1
    )
}

/// Removes every placement on screen. The transmitted data stays, so the next
/// frame can place it again without sending it again.
#[must_use]
pub fn clear_placements() -> String {
    "\x1b_Ga=d,d=a,q=2\x1b\\".to_owned()
}

/// Everything the loop writes after a kitty frame: clear the last frame's
/// placements, then transmit and place each picture the frame drew.
///
/// Image ids are the placement's position plus one, so ids repeat from frame
/// to frame and the terminal holds at most as many pictures as the screen
/// shows. A picture whose bytes have left the cache draws nothing.
#[must_use]
pub fn frame(app: &App) -> String {
    let mut out = clear_placements();
    for (slot, placed) in app.ui.placements.borrow().iter().enumerate() {
        let Some(image) = app.conversation.stickers.get(placed.message_id) else {
            continue;
        };
        let id = u32::try_from(slot + 1).unwrap_or(u32::MAX);
        out.push_str(&transmit(id, image));
        out.push_str(&place(id, placed.x, placed.y, placed.cols, placed.rows));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// A one-pixel picture is one escape, carrying its four bytes in base64.
    #[test]
    fn a_small_picture_is_one_escape() {
        let image = DecodedSticker {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        assert_eq!(
            transmit(7, &image),
            "\x1b_Ga=t,f=32,s=1,v=1,i=7,q=2,m=0;/wAA/w==\x1b\\"
        );
    }

    /// A picture whose base64 runs past one chunk is cut into chunks, the first
    /// carrying the header and `m=1` until the last, which is `m=0`.
    #[test]
    fn a_large_picture_is_chunked_with_more_flags() {
        let image = DecodedSticker {
            width: 1024,
            height: 2,
            rgba: vec![0; 1024 * 2 * 4],
        };
        let out = transmit(3, &image);
        let escapes: Vec<&str> = out.split("\x1b\\").filter(|s| !s.is_empty()).collect();

        assert_eq!(escapes.len(), 3, "8192 bytes is three 4096-char chunks");
        assert!(escapes[0].starts_with("\x1b_Ga=t,f=32,s=1024,v=2,i=3,q=2,m=1;"));
        assert!(escapes[1].starts_with("\x1b_Gm=1;"));
        assert!(escapes[2].starts_with("\x1b_Gm=0;"));
        for chunk in &escapes {
            let payload = chunk.rsplit(';').next().unwrap_or_default();
            assert!(payload.len() <= CHUNK, "no chunk exceeds the limit");
        }
    }

    #[test]
    fn placement_moves_the_cursor_then_places_without_moving_it() {
        assert_eq!(
            place(2, 7, 4, 24, 8),
            "\x1b[5;8H\x1b_Ga=p,i=2,c=24,r=8,C=1,q=2\x1b\\"
        );
    }

    #[test]
    fn clearing_deletes_placements_and_keeps_data() {
        assert_eq!(clear_placements(), "\x1b_Ga=d,d=a,q=2\x1b\\");
    }
}
