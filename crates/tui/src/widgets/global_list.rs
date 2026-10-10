//! The global search overlay: the messages `?` or `:search` found, grouped by chat.
//!
//! It covers the whole body, not one column, because a hit can belong to any
//! chat: the panes are not drawn behind it. The status line below it carries the
//! state (`?query — 9 results in 3 chats`); the foot of the box carries the keys
//! while the list is up, and `Esc: close` while it is not (searching, no matches,
//! failed), because no key but `Esc` does anything then.
//!
//! One rule per chat, the chat's name set into it, then one row per message:
//! two columns of indent, the time, the sender tag, and a snippet cut around the
//! first match. The cursor row takes reverse video across the whole interior;
//! the chat header is not a row the cursor can land on.
//!
//! The window is stateless: it scrolls just far enough to keep the cursor on
//! screen, so nothing about where it sits is kept between frames.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Focus};
use crate::bidi::{self, BidiMode, Chunk};
use crate::text_row::{self, TextRow};
use crate::wrap::columns;
use domain::global_search::GlobalHit;

/// Most cells a snippet takes, after the indent, time and sender tag.
const SNIPPET_CELLS: usize = 62;

/// The keys the foot names while the list is up.
const HINT: &str = "j/k: move  ⏎: open  { }: chat  gg/G: ends  Esc: close";

/// The foot while the list is not up: only `Esc` answers.
const HINT_IDLE: &str = "Esc: close";

/// Draws the overlay over `area`, leaving its last row to the status line.
///
/// Drawn only when a search is active and the prompt does not have the focus:
/// while the prompt is open the bar is the query being typed, as with the
/// new-chat list.
pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let search = app.global_search();
    if app.ui.focus == Focus::Input || !search.is_active() {
        return;
    }

    let body = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    if body.height < 3 || body.width < 3 {
        return;
    }
    frame.render_widget(Clear, body);

    let theme = app.ui.theme;
    let list_up = !search.in_flight() && !search.is_empty();
    let hint = if list_up { HINT } else { HINT_IDLE };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.border)
        .title(Line::from(vec![
            Span::styled("─ ", theme.border),
            Span::styled("Search", theme.text),
            Span::styled(" ", theme.border),
        ]))
        .title_bottom(Line::from(vec![
            Span::styled("─ ", theme.border),
            Span::styled(hint, theme.text),
            Span::styled(" ", theme.border),
        ]));
    let inner = block.inner(body);
    frame.render_widget(block, body);

    if !list_up {
        return;
    }

    let width = usize::from(inner.width);
    let selected = search.selected();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_line = 0;

    for group in search.groups() {
        let name = app
            .chats()
            .iter()
            .find(|chat| chat.id == group.chat_id)
            .map_or("", |chat| chat.title.as_str());
        lines.push(header_line(app, name, group.hits.len(), width));

        for (offset, hit) in group.hits.iter().enumerate() {
            let cursor = group.first_index + offset == selected;
            if cursor {
                cursor_line = lines.len();
            }
            let query = search.query().unwrap_or("");
            lines.push(message_line(app, hit, query, width, cursor));
        }
    }

    // Keep the cursor on screen: the window ends at the cursor once it is past
    // the first screenful, and starts at the top before that.
    let visible = usize::from(inner.height);
    let start = (cursor_line + 1).saturating_sub(visible);
    let shown: Vec<Line<'static>> = lines.into_iter().skip(start).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

/// The rule that heads a chat: `──── Name · N ────`, the name in body ink and
/// the count dim, the rule filling the rest of the row.
fn header_line(app: &App, name: &str, count: usize, width: usize) -> Line<'static> {
    let theme = app.ui.theme;
    let head = format!(" · {count} ");
    let used = columns("──── ") + columns(name) + columns(&head);
    let fill = width.saturating_sub(used);

    Line::from(vec![
        Span::styled("──── ", theme.border),
        Span::styled(name.to_owned(), theme.text),
        Span::styled(head, theme.text_dim),
        Span::styled("─".repeat(fill), theme.border),
    ])
}

