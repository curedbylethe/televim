//! Turns the typed global-search line into the [`SearchSpec`] the server sees.
//!
//! The tokens are `media:`, `since:` and `until:`, recognised anywhere in the
//! line by their ASCII-lowercased prefix. Every other word — `https://…`,
//! `12:30`, anything with a colon — stays free text, because a colon is legal
//! in ordinary text. `from:`, `sender:` and `by:` are recognised only so they
//! can be refused: the server's global method has no sender filter.
//!
//! A token repeated is not an error — the last occurrence wins.
//!
//! A refusal is returned rather than flashed: the caller flashes it and reopens
//! the prompt with the reader's line, so nothing typed is lost.

use domain::global_search::{MediaFilter, SearchSpec};

use crate::date;

/// Why a typed line is not a search, and the word that caused it.
#[derive(Debug)]
pub(crate) struct Refusal {
    /// The offending word as typed; empty when the refusal is about the line.
    ///
    /// Nothing reads it yet: it is kept so a caller can point at the word
    /// without parsing the sentence.
    #[allow(dead_code)]
    pub token: String,

    /// The sentence the status line shows.
    pub message: String,
}

/// Parses `query` into the spec to send, `offset` being the reader's UTC
/// offset in seconds, which makes `since`/`until` whole days in their zone.
pub(crate) fn parse(query: &str, offset: i64) -> Result<SearchSpec, Refusal> {
    let mut media = MediaFilter::None;
    let mut since = None;
    let mut until = None;
    let mut since_word = None;
    let mut until_word = None;
    let mut words = Vec::new();

    for word in query.split_ascii_whitespace() {
        let lower = word.to_ascii_lowercase();

        if let Some(value) = value_after(word, &lower, "media:") {
            media = media_filter(value).ok_or_else(|| Refusal {
                token: word.to_owned(),
                message: format!("{word} is not a media filter: try photo_video."),
            })?;
        } else if let Some(value) = value_after(word, &lower, "since:") {
            since = Some(date::day_start(day(value, word)?, offset));
            since_word = Some(word);
        } else if let Some(value) = value_after(word, &lower, "until:") {
            let day = day(value, word)?;
            until = Some(date::day_start(day.saturating_add(1), offset) - 1);
            until_word = Some(word);
        } else if is_sender_token(&lower) {
            return Err(Refusal {
                token: word.to_owned(),
                message: format!(
                    "{word} is not a filter here: global search has no sender filter."
                ),
            });
        } else {
            words.push(word);
        }
    }

    if let (Some(start), Some(end), Some(from), Some(to)) = (since, until, since_word, until_word)
        && start > end
    {
        return Err(Refusal {
            token: from.to_owned(),
            message: format!("{from} is after {to}."),
        });
    }

    if words.is_empty() {
        return Err(Refusal {
            token: String::new(),
            message: "a filter needs a word to search for.".to_owned(),
        });
    }

    let text = words.join(" ");
    Ok(SearchSpec {
        echo: text.clone(),
        text,
        media,
        since,
        until,
    })
}

/// The value of `word` after `prefix`, when the lowercased word starts with it.
///
/// The slice is safe because ASCII case folding keeps every byte and every
/// index where it was.
fn value_after<'a>(word: &'a str, lower: &str, prefix: &str) -> Option<&'a str> {
    if lower.starts_with(prefix) {
        Some(&word[prefix.len()..])
    } else {
        None
    }
}

