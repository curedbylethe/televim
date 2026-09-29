//! The `:shortcode` fragment the input line is inside, and what it could become.
//!
//! The trigger is a pure function of the draft and the caret, re-derived after
//! every key that reaches the line rather than cached: a cache is a second
//! thing to keep in step with the buffer, and that is the failure this module
//! exists not to have. Deciding whether the reader is completing is [`detect`];
//! ranking what the catalog offers is [`candidates`], which is private to this
//! module and reached through the trigger.
//!
//! The catalog is [`emojis`], the GitHub gemoji set: 1914 emoji in CLDR order,
//! 1870 of them with a shortcode and 40 with more than one. It is `&'static`
//! data — `phf` tables linked into the binary — and it is reached only while a
//! reader is composing, which is what keeps it off the startup path.

use std::ops::Range;

/// How many candidates a completion keeps.
///
/// Eight is what a popup can show without the list needing a scroll of its own,
/// and it is the number `emoji_popup` clamps to as well — one number, because
/// two numbers for one list is one of them eventually disagreeing.
pub const MAX_CANDIDATES: usize = 8;

/// The `:query` fragment the caret is inside, and what it could become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    /// The `:` and the query, as a byte range into the draft.
    pub range: Range<usize>,

    /// The shortcode being typed, without its `:`.
    pub query: String,

    /// Ranked best first, never longer than [`MAX_CANDIDATES`].
    ///
    /// `&'static Emoji` rather than `Emoji`, because the crate deliberately
    /// implements neither `Clone` nor `Copy` for the value.
    pub candidates: Vec<&'static emojis::Emoji>,

    /// Which one the reader is on. Always inside `candidates`.
    pub selected: usize,
}

impl Trigger {
    /// The candidate the reader is on, if the list has one.
    #[must_use]
    pub fn chosen(&self) -> Option<&'static emojis::Emoji> {
        self.candidates.get(self.selected).copied()
    }

    /// Moves the selection one candidate, wrapping at both ends.
    ///
    /// Wrapping rather than clamping: the list is at most eight, and a reader
    /// who has run off one end is reaching for the other.
    pub fn move_selection(&mut self, forward: bool) {
        let len = self.candidates.len();
        if len == 0 {
            return;
        }
        self.selected = if forward {
            (self.selected + 1) % len
        } else {
            (self.selected + len - 1) % len
        };
    }

    /// Clamps `selected` into the list, for a candidate list that changed.
    ///
    /// What a re-detection does with the row the reader was on: a filter that
    /// shortens the list keeps their place when it can, and puts them on the
    /// last candidate when it cannot.
    pub fn reselect(&mut self, selected: usize) {
        self.selected = selected.min(self.candidates.len().saturating_sub(1));
    }
}

/// The `:query` fragment the caret is inside, or `None`.
///
/// Byte offsets, because that is what the draft and its caret are: a byte into
/// the text, on a character boundary, which `LineEditor::caret` guarantees.
///
/// `None` for every one of these, and the reasons are the rule:
///   - nothing between the last `:` and the caret (`:`, `::`, `:-)`);
///   - a query that does not begin with a letter (`:1`, `:8ball`, `:+1`);
///   - a character the catalog has no use for (`:http`, `:a b`);
///   - a `:` straight after a word character (`12:30`, `abc:cry`);
///   - a query nobody matches (`:zzzqqq`).
///
/// The last one closes rather than opening on nothing. A popup reading
/// "no emoji found" is a row of the reader's screen answering a question they
/// did not ask, and it re-appears on every keystroke of a word they are still
/// typing.
#[must_use]
pub fn detect(text: &str, caret: usize) -> Option<Trigger> {
    let caret = caret.min(text.len());
    let colon = text[..caret].rfind(':')?;
    let query = &text[colon + 1..caret];
    if !is_query(query) {
        return None;
    }

    // GitHub's rule: a shortcode opens where a word ends, not inside one, so
    // `12:30` and `note:cry` are not completions while `(:cry`, `hi :cry` and a
    // draft that begins `:cry` are.
    if let Some(previous) = text[..colon].chars().next_back()
        && is_word(previous)
    {
        return None;
    }

    let candidates = candidates(query);
    if candidates.is_empty() {
        return None;
    }

    Some(Trigger {
        range: colon..caret,
        query: query.to_owned(),
        candidates,
        selected: 0,
    })
}