/// One message row. On the cursor row every span takes `selection`, so the
/// whole interior is reverse video; the quieter spans take body ink under it,
/// and a match keeps `match_hit`, both as the design draws them.
fn message_line(
    app: &App,
    hit: &GlobalHit,
    query: &str,
    width: usize,
    cursor: bool,
) -> Line<'static> {
    let theme = app.ui.theme;
    let paint = |style: Style| {
        if cursor {
            theme.selection.patch(style)
        } else {
            style
        }
    };
    let quiet = if cursor { theme.text } else { theme.text_dim };
    let tag = if hit.outgoing { "[you]" } else { "[them]" };
    let time = hit
        .sent_at
        .and_then(|at| crate::date::clock(at, app.offset()))
        .unwrap_or_default();

    let mut spans = vec![Span::styled(
        format!("  {time:<5}  {tag:<6} "),
        paint(quiet),
    )];
    let mut used = columns(&format!("  {time:<5}  {tag:<6} "));

    // The ellipses are chrome, drawn outside the reorder; only the body is permuted.
    let snip = snippet(hit.display_body(), query, SNIPPET_CELLS);
    let ellipsis = |shown: bool| shown.then(|| Span::styled("…", theme.text));
    let body = snippet_spans(app, hit.display_body(), &snip.body);
    for span in ellipsis(snip.lead)
        .into_iter()
        .chain(body)
        .chain(ellipsis(snip.trail))
    {
        used += columns(&span.content);
        spans.push(Span::styled(span.content.into_owned(), paint(span.style)));
    }

    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used)),
        paint(Style::default()),
    ));
    Line::from(spans)
}

/// A snippet: the body runs, each text and whether it is a match, and whether the
/// text was cut at either end. The cut ends are the caller's to draw.
struct Snippet {
    lead: bool,
    body: Vec<(String, bool)>,
    trail: bool,
}

/// Cuts `text` to at most `cells` cells, centred on the first match of `query`.
///
/// Matching is case-insensitive and every match in the window is a run of its
/// own. A cut end is reported, not drawn. A row always shows its first match: the
/// window is chosen around it, and only a match wider than the snippet is cut.
fn snippet(text: &str, query: &str, cells: usize) -> Snippet {
    let chars: Vec<char> = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let needle: Vec<char> = query.chars().collect();
    let found = find_matches(&chars, &needle);
    let (anchor_start, anchor_end) = found.first().copied().unwrap_or((0, 0));
    let width_of = |c: char| columns(c.encode_utf8(&mut [0; 4]));

    // Narrow the window until the text plus its ellipses fits in `cells`.
    let mut budget = cells;
    let (start, end) = loop {
        let (start, end) = window(&chars, anchor_start, anchor_end, budget, width_of);
        let ellipses = usize::from(start > 0) + usize::from(end < chars.len());
        let text_cells: usize = chars[start..end].iter().map(|&c| width_of(c)).sum();
        if text_cells + ellipses <= cells || budget == 0 {
            break (start, end);
        }
        budget -= 1;
    };

    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut push = |ch: char, matched: bool| match runs.last_mut() {
        Some((run, last)) if *last == matched => run.push(ch),
        _ => runs.push((ch.to_string(), matched)),
    };

    for (index, &ch) in chars.iter().enumerate().take(end).skip(start) {
        let matched = found.iter().any(|&(s, e)| (s..e).contains(&index));
        push(ch, matched);
    }
    Snippet {
        lead: start > 0,
        body: runs,
        trail: end < chars.len(),
    }
}