/// Whether the word names the sender filter this search cannot honour.
fn is_sender_token(lower: &str) -> bool {
    ["from:", "sender:", "by:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// The day key a `since:`/`until:` value names, or the refusal naming it.
fn day(value: &str, word: &str) -> Result<i64, Refusal> {
    let date = date::parse_iso_date(value).ok_or_else(|| Refusal {
        token: word.to_owned(),
        message: format!("{word} is not a date: try 2026-01-01."),
    })?;

    Ok(date::days_from_civil(date))
}

/// The filter a `media:` value names, or `None` when it is not one of them.
fn media_filter(value: &str) -> Option<MediaFilter> {
    match value.to_ascii_lowercase().as_str() {
        "photo_video" => Some(MediaFilter::PhotoVideo),
        "document" => Some(MediaFilter::Document),
        "music" => Some(MediaFilter::Music),
        "voice" => Some(MediaFilter::Voice),
        "url" => Some(MediaFilter::Url),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::{CivilDate, clock, day_key, day_start, days_from_civil};

    /// Seconds in a civil day.
    const DAY: i64 = 86_400;

    /// Tehran, the stage's `+03:30` example: a half-hour zone, so an offset
    /// applied as whole hours cannot pass.
    const TEHRAN: i64 = 3 * 3_600 + 30 * 60;

    fn spec(query: &str) -> SearchSpec {
        parse(query, 0).expect("the query is a search")
    }

    fn refusal(query: &str) -> Refusal {
        parse(query, 0).expect_err("the query is refused")
    }

    fn day_of(year: i64, month: u32, day: u32) -> i64 {
        days_from_civil(CivilDate { year, month, day })
    }

    #[test]
    fn media_photo_video_reaches_the_spec() {
        let spec = spec("holiday media:photo_video");

        assert_eq!(spec.media, MediaFilter::PhotoVideo);
        assert_eq!(spec.text, "holiday", "the token is not sent to the server");
    }

    #[test]
    fn every_media_value_maps_to_its_filter() {
        for (value, filter) in [
            ("photo_video", MediaFilter::PhotoVideo),
            ("document", MediaFilter::Document),
            ("music", MediaFilter::Music),
            ("voice", MediaFilter::Voice),
            ("url", MediaFilter::Url),
        ] {
            assert_eq!(
                spec(&format!("x media:{value}")).media,
                filter,
                "media:{value}"
            );
        }
    }

    #[test]
    fn a_media_value_is_refused_by_name() {
        for (query, message) in [
            (
                "media:photo",
                "media:photo is not a media filter: try photo_video.",
            ),
            ("media:", "media: is not a media filter: try photo_video."),
        ] {
            let refusal = refusal(query);

            assert_eq!(refusal.message, message);
            assert_eq!(refusal.token, query, "the token is the word as typed");
        }
    }

    #[test]
    fn dates_become_the_readers_day_bounds() {
        let spec = parse("party since:2026-03-01 until:2026-03-02", TEHRAN)
            .expect("the query is a search");
        let first = date::day_start(day_of(2026, 3, 1), TEHRAN);
        let last = date::day_start(day_of(2026, 3, 3), TEHRAN) - 1;

        assert_eq!(spec.since, Some(first), "since is the day's first second");
        assert_eq!(spec.until, Some(last), "until is the day's last second");
        assert_eq!(clock(first, TEHRAN).as_deref(), Some("00:00"));
        assert_eq!(clock(last, TEHRAN).as_deref(), Some("23:59"));
        assert_eq!(day_key(first, TEHRAN), Some(day_of(2026, 3, 1)));
        assert_eq!(day_key(last, TEHRAN), Some(day_of(2026, 3, 2)));
        assert_eq!(
            last - first,
            2 * DAY - 1,
            "both bounds are whole days, inclusive"
        );
    }

    #[test]
    fn an_inverted_range_is_refused() {
        let refusal = refusal("since:2026-03-01 until:2026-02-01");

        assert_eq!(
            refusal.message,
            "since:2026-03-01 is after until:2026-02-01."
        );
        assert_eq!(refusal.token, "since:2026-03-01");
    }

    #[test]
    fn a_sender_token_names_the_missing_filter() {
        for prefix in ["from", "sender", "by"] {
            let query = format!("{prefix}:ada hello");
            let refusal = refusal(&query);

            assert_eq!(
                refusal.message,
                format!("{prefix}:ada is not a filter here: global search has no sender filter.")
            );
            assert_eq!(refusal.token, format!("{prefix}:ada"));
        }
    }

    #[test]
    fn filters_alone_are_refused() {
        let refusal = refusal("media:voice");

        assert_eq!(refusal.message, "a filter needs a word to search for.");
        assert_eq!(refusal.token, "");
    }

    #[test]
    fn a_word_with_a_colon_stays_text() {
        let spec = spec("https://example.com 12:30");

        assert_eq!(spec.text, "https://example.com 12:30");
        assert_eq!(spec.media, MediaFilter::None);
        assert_eq!(spec.since, None);
        assert_eq!(spec.until, None);
    }

    #[test]
    fn the_echo_drops_the_tokens_it_restates() {
        let spec = spec("  media:voice  hello   world ");

        assert_eq!(
            spec.echo, "hello world",
            "the filter is restated as a clause, so it leaves the echo"
        );
        assert_eq!(spec.text, "hello world", "what the server sees");
    }

    #[test]
    fn the_last_token_wins() {
        assert_eq!(spec("media:voice media:url x").media, MediaFilter::Url);
        assert_eq!(
            spec("since:2026-01-01 since:2026-02-01 x").since,
            Some(day_start(day_of(2026, 2, 1), 0))
        );
    }

    #[test]
    fn an_unparseable_date_is_refused_by_name() {
        let refusal = refusal("x since:2026-13-01");

        assert_eq!(
            refusal.message,
            "since:2026-13-01 is not a date: try 2026-01-01."
        );
        assert_eq!(refusal.token, "since:2026-13-01");
    }
}