/// Whether `query` is a shape the catalog can answer.
///
/// The first character is a letter because the catalog's non-letter initials —
/// `+1`, `-1`, `100`, `8ball` — would make `:-)` and `12:3` open a popup. Every
/// later character is `[A-Za-z0-9_+-]`, verified against the catalog: it holds
/// nothing else and nothing upper case. There is no `to_lowercase` anywhere in
/// this module, because the catalog is lower case by construction and `C` is a
/// different key from `c`, which the reader can type.
fn is_query(query: &str) -> bool {
    let mut chars = query.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'))
}

/// Whether `c` is the kind of character a shortcode cannot open after.
///
/// Underscore included so a query does not open inside an identifier, and a
/// non-ASCII letter is not, because the rule is GitHub's and its `\w` is ASCII:
/// `héllo:cry` is a shortcode and `hello:cry` is not.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The candidates for `query`, best first.
///
/// One pass over the catalog, which is 1870 shortcodes and costs about five
/// microseconds, so there is no index to build and nothing to invalidate: a
/// trie over shortcodes would be a cache with an invalidation rule in front of
/// a problem that is not a problem.
///
/// **Every** shortcode of an emoji is matched, not only its first. Forty emoji
/// have more than one, and `:poop`, `:hankey` and `:shit` are three ways of
/// naming the same 💩 — matching `shortcode()` alone would answer two of the
/// three with nothing. Each emoji enters the list once, ranked by its best
/// alias for this query.
///
/// Ranked by the *matching* shortcode — exact, then prefix, then substring —
/// and shortest match first within a tier, because the shortcode the reader
/// typed is the thing they are looking at. Ties keep catalog order, which is
/// CLDR order and therefore stable across runs.
fn candidates(query: &str) -> Vec<&'static emojis::Emoji> {
    let mut ranked: Vec<(u8, usize, &'static emojis::Emoji)> = Vec::new();

    for emoji in emojis::iter() {
        let best = emoji
            .shortcodes()
            .filter_map(|shortcode| tier(shortcode, query).map(|tier| (tier, shortcode.len())))
            .min();

        if let Some((tier, length)) = best {
            ranked.push((tier, length, emoji));
        }
    }

    ranked.sort_by_key(|&(tier, length, _)| (tier, length));
    ranked.truncate(MAX_CANDIDATES);
    ranked.into_iter().map(|(_, _, emoji)| emoji).collect()
}