/// The snippet's body, drawn in the order the message's direction asks for.
///
/// The direction is the whole message's, not the snippet's: a row of an
/// all-neutral snippet carries no evidence of its own. Under
/// [`BidiMode::Terminal`] the runs are emitted as they are stored, split per
/// cluster; under [`BidiMode::Visual`] each piece of the reordered snippet keeps
/// its match, so a match follows its own characters across the reorder.
fn snippet_spans(app: &App, message: &str, runs: &[(String, bool)]) -> Vec<Span<'static>> {
    // The runs laid end to end, so each one is a byte range the text_row functions
    // can paint.
    let flat: String = runs.iter().map(|(run, _)| run.as_str()).collect();
    let mut ranges: Vec<(Range<usize>, bool)> = Vec::with_capacity(runs.len());
    let mut at = 0;
    for (run, matched) in runs {
        ranges.push((at..at + run.len(), *matched));
        at += run.len();
    }

    let text = flat.as_str();
    let ink = text_row::Ink::readonly(&app.ui.theme);
    let row = |range: Range<usize>, matched: bool| TextRow {
        ink,
        text,
        range,
        matched,
        selected: None,
        caret: None,
        concealed: false,
        reversed: false,
    };

    let drawn = match app.bidi() {
        BidiMode::Terminal => ranges
            .iter()
            .flat_map(|(range, matched)| {
                text_row::per_cluster(text_row::spans(&row(range.clone(), *matched)))
            })
            .collect::<Vec<_>>(),
        BidiMode::Visual => {
            let base = bidi::base_direction(message);
            let mut drawn = Vec::new();
            for chunk in bidi::visual_row(text, base) {
                // A chunk is a forward slice of the snippet, so cutting it at the
                // run boundaries keeps the pieces in the order they are drawn.
                for (range, matched) in &ranges {
                    let logical =
                        chunk.logical.start.max(range.start)..chunk.logical.end.min(range.end);
                    if logical.is_empty() {
                        continue;
                    }
                    let piece = [Chunk {
                        cells: columns(&text[logical.clone()]),
                        logical: logical.clone(),
                    }];
                    drawn.extend(text_row::spans_permuted(&row(logical, *matched), &piece));
                }
            }
            drawn
        }
    };

    drawn
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
}

/// Every non-overlapping match of `needle` in `chars`, as character ranges.
fn find_matches(chars: &[char], needle: &[char]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    if needle.is_empty() || needle.len() > chars.len() {
        return found;
    }

    let mut index = 0;
    while index + needle.len() <= chars.len() {
        let hit = needle
            .iter()
            .zip(&chars[index..index + needle.len()])
            .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()));
        if hit {
            found.push((index, index + needle.len()));
            index += needle.len();
        } else {
            index += 1;
        }
    }
    found
}

/// The character range of at most `budget` cells that holds the anchor, with the
/// anchor centred as far as the text allows.
fn window(
    chars: &[char],
    anchor_start: usize,
    anchor_end: usize,
    budget: usize,
    width_of: impl Fn(char) -> usize,
) -> (usize, usize) {
    let anchor_cells: usize = chars[anchor_start..anchor_end]
        .iter()
        .map(|&c| width_of(c))
        .sum();
    if anchor_cells >= budget {
        // The match alone fills the window: keep as much of it as fits.
        let mut end = anchor_start;
        let mut used = 0;
        while end < anchor_end && used + width_of(chars[end]) <= budget {
            used += width_of(chars[end]);
            end += 1;
        }
        return (anchor_start, end);
    }

    let mut start = anchor_start;
    let mut before = 0;
    let before_budget = (budget - anchor_cells) / 2;
    while start > 0 && before + width_of(chars[start - 1]) <= before_budget {
        start -= 1;
        before += width_of(chars[start]);
    }

    let mut remaining = budget - anchor_cells - before;
    let mut end = anchor_end;
    while end < chars.len() && width_of(chars[end]) <= remaining {
        remaining -= width_of(chars[end]);
        end += 1;
    }
    while start > 0 && width_of(chars[start - 1]) <= remaining {
        start -= 1;
        remaining -= width_of(chars[start]);
    }

    (start, end)
}

#[cfg(test)]
mod tests {
    use super::{find_matches, snippet};
    use crate::app::App;
    use crate::bidi::BidiMode;
    use domain::global_search::GlobalHit;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// Arabic, logical: one word, four glyphs.
    const ARABIC: &str = "سلام";

    /// A hit whose text is the Arabic word, from someone else, so the row is tagged
    /// `[them]` and it is the only message row on the screen.
    fn arabic_hit() -> GlobalHit {
        GlobalHit {
            chat_id: 1,
            message_id: 1,
            text: ARABIC.to_owned(),
            media: None,
            sent_at: None,
            outgoing: false,
        }
    }

