//! Colour scheme.
//!
//! # The order styles compose in
//!
//! Five of them can land on one cell — the cursor's reverse video, a selection's
//! background, a search match's colour, a caret, and the plain text under all of
//! them. **Four** of them compose in this order, each `patch`ed over the last:
//!
//! 1. `text` — the baseline everything starts from;
//! 2. `match_hit` — a row a search found;
//! 3. `selection_bg` — a slice of a row a selection covers;
//! 4. `selection` — the cursor's `REVERSED` row, drawn by the list widget.
//!
//! The fifth is a **caret**, and it is deliberately *not* a fifth patch.
//! `caret_insert` and `caret_normal` say what a caret looks like on a row the
//! surface has *not* reversed. On a reversed row the caret comes out as the
//! ground colour, and it gets there by **not** being patched with `selection` at
//! all: one cell of the row keeps its own ink, which is a hole in the reverse
//! video. That is the design's rule, and it is why a caret cannot be a patch —
//! `patch` can only add, and the caret's whole job on a reversed row is to take
//! something away. [`crate::text_row`] is what applies it.
//!
//! Later wins, because `Style::patch` takes the other style's fields wherever the
//! other style sets one. Four consequences worth stating rather than leaving to
//! be discovered:
//!
//! - `selection_bg` never uses `REVERSED`, and neither does `match_hit`. The
//!   cursor owns reverse video; a second user of it would leave the reader unable
//!   to tell which row the cursor is on.
//! - `selection_bg` is a background and `match_hit` a foreground, so a cell that
//!   is both is legible rather than one of them winning outright.
//! - A cell that is both takes the *selection's* foreground, because the
//!   selection is applied later and a selection's text follows the selection
//!   rather than the terminal. The match keeps its `BOLD`, which is what still
//!   says a match is there.
//! - A caret on a reversed row is a hole, not a colour, so it is legible
//!   whatever the surface behind it happens to be. Painting it in a second ink
//!   would need that ink to clear the contrast floor against *the selection's*
//!   background instead of the terminal's, which is a number that changes with
//!   the theme; a hole needs no floor.
//!
//! # Why the two carets differ by a modifier and not by a colour
//!
//! Both are `text`'s own colour. A terminal has no border to draw a hollow with,
//! and a two-column bar overhangs a row in a way no cell can, so the *shape* of a
//! caret has to come from the cell decoration rather than from the palette:
//! `caret_insert` reverses the cell and `caret_normal` underlines it. That is
//! also the reason the line cannot ask the terminal for its cursor: a terminal
//! cursor has exactly one shape, and these are two.
//!
//! # Why the values are hex and not the named ANSI colours
//!
//! `Color::Rgb` is a truecolour escape sequence, so a 16- or 256-colour terminal
//! renders it as whatever it approximates. The values below are desaturated to
//! survive that, and the four mode labels are the rows it hurts most: they are
//! the only rows with two colours set, so an approximation error is doubled.
//! `every_mode_label_clears_the_body_text_floor` is what keeps them legible.

use ratatui::style::{Color, Modifier, Style};