/// How well `shortcode` answers `query`.
///
/// `0` is an exact match, `1` a prefix and `2` a substring; nothing else
/// matches. The tier is the first half of the sort key, so an exact match
/// outranks every prefix however long.
fn tier(shortcode: &str, query: &str) -> Option<u8> {
    if shortcode == query {
        Some(0)
    } else if shortcode.starts_with(query) {
        Some(1)
    } else if shortcode.contains(query) {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The candidate shortcodes for a draft with the caret at its end.
    fn shortcodes(text: &str) -> Vec<&'static str> {
        detect(text, text.len())
            .map(|trigger| {
                trigger
                    .candidates
                    .iter()
                    .filter_map(|emoji| emoji.shortcode())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_query_is_the_text_between_the_colon_and_the_caret() {
        let trigger = detect("hi :cry", 7).expect("a completion is up");

        assert_eq!(trigger.range, 3..7);
        assert_eq!(trigger.query, "cry");
    }

    #[test]
    fn a_shortcode_at_the_start_of_the_draft_still_opens_it() {
        let trigger = detect(":cry", 4).expect("a completion is up");

        assert_eq!(trigger.range, 0..4);
    }

    #[test]
    fn a_colon_with_no_query_is_not_a_shortcode() {
        for text in [":", "::", ":-)"] {
            assert!(detect(text, text.len()).is_none(), "{text:?}");
        }
    }

    #[test]
    fn a_query_that_does_not_begin_with_a_letter_is_not_a_shortcode() {
        for text in [":1", ":8ball", ":+1", ":-"] {
            assert!(detect(text, text.len()).is_none(), "{text:?}");
        }
    }

    #[test]
    fn a_url_is_not_a_shortcode() {
        assert!(detect("https://", 8).is_none());
    }

    #[test]
    fn a_clock_is_not_a_shortcode() {
        assert!(detect("12:30", 5).is_none());
    }

    #[test]
    fn a_word_character_before_the_colon_ends_it() {
        assert!(detect("note:cry", 8).is_none());
    }

    #[test]
    fn a_character_the_catalog_has_no_use_for_ends_the_query() {
        assert!(detect(":cry!", 5).is_none());
    }

    #[test]
    fn a_query_nobody_matches_opens_nothing() {
        assert!(detect(":zzzqqq", 7).is_none());
    }

    /// The catalog is pinned here rather than trusted: an `emojis` upgrade that
    /// reorders or drops a shortcode fails this test instead of silently
    /// changing what `:cry` offers.
    #[test]
    fn the_candidates_are_ranked_exact_then_prefix_then_shorter() {
        assert_eq!(
            shortcodes(":cry"),
            ["cry", "crystal_ball", "crying_cat_face"]
        );
    }

    /// Shorter wins inside a tier, because the shortest prefix is the one the
    /// reader is most likely to be spelling.
    #[test]
    fn a_shorter_shortcode_outranks_a_longer_one_that_also_matches() {
        let names = shortcodes(":smi");
        let at = |name: &str| {
            names
                .iter()
                .position(|code| *code == name)
                .unwrap_or_else(|| panic!("{name:?} is in {names:?}"))
        };

        assert!(at("smile") < at("smiley"), "{names:?}");
        assert!(at("smirk") < at("smiley"), "{names:?}");
    }

    #[test]
    fn an_alias_shortcode_finds_the_same_emoji() {
        for query in [":poop", ":hankey"] {
            let trigger = detect(query, query.len()).expect("a completion is up");
            let chosen = trigger.chosen().expect("a candidate is chosen");

            assert_eq!(chosen.as_str(), "💩", "{query}");
        }
    }

    #[test]
    fn every_candidate_carries_the_shortcode_that_matched_it() {
        for query in [":cry", ":poop", ":a", ":smile", ":heart", ":flag"] {
            let trigger = detect(query, query.len()).expect("a completion is up");

            assert!(!trigger.candidates.is_empty(), "{query}");
            assert!(
                trigger
                    .candidates
                    .iter()
                    .all(|emoji| emoji.shortcode().is_some()),
                "{query} offered an emoji with no shortcode"
            );
        }
    }

    #[test]
    fn the_selection_wraps_in_both_directions() {
        let mut trigger = detect(":cry", 4).expect("a completion is up");
        assert!(trigger.candidates.len() > 1);

        trigger.move_selection(false);
        assert_eq!(trigger.selected, trigger.candidates.len() - 1, "up wraps");

        trigger.move_selection(true);
        assert_eq!(trigger.selected, 0, "and down comes back");
    }

    #[test]
    fn the_selection_is_clamped_into_a_shorter_list() {
        let mut trigger = detect(":cry", 4).expect("a completion is up");
        assert_eq!(trigger.candidates.len(), 3);

        trigger.reselect(7);

        assert_eq!(trigger.selected, 2);
    }

    #[test]
    fn the_candidate_list_is_never_longer_than_the_ceiling() {
        for query in [":a", ":e", ":o"] {
            let trigger = detect(query, query.len()).expect("a completion is up");

            assert!(
                trigger.candidates.len() <= MAX_CANDIDATES,
                "{query} offered {}",
                trigger.candidates.len()
            );
        }
    }
}
