use std::borrow::Cow;

use domain::message::{MediaKind, Message, MessageStatus};

const SEEDS: [[&str; 3]; 3] = [
    ["lantern", "harbor", "orchard"],
    ["atlas", "meadow", "signal"],
    ["cobalt", "willow", "ember"],
];

pub(super) fn thread(
    account_index: usize,
    chat_id: i64,
    chat_index: usize,
    base_timestamp: i64,
) -> Vec<Message> {
    // Chat slot four is the single intentionally empty conversation.
    if chat_index == 4 {
        return Vec::new();
    }

    let count = 18_i64;
    (1..=count)
        .map(|sequence| {
            let account_offset =
                i64::try_from(account_index).expect("three fixture accounts fit i64");
            let chat_offset = i64::try_from(chat_index).expect("six fixture chats fit i64");
            let id = 100_000 + account_offset * 100_000 + chat_offset * 1_000 + sequence;
            let media = media_kind(chat_index, sequence);
            let text = text(account_index, chat_index, sequence, media);
            let failed = chat_index == 2 && sequence == 16;
            let outgoing = failed || sequence == count || sequence % 4 == 0;
            Message {
                id,
                chat_id,
                text: Cow::Borrowed(text),
                timestamp: base_timestamp.saturating_sub((count - sequence) * 60),
                status: if failed {
                    MessageStatus::Failed
                } else if outgoing {
                    MessageStatus::Sent
                } else {
                    MessageStatus::Received
                },
                is_outgoing: outgoing,
                is_service: false,
                reply_to: (sequence == 7).then_some(id - 3),
                media,
                media_id: media.map(|_| 500_000 + id),
            }
        })
        .collect()
}

fn media_kind(chat_index: usize, sequence: i64) -> Option<MediaKind> {
    if chat_index != 3 {
        return None;
    }
    match sequence {
        5 => Some(MediaKind::Photo),
        9 => Some(MediaKind::File),
        12 => Some(MediaKind::Voice),
        15 => Some(MediaKind::Sticker),
        _ => None,
    }
}

fn text(
    account_index: usize,
    chat_index: usize,
    sequence: i64,
    media: Option<MediaKind>,
) -> &'static str {
    if matches!(media, Some(MediaKind::Photo | MediaKind::Sticker)) {
        return "";
    }
    if chat_index < 3 && sequence == 1 {
        return SEEDS[account_index][chat_index];
    }
    match (account_index, chat_index, sequence) {
        (_, 0, 2) => "سلام، حالت چطوره؟",
        (_, 0, 3) => "مرحبا، كيف حالك؟",
        (_, 0, 5) => "Decomposed café: cafe\u{301}.",
        (_, 0, 6) => "The observatory sketch shows 👩‍🔬 beside a telescope.",
        (_, 0, 8) => "The poster uses ❤️ and :wave: in its caption.",
        (_, 1, 4) => "A fictional link label: https://example.invalid/field-note",
        (_, 3, 9) => "A text note from the imaginary paper archive.",
        (_, 3, 12) => "Voice note placeholder; text metadata only.",
        _ if media.is_some() => "A fictional attachment with a short caption.",
        _ => "The paper map marks a quiet path past the old station.",
    }
}