/// [`Color::Rgb`] is not a `const fn`, so every literal below needs a helper.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub border: Style,
    pub border_focused: Style,
    pub text: Style,
    pub text_dim: Style,
    pub selection: Style,
    /// A row a search matched.
    ///
    /// Bold and a colour of its own rather than `selection`'s `REVERSED`: the
    /// cursor can stand on a match, and the two styles are composed for that
    /// row, so this has to stay legible underneath the reverse video.
    pub match_hit: Style,
    /// A slice of a row a selection covers.
    ///
    /// A background rather than a foreground, for the same reason
    /// [`Theme::match_hit`] is a foreground: the cursor can stand inside a
    /// selection, and the two have to compose for that cell. A background is what
    /// leaves a match's colour legible on top of it.
    ///
    /// It carries a foreground of its own as well, and that is the point: the
    /// text *on* a selection follows the selection, not the terminal. This is the
    /// one background that is the same colour on every terminal, so its text is
    /// too — [`Theme::text`] would be `#1d2128` on a light one, which is 2.48:1
    /// on `#6b4bab`.
    pub selection_bg: Style,
    /// The line's caret while it is being composed.
    ///
    /// A block, which is what an insert caret is in every terminal, and the
    /// reason this is a modifier and not a colour. It is drawn in `text`'s own
    /// ink because the bar is never reversed, so a reversed cell is the only
    /// thing distinguishing it.
    pub caret_insert: Style,
    /// The line's caret in its own Normal mode, and a card row's inline position.
    ///
    /// One cell marked without being filled, which is a terminal's nearest
    /// equivalent of a hollow. The two carets are the two shapes a terminal
    /// cursor cannot be at once, which is why neither of them asks the terminal
    /// for one.
    pub caret_normal: Style,
    pub mode_normal: Style,
    pub mode_insert: Style,
    pub mode_visual: Style,
    pub mode_confirm: Style,
    /// The connection dot while the feed delivers.
    ///
    /// A foreground in the insert label's own green: the dot is one cell, so
    /// it takes its ink rather than its ground.
    pub conn_connected: Style,
    /// The connection dot while a bring-up or a rebuild is under way.
    ///
    /// Connecting and reconnecting share one yellow — the visual label's —
    /// because both are the same wait: nothing has answered yet.
    pub conn_transient: Style,
    /// The connection dot once the bring-up or the reconnect budget is spent.
    ///
    /// The confirm label's red: offline is the state that needs acting on.
    pub conn_offline: Style,
}