    /// The screen's row that carries the sender tag, as its cells read left to right.
    fn hit_screen(mode: BidiMode) -> (Buffer, u16) {
        let mut app = App::mock().with_bidi(mode);
        app.begin_global_search("لا");
        assert!(app.adopt_global_search("لا", vec![arabic_hit()], 1, 0));

        let mut terminal = Terminal::new(TestBackend::new(80, 12)).expect("the backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");
        let buffer = terminal.backend().buffer().clone();

        let y = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("[them]")
            })
            .expect("the hit's row is on screen");
        (buffer, y)
    }

    /// The screen's row that carries the sender tag, as its cells read left to right.
    fn hit_row(mode: BidiMode) -> String {
        let (buffer, y) = hit_screen(mode);
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn a_match_follows_its_letters_across_the_visual_reorder() {
        let (buffer, y) = hit_screen(BidiMode::Visual);
        let fg_of = |glyph: &str| {
            (0..buffer.area.width)
                .find(|&x| buffer[(x, y)].symbol() == glyph)
                .map(|x| buffer[(x, y)].fg)
                .expect("the glyph is on the row")
        };

        let match_fg = App::mock()
            .ui
            .theme
            .match_hit
            .fg
            .expect("a match is a foreground");
        assert_eq!(fg_of("ل"), match_fg, "the match's first letter");
        assert_eq!(fg_of("ا"), match_fg, "the match's second letter");
        assert_ne!(fg_of("س"), match_fg, "the plain letter is not a match");
    }

    #[test]
    fn a_right_to_left_hit_is_drawn_in_logical_order_in_terminal_mode() {
        let row = hit_row(BidiMode::Terminal);
        assert!(
            row.contains(ARABIC),
            "the word is whole and in order: {row:?}"
        );
    }

    #[test]
    fn a_right_to_left_hit_is_drawn_reversed_in_visual_mode() {
        let row = hit_row(BidiMode::Visual);
        let reversed: String = ARABIC.chars().rev().collect();
        assert!(
            row.contains(&reversed),
            "the word is drawn reversed: {row:?}"
        );
        assert!(!row.contains(ARABIC), "and not in logical order: {row:?}");
        assert!(row.contains("[them]"), "the tag is not permuted: {row:?}");
    }

    /// The snippet's text, with its match runs wrapped in `[` `]`, for asserting.
    fn marked(text: &str, query: &str, cells: usize) -> String {
        let snip = snippet(text, query, cells);
        let body: String = snip
            .body
            .into_iter()
            .map(|(run, matched)| if matched { format!("[{run}]") } else { run })
            .collect();
        let lead = if snip.lead { "…" } else { "" };
        let trail = if snip.trail { "…" } else { "" };
        format!("{lead}{body}{trail}")
    }

    #[test]
    fn a_short_text_is_shown_whole_with_its_matches_marked() {
        assert_eq!(
            marked("Bring the tickets", "tickets", 62),
            "Bring the [tickets]"
        );
    }

    #[test]
    fn matching_ignores_case_and_marks_every_match() {
        assert_eq!(
            find_matches(&['A', 'b', 'a', 'B'], &['a', 'b']),
            vec![(0, 2), (2, 4)]
        );
    }

    #[test]
    fn a_long_text_is_cut_around_the_first_match_with_ellipses() {
        let text = format!("{}tickets{}", "a".repeat(100), "b".repeat(100));
        let cut = marked(&text, "tickets", 20);

        assert!(cut.contains("[tickets]"), "the match is kept: {cut}");
        assert!(
            cut.starts_with('…') && cut.ends_with('…'),
            "both ends cut: {cut}"
        );
        assert!(cut.chars().count() <= 20 + 4, "bounded: {cut}");
    }

    #[test]
    fn the_snippet_never_exceeds_its_cells() {
        let text = "é".repeat(200);
        let snip = snippet(&text, "zzz", 62);
        let cells: usize = snip.body.iter().map(|(run, _)| run.chars().count()).sum();
        assert!(cells <= 62, "{cells} cells");
    }
}
