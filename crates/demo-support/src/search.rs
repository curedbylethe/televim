use domain::{
    global_search::{GlobalHit, MediaFilter, SearchSpec},
    message::{MediaKind, Message},
    search::word_prefix_match,
};

pub(crate) const GLOBAL_PAGE_SIZE: usize = 20;

#[derive(Debug, Clone)]
pub(crate) struct GlobalPage {
    pub spec: SearchSpec,
    pub hits: Vec<GlobalHit>,
    pub cursor: usize,
}

pub(crate) fn search_messages(messages: &[Message], query: &str) -> Vec<i64> {
    messages
        .iter()
        .filter(|message| word_prefix_match(message.display_body(), query))
        .map(|message| message.id)
        .take(domain::search::SEARCH_MATCHES)
        .collect()
}

pub(crate) fn global_hits<'a>(
    spec: &SearchSpec,
    messages: impl Iterator<Item = &'a Message>,
) -> Vec<GlobalHit> {
    let mut hits: Vec<_> = messages
        .filter(|message| {
            let text_match =
                spec.text.is_empty() || word_prefix_match(message.display_body(), &spec.text);
            let date_match = spec.since.is_none_or(|since| message.timestamp >= since)
                && spec.until.is_none_or(|until| message.timestamp <= until);
            text_match && date_match && media_matches(spec.media, message)
        })
        .map(|message| GlobalHit {
            chat_id: message.chat_id,
            message_id: message.id,
            text: message.text.to_string(),
            media: message.media,
            sent_at: Some(message.timestamp),
            outgoing: message.is_outgoing,
        })
        .collect();
    hits.sort_by(|a, b| {
        b.sent_at
            .cmp(&a.sent_at)
            .then_with(|| b.message_id.cmp(&a.message_id))
    });
    hits
}

fn media_matches(filter: MediaFilter, message: &Message) -> bool {
    match filter {
        MediaFilter::None => true,
        MediaFilter::PhotoVideo => matches!(
            message.media,
            Some(MediaKind::Photo | MediaKind::Video | MediaKind::Gif)
        ),
        MediaFilter::Document => matches!(message.media, Some(MediaKind::File)),
        MediaFilter::Music => false,
        MediaFilter::Voice => matches!(message.media, Some(MediaKind::Voice)),
        MediaFilter::Url => {
            message.display_body().contains("http://")
                || message.display_body().contains("https://")
        }
    }
}