impl Default for Theme {
    fn default() -> Self {
        // A terminal's own background is `background` and this program never
        // paints it. `surface` is a panel's inside and is likewise left alone:
        // a client that paints its own background is a client that fights the
        // reader's colour scheme, and that is the reader's decision.
        Self {
            border: Style::default().fg(rgb(0x3a, 0x41, 0x50)),
            border_focused: Style::default().fg(rgb(0x5a, 0xd4, 0xe6)),
            text: Style::default().fg(rgb(0xd7, 0xdc, 0xe5)),
            text_dim: Style::default().fg(rgb(0x7b, 0x84, 0x96)),
            selection: Style::default().add_modifier(Modifier::REVERSED),
            match_hit: Style::default()
                .fg(rgb(0xe5, 0xc0, 0x7b))
                .add_modifier(Modifier::BOLD),
            // A `fg` here as well as a `bg`: text painted on a selection follows
            // the *selection*, not the terminal. `text` is #d7dce5 on a dark
            // terminal and #1d2128 on a light one, and #1d2128 on this background
            // is 2.48:1.
            selection_bg: Style::default()
                .bg(rgb(0x6b, 0x4b, 0xab))
                .fg(rgb(0xd7, 0xdc, 0xe5)),
            // Both carets are `text`. A palette entry that distinguished them by
            // colour would be a distinction a terminal could not draw, because
            // what separates them is a shape and a shape is a modifier.
            caret_insert: Style::default()
                .fg(rgb(0xd7, 0xdc, 0xe5))
                .add_modifier(Modifier::REVERSED),
            caret_normal: Style::default()
                .fg(rgb(0xd7, 0xdc, 0xe5))
                .add_modifier(Modifier::UNDERLINED),
            // One ink for all four, because all four clear 4.5:1 with it: the
            // two-foreground split this palette replaced put NORMAL and CONFIRM
            // *under* the floor, and CONFIRM is the word a reader has to parse
            // before destroying something.
            mode_normal: Style::default()
                .bg(rgb(0x4a, 0x7f, 0xb5))
                .fg(rgb(0x0d, 0x11, 0x17)),
            mode_insert: Style::default()
                .bg(rgb(0x6f, 0xbf, 0x73))
                .fg(rgb(0x0d, 0x11, 0x17)),
            mode_visual: Style::default()
                .bg(rgb(0xe5, 0xc0, 0x7b))
                .fg(rgb(0x0d, 0x11, 0x17)),
            mode_confirm: Style::default()
                .bg(rgb(0xe0, 0x6c, 0x75))
                .fg(rgb(0x0d, 0x11, 0x17)),
            // The mode labels' own hues as foregrounds: the dot is one cell,
            // so it takes their ink rather than their ground.
            conn_connected: Style::default().fg(rgb(0x6f, 0xbf, 0x73)),
            conn_transient: Style::default().fg(rgb(0xe5, 0xc0, 0x7b)),
            conn_offline: Style::default().fg(rgb(0xe0, 0x6c, 0x75)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The relative luminance WCAG 2.1 defines, and the ratio it gives two
    /// colours.
    ///
    /// Written out because there is no dependency for it, and because a palette
    /// whose contrast is a test does not need a paragraph defending it.
    fn contrast(a: Color, b: Color) -> f64 {
        let (hi, lo) = {
            let (x, y) = (relative_luminance(a), relative_luminance(b));
            if x >= y { (x, y) } else { (y, x) }
        };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The sRGB transfer function, then WCAG's weights for the primaries.
    fn relative_luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("the palette is truecolour hex; {color:?} is not");
        };
        let channel = |raw: u8| {
            let c = f64::from(raw) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// The ink every mode label is painted in, on both palettes.
    ///
    /// One value for all eight labels. The split it replaced put four of the
    /// eight under the floor.
    const LABEL_INK: Color = Color::Rgb(0x0d, 0x11, 0x17);

    /// Every mode label, on both palettes, clears the body-text floor.
    ///
    /// The eight are the only cells in the program with two colours set, so they
    /// are the only ones where a wrong foreground is invisible to a review of a
    /// screenshot and visible to the reader. Four of the eight were under the
    /// floor: the dark column's `NORMAL` and `CONFIRM` at 3.06:1 and 2.32:1, and
    /// the light column's at 4.12:1 and 3.63:1. Two of the four were in a column
    /// with no implementation and no test, which is why they survived as long as
    /// they did.
    ///
    /// The light four are literals, and that is the point. `Theme::light()` does
    /// not exist, so the light column is a documented decision rather than shipped
    /// code — and a decision nobody checks is a decision that rots the moment
    /// somebody edits a hex value. Asserting the *pairs* rather than the *values*
    /// means the test keeps its meaning if the spec's numbers move: someone who
    /// lightens `mode-normal-light` again gets a failure telling them the floor,
    /// not a failure telling them a literal changed.
    #[test]
    fn every_mode_label_clears_the_body_text_floor() {
        let t = Theme::default();

        // The dark column reads from the theme, so a renamed field or a dropped
        // foreground fails here rather than passing against a literal that no
        // longer describes what gets drawn.
        for (name, style) in [
            ("dark NORMAL", t.mode_normal),
            ("dark INSERT", t.mode_insert),
            ("dark VISUAL", t.mode_visual),
            ("dark CONFIRM", t.mode_confirm),
        ] {
            let (Some(fg), Some(bg)) = (style.fg, style.bg) else {
                panic!("{name} has no fg or no bg; a mode label needs both");
            };
            let ratio = contrast(fg, bg);
            assert!(
                ratio >= 4.5,
                "{name} is {ratio:.2}:1, under the 4.5:1 floor"
            );
        }

        // The light column does not ship, so these are the spec's values and not
        // `Theme`'s. They are literals precisely because there is no
        // `Theme::light()` to read them from — which is the reason they are here.
        for (name, bg) in [
            ("light NORMAL", Color::Rgb(0x6d, 0x9d, 0xd0)),
            ("light INSERT", Color::Rgb(0x4f, 0x9d, 0x55)),
            ("light VISUAL", Color::Rgb(0xb9, 0x8d, 0x2e)),
            ("light CONFIRM", Color::Rgb(0xd6, 0x7b, 0x83)),
        ] {
            let ratio = contrast(LABEL_INK, bg);
            assert!(
                ratio >= 4.5,
                "{name} is {ratio:.2}:1, under the 4.5:1 floor"
            );
        }
    }

    /// The text on a selection clears the floor against the selection's own
    /// colour, which is the reason `selection_bg` carries a foreground at all.
    #[test]
    fn selection_text_clears_the_floor_against_the_selection() {
        let t = Theme::default();
        let (Some(fg), Some(bg)) = (t.selection_bg.fg, t.selection_bg.bg) else {
            panic!("selection_bg needs a fg: it is the text's background, not just a block");
        };
        let ratio = contrast(fg, bg);
        assert!(ratio >= 4.5, "text on a selection is {ratio:.2}:1");
    }
}
