use domain::message::MediaKind;

use crate::DemoMedia;

pub(super) fn fixture(
    kind: MediaKind,
    account_index: usize,
    chat_index: usize,
    message_id: i64,
) -> DemoMedia {
    let filename = format!("demo-media-{message_id}.txt");
    match kind {
        MediaKind::Photo => DemoMedia {
            kind,
            filename: format!("demo-image-{message_id}.svg"),
            mime: "image/svg+xml",
            bytes: format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"96\" height=\"64\" viewBox=\"0 0 96 64\"><rect width=\"96\" height=\"64\" fill=\"#3a3a36\"/><path d=\"M0 52 30 21 52 45 68 28 96 56v8H0z\" fill=\"#b01e28\"/><text x=\"8\" y=\"14\" fill=\"#ede4d3\" font-size=\"8\">FICTIONAL {}.{}</text></svg>",
                account_index + 1,
                chat_index + 1
            )
            .into_bytes(),
            caption: "A small fictional observatory sketch.".to_owned(),
            downloadable: true,
        },
        MediaKind::File => DemoMedia {
            kind,
            filename: format!("demo-note-{message_id}.txt"),
            mime: "text/plain",
            bytes: b"Fictional note. No real conversation data.\n".to_vec(),
            caption: "A text note from the imaginary paper archive.".to_owned(),
            downloadable: true,
        },
        MediaKind::Voice => DemoMedia {
            kind,
            filename: format!("demo-voice-note-{message_id}.txt"),
            mime: "text/plain",
            bytes: b"Voice note placeholder; text metadata only.\n".to_vec(),
            caption: "Voice note placeholder; text metadata only.".to_owned(),
            downloadable: true,
        },
        MediaKind::Sticker => DemoMedia {
            kind,
            filename: format!("demo-sticker-{message_id}.txt"),
            mime: "text/plain",
            bytes: b"[fictional sticker token]".to_vec(),
            caption: "Fictional sticker token; no bitmap is included.".to_owned(),
            downloadable: false,
        },
        MediaKind::Video | MediaKind::Gif | MediaKind::AnimatedSticker => DemoMedia {
            kind,
            filename,
            mime: "text/plain",
            bytes: format!("{} placeholder; no media player is provided.\n", kind.label())
                .into_bytes(),
            caption: format!("{} placeholder; text metadata only.", kind.label()),
            downloadable: true,
        },
    }
}
