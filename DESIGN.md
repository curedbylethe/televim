---
name: Televim
category: Developer Tools
surface: terminal
colors:
  background: "#0b0d10"
  surface: "#141821"
  border: "#3a4150"
  border-focused: "#5ad4e6"
  text: "#d7dce5"
  text-dim: "#7b8496"
  selection: "#d7dce5"
  caret-insert: "#d7dce5"
  caret-normal: "#d7dce5"
  match: "#e5c07b"
  selection-bg: "#6b4bab"
  mode-normal: "#4a7fb5"
  mode-insert: "#6fbf73"
  mode-visual: "#e5c07b"
  mode-confirm: "#e06c75"
  border-light: "#b0b5c0"
  border-focused-light: "#1a7f8c"
  text-light: "#1d2128"
  text-dim-light: "#6b7280"
  caret-insert-light: "#1d2128"
  caret-normal-light: "#1d2128"
  match-light: "#8a6100"
  mode-normal-light: "#6d9dd0"
  mode-insert-light: "#4f9d55"
  mode-visual-light: "#b98d2e"
  mode-confirm-light: "#d67b83"
  on-mode-light: "#0d1117"
---

# Televim

> Category: Developer Tools

> Surface: terminal

> **Status: proposed.** `crates/tui/src/theme.rs` still carries the 16 named ANSI
> colours. The palette below lands in `02-theme-palette.md`. Until that commit,
> the `baseline` column is what the binary draws and the `v2` column is the
> target; after it, `v2` is what it draws.

*A private room, read at a glance, typed at without the mouse.*

A Telegram client that shows only the people you talk to — no channels, no
groups, no bots — inside a terminal, driven by Vim keys. The name is Telegram
and Vim run together, and that is the whole proposition: the reading is the
same shape as every other terminal program, and the typing is the same shape as
every other editor.

## The canvas

The terminal's own background is `background`, and the program never paints it.
A client that paints its own background is a client that fights the reader's
colour scheme, and that is the reader's decision. In this document `#0b0d10` is
the colour a frame is drawn on and nothing else.

`surface` is the inside of a bordered panel. It is specified, and it is also not
painted: a panel is a border and the text inside it, and painting the inside
would be a second background where the reader expects one. It exists so a
frame has a name for the region rather than reaching for `background`.

## Colour Palette

Fifteen roles, because the TUI has fifteen. A generic forty-token web palette
would be fiction, and the ones this program does not have are the ones a
designer must not invent either.

Two columns: **baseline** is what a standard 16-colour terminal shows for the
ANSI name in the third column, and is what the binary draws today. **v2** is
the proposal. The light column is v2 re-specified for a reader on a light
terminal. Only the dark column ships: the light column's numbers are decided
and tested, `Theme` has one set of values, and a second palette is a later
change.

| Role | ANSI today | baseline (dark) | v2 (dark) | v2 (light) | Why it holds this colour |
| :--- | :--------- | :-------------- | :-------- | :--------- | :----------------------- |
| `background` | — | terminal's own | `#0b0d10` | terminal's own | never painted; a frame's ground only |
| `surface` | — | terminal's own | `#141821` | terminal's own | a panel's inside, also never painted |
| `border` | `DarkGray` | `#545862` | `#3a4150` | `#b0b5c0` | present, not attention-drawing |
| `border-focused` | `Cyan` | `#00afaf` | `#5ad4e6` | `#1a7f8c` | the *only* thing saying where keys go |
| `text` | `White` | `#e5e5e5` | `#d7dce5` | `#1d2128` | body |
| `text-dim` | `Gray` | `#8a8a8a` | `#7b8496` | `#6b7280` | decoration, hints, an unfocused draft, a media placeholder |
| `selection` | `REVERSED` | reverse video | reverse video | reverse video | the row the cursor is on, in a read-only surface; owned by it alone |
| `caret-insert` | — | terminal's own | `#d7dce5` | `#1d2128` | the bar, while the line is composed; on the bar's plain ground |
| `caret-normal` | — | terminal's own | `#d7dce5` | `#1d2128` | the hollow; `text` on a plain ground, `background` on the reversed row |
| `match` | `Yellow`+`BOLD` | `#87af00` | `#e5c07b` | `#8a6100` | a search hit; colour **and** bold |
| `selection-bg` | `Magenta` | `#af005f` | `#6b4bab` | `#6b4bab` | a Visual selection's background |
| `mode-normal` | `Blue`/`Black` | `#0000af` | `#4a7fb5` | `#6d9dd0` | the mode label |
| `mode-insert` | `Green`/`Black` | `#00af00` | `#6fbf73` | `#4f9d55` | ditto |
| `mode-visual` | `Yellow`/`Black` | `#afaf00` | `#e5c07b` | `#b98d2e` | ditto |
| `mode-confirm` | `Red`/`Black` | `#af0000` | `#e06c75` | `#d67b83` | ditto |

All eight mode labels, the four dark-column and the four light-column, are
`on-mode-light` (`#0d1117`). One label ink, in both columns, and no split. On a
dark terminal that is 4.50:1 on `mode-normal`, 8.45:1 on `mode-insert`, 10.96:1
on `mode-visual` and 5.92:1 on `mode-confirm`. On a light terminal it is 6.65:1
on `mode-normal-light` (`#6d9dd0`), 5.67:1 on `mode-insert-light`, 6.23:1 on
`mode-visual-light` and 6.33:1 on `mode-confirm-light` (`#d67b83`). The old
split put four of the eight under the body-text floor: `#d7dce5` was 3.06:1 on
`mode-normal` and 2.32:1 on `mode-confirm`, and 4.12:1 and 3.63:1 on the old
light fills `#3a6a99` and `#c0454e`. Those fills were also 3.34:1 and 3.79:1
against `#0d1117`, which is why they moved. Naming the foreground makes the
label's contrast a property of the theme rather than of the reader's terminal,
which is the only reason it is stated at all.

The two carets are the body-text value in both columns, so they clear the
body-text floor by the same margin `text` does: `#d7dce5` on the dark ground is
14.14:1, and `#1d2128` on a white light ground is 16.15:1. On the reversed row
their ink is `background`, the reverse of that same pair, which is the only ink
that survives the row's own swap. The values are listed twice because every role
is specified twice; the two carets differ in shape, never in ink, and the rule
that fixes which of the two inks applies is below.

### Why the refresh

`baseline`'s four mode labels are the raw ANSI primaries. `#0000af` on a dark
terminal is legible; on a light one it is a smear, and the mode label is the one
row that has to be readable in every state. The v2 set is desaturated so it
survives a terminal approximating it — which also means it survives a terminal
that is genuinely 16-colour, because `Color::Rgb` becomes a truecolour escape
that such a terminal maps to whatever it has.

Two costs, stated rather than hidden:

- **Truecolour is required.** On a 16- or 256-colour terminal v2 degrades to the
  terminal's own approximation, which the desaturated values are chosen to
  survive. It degrades; it does not break. `Color::Indexed` was rejected: it
  never looks right on the truecolour terminals that are the majority.
- **Text on a selection follows the selection, not the terminal.** `selection-bg`
  is `#6b4bab` in both columns, so the text on it is always `#d7dce5`, and that
  pair is 4.74:1. It passes. The light column's own text ink `#1d2128` on
  `#6b4bab` would be 2.48:1. The same rule covers a search match under a
  selection: it takes the pale ink too, because the light match `#8a6100` on
  `#6b4bab` is 1.18:1. Neither is reversed.

### Two rules the palette cannot change

Both are asserted by tests today, and a palette that broke them would be a
palette that made the screen lie.

1. **Reverse video belongs to the cursor alone.** `selection-bg` and `match`
   never use `REVERSED`. A second user of it leaves the reader unable to tell
   which row the cursor is on.
2. **`selection-bg` is a background.** A cell that is also a match keeps that
   background and takes the pale ink (`#d7dce5`), not the match colour, which is
   what keeps it legible. Neither is reversed. Verified by
   `a_match_under_a_selection` in `conversation.rs`, which asserts one cell
   carries both.

### The three names, and the caret rule

Three visuals, three names, and no synonyms. The engine's flag legend, the
stylesheet's classes and this document use the same three words, and the table is
the port list: the field each one becomes in `crates/tui/src/theme.rs`.

| The visual | What it is | Field | Flag | Class |
| :--------- | :--------- | :---- | :--- | :---- |
| **`selection`** | the row the cursor is on, in a read-only surface: reverse video on the row itself | `Theme::selection` (kept) | `r` | `.rv` |
| **`caret-insert`** | a two-column bar: the line, while it is being composed | `Theme::caret_insert` (added) | `c` | `.ci` |
| **`caret-normal`** | a hollow cell: the line's own Normal mode, and a card's inline position | `Theme::caret_normal` (added) | `n` | `.cn` |

**The rule, in three parts.**

1. **A read-only surface marks its cursor with reverse video on the row itself,
   and nothing else is reversed.** This is the rule above, unchanged, and
   `selection-bg` still never uses `REVERSED`: a second user of it leaves the
   reader unable to tell which row the cursor is on.
2. **A caret drawn inside a reversed row is the ground colour**, because that row
   has already taken `selection` — which is `text` — as its background. A caret
   in `text` there would be `text` on `text`. The ground is the only ink that
   survives the swap, and that is the whole reason there are two ink values and
   not one.
3. **A surface that is not reversed draws its caret in `text`.** The bar is the
   common case and it is always plain: an insert caret while the line is
   composed, and a normal caret in the line's own Normal mode. The card's inline
   position is the opposite case, and it is the only one: it is drawn inside the
   reversed row and takes the ground colour. The plain case is met once more on a
   card, where a charwise selection makes the row the `selection-bg` background
   rather than reverse video; the hollow takes `text` there too, by the same
   rule.

**The repo asks the terminal for its caret today**, in `input_bar.rs`, and it
must stop and paint instead. A terminal cursor has one shape, and the two names
above are exactly the two shapes it cannot be at once: the bar while Insert runs
and the hollow in the line's own Normal mode. Asking for a block gives a filled
block in both, so the state the reader is most often in — the line's Normal mode —
is the one the artifact and the repo disagree about. Painting the carets also
makes them the same kind of object the card's inline position already is: a cell
`TestBackend` can assert, rather than a request the backend does not model.

## Typography

The terminal's own. One monospace family, three weights, no sizes — a terminal
has one cell height and the reader chose the cell.

- **Weight is the only hierarchy.** Regular for body, `bold` for a search match
  and nothing else, `dim` for `text-dim`. No second size and no italic: both are
  unreliable across terminals, so the spec must not depend on them.
- **The column budget is a hard constraint, and it is tested.**
  `MODE_LABEL_WIDTH = 9` — the widest label is `NORMAL`, and a `Confirm` never
  shares the row with a hint, so it is one number rather than a case per label.
  `ASSUMED_WIDTH = 80` — every hint must fit an 80-column terminal less the
  label and its gap, which leaves 71. `ALL_HINTS` is the array of all fourteen,
  and a test iterates it, so a hint left out of the array is a hint the check
  never sees. **A hint that does not fit fails the build; the fix is a shorter
  sentence, not a wider budget.**
- **One cell is one character, and a character may be two cells wide.**
  `wrap::columns` is the table. A row is never cut inside a grapheme cluster —
  `grapheme::cluster_start` / `cluster_end` are what make that true, and a
  delete removes one cluster rather than one code point.
- **`·` marks a cell that holds something the screen would otherwise show as
  nothing, in three places, and only those three.** In a draft being composed a
  space is a cell that paints nothing, and the insert caret is a two-column bar
  on a blank cell, so the key that typed one looked like the key that did
  nothing; the dot occupies the space's own cell, which is what keeps the caret
  and the wrap in step. `·` also joins a panel's title to a note (`· 2 selected`,
  `· typing`).
  And it is a card row's cue: one column, present, dim, on the row's first line,
  saying the row carries something. **The conversation gets no dots** — a message
  is read as prose, and a sentence with its spaces dotted reads as something else.
- **A continuation row of a wrapped draft has no prefix and no indent.** The
  `:` or `/` goes on the first row only: it names the whole line, and a reader
  who typed two of them has a question rather than a command.
- **The dots apply to every prompt the reader is typing in**, `:` and `/`
  included. The condition is not *what kind of prompt this is*, it is *is the
  caret in here right now* — the reason the dot exists is that a caret is
  standing on a blank cell, and that is true of a search query too.

### The hint rows

`ALL_HINTS` is the array a test iterates: eighteen entries, the sign-in and
passphrase rows among them. The model's `HINT` draws nineteen, among them the
contacts and archived lists' key help, which the build does not draw (below). These are the rows the screens
above reach, verbatim, with the width each one measures. The width is
`chars().count()`, the same count the test makes, and the array is the
authority: the numbers below were re-measured from it.

| Shown when | Text | Columns |
| :--------- | :--- | ------: |
| conversation, Normal, bar empty | ` i:ins  r:rep  e:edit  dd:del  v:vis  /:find  ::cmd  A:card  S:acct` | 67 |
| chat list has the focus | ` j/k: chat  ⏎: open  Tab: pane  h: conversation  A:card  S:you  p:pin` | 69 |
| conversation, Visual | ` d: delete  y: yank  s: forward  r: reply  Esc: cancel` | 54 |
| the forward picker is up | `VISUAL  j/k: choose  ⏎: forward  Esc: cancel` | 44 |
| a deletion is confirmed | ` y: delete  n/Esc: cancel` | 24 |
| the bar holds a draft, not focused | ` ⏎ draft — i to continue, ^J/⏎ to discard` | 41 |
| the line is being typed in | ` ⏎: send  ^J: newline  shift+⏎: newline where supported` | 55 |
| a `:shortcode` is being completed | ` ⇥/⏎: pick  ↑/↓: choose  Esc: close` | 35 |
| the line's own Normal mode | ` i/a: ins  w/b/e  x: del  dw/cc  p: paste  gg/G: ends  ⏎: send  Esc` | 67 |
| a selection inside the line | ` y: yank  d: cut  Esc: back` | 27 |
| the editable profile has the focus | ` e/Enter: edit  j/k: field  Esc/Tab/Ctrl+w: leave` | 49 |
| the self card has the focus | ` j/k: row  h/l: within  v: vis  y: yank  d: act  Esc: back` | 58 |
| a contact's card has the focus | ` j/k: row  h/l: within  v: vis  y/yy: yank  d: act  Esc: back` | 61 |
| the session is not signed in | ` ::signin  q:quit` | 17 |
| nothing has been read yet | ` q:quit` | 7 |

Notes, because they are the answers to questions a reader will have:

- **The subtraction starts from the binary's 67, not the 70 this document once
  quoted.** The row it used to print carried `D:dismiss` and did not carry
  `S:acct`. `D:dismiss` is gone — dismissing a failed send is a *refusal*, and a
  refusal is the status line's to say, not the hint's, which has no room for why —
  and `S:acct` is present. Eleven columns left the row and eight arrived, so the
  row the arithmetic starts from is three columns shorter than this document
  assumed.
- **`gd` is not on the conversation's row, and nothing is cut to make room.** The
  row is 67 of 71, so a hint for it (`  gd:quote`, ten columns) would not fit, and
  paying with `A:card`, `S:acct` or `q`'s `:q` route would trade a key that is
  always meaningful for one that is meaningful only when the cursor is on a reply. A
  fixed hint cannot know that; a sixteenth, per-message hint row would. The key is
  taught where it matters instead: on a message with no quote it answers with
  `Not a reply: gd jumps to the message a reply quotes.`, and the reply row
  already shows `> quote ‖`, the thing the key follows.
- **`A:card` costs eight columns and `q:quit` pays for it: 67 − 8 + 8 = 67, four
  columns spare.** A hint is the only place a reader learns a key the rest of the
  screen is silent about, and `A` is one. `q` is the only one of the three keys
  the reader uses most — `e:edit`, `v:vis`, `q:quit` — that has a second route to
  the same action: `:q` and `:quit` reach it, and `::cmd` is named on the same
  row. `e` and `v` have no other mention anywhere on screen, so they keep theirs.
  The cost is real and it is visible: a reader who has not learned `q` now has to
  learn `:q`.
- **What is tighter is the pair of keys, not the row.** The binary's row is three
  columns shorter than this document assumed, so it is the *easier* row to name a
  key on, and the corrected arithmetic says so: four columns spare, not one. The
  part that is genuinely tighter than before is that `A:card` and `S:acct` must
  both be named, `S` has no second route either, and neither may be dropped for the
  other — two keys where the old note counted one.
- **`S:acct` is on the conversation's row and `S:you` is on the chat list's, and
  that one key has two spellings is a rule rather than a slip.** `S` opens your
  own profile from either pane, and, like `A`, only a hint names it. The chat list
  spells it among the people, where you are one of them; the conversation spells
  it for the account the card is. A reader who meets the second spelling has to
  reconcile it with the first, so both are named deliberately and neither is
  renamed to match. The list's row is 66 of 71 with `A:card` and `S:you` in it.
- **The two card rows differ because their rows differ.** The self card names
  `d`, because it has two rows that act, and names `y` but not `yy`. A contact's
  card names `y/yy`, because that card is read and `yy` is the verb worth
  teaching, and it does not name `d` at all. Each names only keys its own card
  answers: a key named on a card that does not answer it tells the reader the key
  is wrong rather than that it is not shown.
- **`gg` and `G` are in the line's hint because the editor has neither.** A key
  the line answers with nothing else on screen naming it is a key the hint exists
  for.
- **The line's Normal mode has no `j`/`k`, and that is not an oversight.** The
  editor behind it makes those history navigation on a one-line buffer, and
  history is not built. The insert caret moves between the lines of a message from
  Insert mode, where the arrows are motions.
- **`^J` before `shift+⏎`**, because `^J` works in every terminal and needs no
  protocol, and a shifted `Enter` is only distinguishable where the terminal
  volunteers it.

### Text direction

A message is stored in **logical order** — the order it is read, so the first
word a reader reads is the first word in the string — and that is the order
every layer above the pixels holds it in: the model's text, `Message.text`, the
row ranges in the panel, selection, search and the register. **Which way a
message reads is a rendering question, and the rendering layer is the only place
that answers it.** `domain::Message` has no direction field and gets none, so
nothing downstream of the domain can learn a message's direction from its type.

- **Which way a message reads** is one answer for the whole message, asked of
  the **first strongly-directional character anywhere in it** — not of its first
  paragraph, because neutrals at the top cannot outvote what follows them. A
  message that opens `--- 123 ---` and then says `שלום` is right-to-left. A
  message with no strong character in it at all reads left-to-right, which is the
  algorithm's own default and not a special case here.
- **In what order a row's pieces are drawn** is a property of that one row, and
  is the Unicode Bidirectional Algorithm over the row alone, at the base direction
  the message was given. A row is not a reordered string: it stays the logical
  slice it was, and what comes back is a list of *pieces* of it in the order they
  are drawn. **A piece is always a whole number of grapheme clusters and always
  a logical slice**, because a piece is text the painter slices, and a Hebrew word
  is not written in the order it is read.
- **An embedded left-to-right run stays whole and stays as written.** Digits, a
  bracketed phrase, a file name in an otherwise right-to-left row: each is one
  run and keeps its own order. A clock time reads `21:30`, not `30:12`.
- **The reorder never changes a row's width.** The pieces partition the row — no
  byte dropped, none drawn twice — and their cell widths sum to the row's, so the
  two modes differ in the *order* of the cells and in nothing else. That is what
  makes the mode a layout-independent fact and not a reflow.
- **Rows are broken logically, in `wrap`, and only reordered at paint time.** The
  line breaks of a right-to-left message are the breaks its logical order gives
  it, which is not the break set a right-to-left layout would choose. This is a
  known ceiling, not a claim: preserving the byte-offset geometry is what keeps
  selection, search, scrolling and the register in logical space.
- **The quoted prefix is chrome and stays put.** `> … ‖ ` is the conversation's
  own decoration, drawn ahead of the text it introduces, and it is not part of
  what gets permuted — only the message's own text is.
- **The block is not right-aligned.** The sentence starts at the right of its own
  run; the message body still lays out left to right from the `WHO_WIDTH` gutter.
  Right-aligning the body would collide with the trailing note, which is already
  at the far end of the row, and with the logical wrap geometry.

**Who does the reordering is a setting, and the default is the terminal.**
`BidiMode::Terminal` emits the row as it is stored and lets the terminal's shaper
reverse a right-to-left run; `BidiMode::Visual` permutes the row here first. Both
are correct programs, on different terminals: kitty, wezterm, foot, iTerm2 and VTE
all shape and would reverse a second time if this program permuted as well, while
xterm and alacritty draw the cells they are handed and do nothing else. **There is
no detection**, because bidi and shaping are a setting on a terminal rather than
an identity — `wezterm` ships `bidi_enabled=false`, VTE needs `CSI ? 2501 h` — and
detection breaks under `tmux` and `ssh`. It is read once from configuration at
start-up and never while a frame is being drawn, so the same conversation is the
same height whichever way it is asked.

**Not claimed here, and not delivered:** contextual shaping (Arabic joining is
the terminal's, and a terminal that does not join renders unjoined), Rule L4
glyph mirroring, and visual-order line breaking. Reading a right-to-left message
correctly on a terminal that neither shapes nor applies the algorithm is the
`Visual` mode's reason to exist, and it is a half of the feature: no assertion in
this repository can prove how a given terminal renders the bytes emitted for it.

**Brackets are paired per row, as the program pairs them.** The model in `design/`
implements the reorder per row, and the row is the unit: `crates/tui/src/bidi.rs`
hands each wrapped row to `ParagraphBidiInfo`, so a bracket in one row is never
paired with one in another. Within a row the model runs BD16 and then N0 after W7
and before N1/N2. BD16 keeps a stack of 63 opening brackets, matches a closing
bracket to the nearest opening one it is canonically equivalent to (U+2329/U+232A
with U+3008/U+3009 are the only equivalents), takes the pairs in order of their
opening bracket, and abandons every pair if the stack overflows. N0 counts EN and
AN as R and sets a pair from what it holds: the embedding direction if that is
inside, the opposite direction only when the strong type before the pair is
opposite too, and nothing when it holds no strong type. A mark after a bracket takes
the bracket's type; an unmatched bracket is a plain neutral. Two adversarial
sequences, `字"מ}[1]` at left-to-right and `א1]Z(漢!)ב` at right-to-left, pin
the pass; their expected drawing orders were captured from `unicode-bidi` 0.3.18
and live in `design/scripts/check-model.js`. `crates/tui/src/bidi.rs` asks
`unicode-bidi` and is the authority; where the model and that file disagree, that
file is the answer.

## Layout

```
┌─ 30% ────────┬─ 70% ─────────────────────────────────────┐
│  Chats (12)  │  Conversation (3/48) · 2 selected          │  Constraint::Min(3)
│              │                                           │
│  border-     │  [them] a message wraps at the panel's    │
│  focused when│         width — at whitespace where there  │
│  it has focus│         is whitespace, and at the edge     │
│              │         where there is not                 │
├──────────────┴───────────────────────────────────────────┤
│ ┌ Message to Ada Lovelace ┐                              │  Length(2 + rows)
│ │ draft text, · for spaces, insert caret                 │  capped at 6
│ └───────────────────────────────────────────────────────┘
│  INSERT   ⏎: send  ^J: newline  shift+⏎: newline where…   │  Length(1)
└──────────────────────────────────────────────────────────┘
```

- **30 / 70 by percentage, three rows.** The top region is `Min(3)`, the bar is
  `Length(2 + rows)`, the status line is `Length(1)`.
- **The right column holds two contents.** The conversation and a profile card
  are the same region: `l` (or `A`) swaps the card in, `Esc` swaps it back, and
  the chat list gives up nothing. `Tab` and `Ctrl-w h`/`Ctrl-w l` walk the two
  panes; inside a card `h`/`l` are inline, which is why the pane keys carry the
  `Ctrl-w` prefix there. A card is not a stack and is not remembered: opening it
  again lands on its first row, where the conversation keeps the reader's place.
- **The bar is always a draft, never absent.** It holds what the reader last
  typed, or a hint. It grows to six rows (`INPUT_MAX_ROWS`) because a draft is
  the reader's own words and the bar is the only place they can be read back —
  but past six, the conversation is what the reader is reading. Under
  `BidiMode::Visual` a draft whose base direction is right-to-left is permuted in
  the bar as a message row is: the row is still broken logically, the `: ` / `/ `
  prefix stays chrome, and the caret is drawn at the visual position of the
  logical one.
- **An open draft is drawn as a row under the messages.** While the line is a message being written and has words in it, a row below the messages reads `[you|draft] ` in `text-dim`, then the words in the message ink, wrapped at the panel's width less the tag. Its rows are taken from the panel before the messages are given theirs, and no count includes them, so typing moves no scroll. It takes no caret, match or receipt. The cursor can rest on it, one past the last message, and then the row is drawn reversed, the same highlight the cursor uses on a message; no message is reversed by it. In follow mode it comes straight after the newest message: at the panel's bottom when the conversation fills the panel, directly under that message when it does not. Scrolled up, it is not drawn at all, since under an older message it would read as that message's: the input bar still shows the words. **DESIGN-TBD:** the row is built to the existing `[you]` tag and `text-dim` vocabulary. No design run was commissioned for it. The visual pass is open, including whether a draft should be pinned to the panel's bottom while the reader is scrolled up.
- **The focused pane's border is the only thing on screen that says where a
  keystroke goes.** Two panes drawn alike are two panes the reader has to guess
  between. `lit_borders` in `conversation.rs` reads the three corners and asserts
  exactly one is `border_focused` for each of the three focus values.
- **Titles are space-padded.** ` Chats (12) `, ` Conversation (3/48) `, ` Input `,
  ` draft `, ` Message to Ada Lovelace `, ` Reply `, ` Edit `, ` Command `,
  ` Find `.
- **The scrollbar is a column the body gives up**, and only above
  `MIN_BODY_WIDTH = 8`. A narrow panel has no room to give: the bar would cost
  more than it tells the reader.
- **Nothing is cached between frames.** A message is as tall as its text needs at
  the width the panel gave it, and the viewport, the scrollbar, `Ctrl+d`/`Ctrl+u`
  and the fetch triggers all count those rows. A cache is a second thing to keep
  in step with the window, which is the failure this arrangement exists to
  prevent.
- **The carets are painted, not asked for.** The three names, the rule they obey
  and the port list are above. A `█` the program paints is one the reader cannot
  tell from the terminal's own, and the terminal's own cannot be a bar in Insert
  and a hollow in Normal. Painting them makes both the same kind of cell the
  card's inline position already is, so `TestBackend` asserts them and section 09
  of the specimen shows them.

### Groups, day separators and read state

A message is still as many rows as its text needs, the sender label still sits
on its first row and a status still sits on its last. Three treatments sit on
top of that and add no row kind a message already had. Every rule below is
conditional on a message having a time: **a message with no time never groups,
never gets a separator and never shows a timestamp**, so a row drawn before these
treatments existed reads exactly as it did.

**Grouping.** Consecutive messages join one visual group when every one of these
holds against the message before: the same side, the same calendar day, no more
than five minutes (`GROUP_MIN`) between the two (the gap is to the *previous*
message, not to the group's first), and the later one is not a reply. A change
of direction, a reply, a day boundary or a longer gap always starts a new group.
A reply starts a group and may be followed into it.

- **The sender label, `[you]` or `[them]`, is on the group's first row only.**
  A later message in the group has a blank 7-column tag and its text begins in
  the same column, so the group reads as one turn. A reply's quoted target stays
  on the reply's first row, which is always a group's first row.
- **The time is on the group's last row only**, right-aligned in the status
  column: `21:04`. It is not repeated per message.
- **The cursor still selects a message.** The row fill and the selection follow
  `mi`, so the cursor can sit on any message of a group and the group does not
  select as one.
- **A message's own status is never hidden by grouping.** `[sending…]` and
  `[failed: …]` stay on the last row of the message that carries them, mid-group
  or not. Only the read state and the time are the group's.

**Read state (outgoing only).** An outgoing message carries what the peer has said
about it: nothing yet, `delivered` (acknowledged, not read) or `read`. It is drawn
**once per group, on the last row of the group's newest message**, left of the
time: `[delivered] 21:16`, `[read] 21:04`. A message with a status, `sending…` or
`failed: …`, shows that status in the same place **instead**, and a failed message
never claims delivered or read, however the peer's last answer stood. An
outgoing group still sending shows `[sending…] 21:32`; the earlier messages'
receipts are not shown. An incoming message has no read state, only a time. If
the state and the time do not fit beside the last line of text, they take a row of
their own, as a status always has.

| Last row of a group | Draws |
| :------------------ | :---- |
| outgoing, peer has read the newest | `[read] 21:04` |
| outgoing, acknowledged, not read | `[delivered] 21:16` |
| outgoing, newest is sending | `[sending…] 21:32` |
| outgoing, newest failed | `[failed: no route] 21:38` |
| outgoing, no answer from the peer yet | `21:40` |
| incoming | `20:14` |

**Day separators.** The first message of each calendar day is preceded by one row
naming the day, sitting exactly between the last message of the day before and the
first of the next, and above the first message of a chat's history. The label is
`Today`, `Yesterday`, the weekday name for the five days before that, and a full
date past the week: `Sep 20, 2026`.

- **It is a row of its own, the full width of the text** (52 columns, the width a
  message's tag and body occupy together): a rule of `─` in the border colour with
  the label set into its middle in the dim colour, padded by one space each side.
  It has no tag column, no fill, no sender label, no reply target and no status. It
  is not a bubble and not a message row.
- **It is not a message.** In the flat row list it is an entry with `mi: -1`, the
  way a `Loading…` row is, so the cursor, a selection and a search never land on
  it; it counts as one row for the viewport and the scrollbar. When the cursor is
  on the first message of a day, the separator above it is kept in view.
- **Group boundaries and separators agree.** A separator is always a group break,
  because a day boundary is.

## Vocabulary

This is the section a designer is most likely to get wrong, because a chat UI
has strong conventions this one deliberately does not follow. Verbatim from the
code.

| Written | Means | Source |
| :------ | :---- | :----- |
| `[you]` `[them]` | which side a message is from, on its first row; in a group, on the group's first row only | `rows.rs` |
| `[you|draft] ` | the open draft, drawn below the last message while it has words: the tag in `text-dim`, the words in the message ink. **DESIGN-TBD:** built to this vocabulary, not designed | `DRAFT_TAG` in `conversation.rs` |
| `> quoted ‖ body` | a reply: quote and body **on one row**, `‖` between | `rows::reply_prefix` |
| `> [message not loaded] ‖ body` | a reply whose target the window does not hold; `gd` on it fetches the target | asserted in `rows.rs` |
| `[image]` | **a message that carries a photo and no caption**: its whole body, in `text-dim` | `MediaKind::label` |
| `[video]` `[gif]` `[voice]` | ditto for a video, an animated GIF, a voice note | ditto |
| `[file]` | ditto, and the **catch-all**: a contact, a document, or any kind this build does not model, which degrades rather than vanishing | ditto |
| a caption beside a placeholder | never: `display_body()` returns the caption when there is one, so the label is drawn only for a message that says nothing; the caption is body ink | `Message::display_body` |
| `[sending…]` | on the message's **last** row, which is the one with room | asserted |
| `[failed: no route]` | ditto, with the reason, truncated to the room | `rows::status_suffix` |
| `[image… 42%]` | a download in flight: the kind's own placeholder with the percentage inside, drawn beside the placeholder and never replacing the body | `rows::download_token` |
| `[image… 2.0 MB]` | ditto when Telegram declared no size: the megabytes so far stand in for the percentage | ditto |
| `[failed: reason] (o retries)` | a download that failed: the reason, then the hint that `o` tries again; the reason wins truncation, so a narrow row clips the hint first | ditto |
| `[delivered]` | an outgoing group the peer's client has acknowledged and not read; the group's **last** row, beside any time | `rows::receipt_word` |
| `[read]` | an outgoing group the peer has read; same row, same place | ditto |
| `21:04` | a group's time, `HH:MM`, once, on the group's **last** row, right-aligned after any state | `rows::trailing_note` |
| `──── Today ────` | a day separator: a rule the width of the text with the day set into it | `rows::day_of` |
| `· 2` | the count a reader is typing before a day walk (`2gg`, `2G`): a bare number straight after the cursor and total, in `text`, with no noun because the next key may clear it | `dayCount` |
| `· 2 selected` | joined onto a panel title by `·` | `selection_note` |
| `· 3 match(es)` | a search's count, in the title | `search_note` |
| `· typing` | the peer is typing in the open conversation: a note on the conversation's title, in `text-dim`; the one title note that is not a count | `typing_note` |
| `·` | a space in a draft being composed; a title-note joiner; a card row's cue | three meanings, all deliberate |
| `Loading…` `Loading older…` `Loading newer…` | a fetch in flight | `FetchDirection::label` |
| `Cached messages — loading the latest…` | a conversation drawn from the local history cache while its newest page is in flight. On the **status line**, not as a row: `Loading…` stands in for messages, and a cached window has messages to show | `REVALIDATING_LABEL` |
| `Jumping to first unread…` | a jump the window could not answer | `JUMP_LABEL` |
| `Jumping to the quoted message…` | `gd` on a reply whose target is not loaded: the page around it is in flight | `JUMP_REPLY_LABEL` |
| `Jumping back…` `Jumping forward…` | `Ctrl-o` / `Ctrl-i` to a position whose message is no longer loaded | `JUMP_BACK_LABEL` / `JUMP_FORWARD_LABEL` |
| `Jumping to <day>…` | a day or date jump (`[N]gg` / `[N]G` / `:date <ISO>`) whose target day is not loaded — `<day>` in the day-separator vocabulary (`Today`, `Yesterday`, `Sep 26, 2026`) | `App::jump_label` (falls back to `JUMP_DAY_LABEL` when no clock stands) |
| `Jumping to the match…` | an `n`/`N` search jump whose match is outside the loaded window (criterion 3): own label, not the first-unread one | `JUMP_MATCH_LABEL` |
| `Not a reply: gd jumps to the message a reply quotes.` | `gd` on a message with no quote: a refusal, a `flash` | `NOT_A_REPLY` |
| `That message is no longer available.` | `gd` on a reply whose target the client does not hold at all | `JUMP_UNAVAILABLE` |
| `televim` | the resting status line, when there is nothing to say | `IDLE_STATUS` |
| `Quit televim? (y/n)` | a screen-wide confirmation | `QUIT_PROMPT` |
| `Delete your message from both sides? (y/n)` | ditto, naming **which side** | `DELETE_OUTGOING_PROMPT` |
| `Delete 3 of your messages? (y/n)` | a count, and pluralisation | `delete_yours_prompt` |
| `Block {name}? (y/n)` `Unblock {name}? (y/n)` | the two block confirmations, naming the person | `block_prompt` |
| `Delete this conversation? (y/n)` | a local conversation deletion | `DELETE_CONVERSATION_PROMPT` |
| `Delete this conversation for both sides? (y/n)` | the same, naming **which side** | `DELETE_CONVERSATION_BOTH_PROMPT` |
| `Report {name} to Telegram as {reason}? (y/n)` | a report, naming the reason in the reader's spelling | `report_prompt` |
| `archived {name}` `unarchived {name}` | the archive moved a chat, and brought it back | `archive` status |
| `blocked {name}` `unblocked {name}` | the block answer | `block` status |
| `renamed {name}` `local name cleared` | a local name was set, or cleared back to the peer's name | `:rename` |
| `deleted {name}` `deleted {name} for both sides` | the conversation is gone | `delete-chat` |
| `reported {name} as {reason}` | the report was accepted | `:report` |
| `contacts — {n}` `archived — {n}` | an open list and how many it holds | `contacts`, `archived` |
| `contacts — searching…` | the contacts fetch is in flight; the previous list stays on show | `ContactListState::label` |
| `contacts: {reason}` | the contacts fetch failed; the previous list stays on show | ditto |
| `unknown report reason: {token} — use spam, violence, …` | `:report` with a token that is not one of the ten, refused before any question | `:report` |
| `archive: {reason}` `block: {reason}` `delete chat: {reason}` `report: {reason}` `rename: could not save — {reason}` | a failed action, in its own sentence | the answer path |
| `NORMAL` `INSERT` `VISUAL` `CONFIRM` | upper case, in a filled label | `status_bar.rs` |
| ` i:ins  r:rep  e:edit ` | hints: `key:word`, two spaces between pairs | the hint rows above |
| `: ` `/ ` | a prompt's prefix, on its first row only; `/ ` is shared by the find line and the new-chat line | `App::prompt_prefix` |
| `3 message(s) selected — Esc clears` | the status line's sentence for a selection, **stating the unit** | `selection_note` |
| `3 character(s) selected — Esc clears` | ditto for a selection inside one message | ditto |
| `/query — match 2 of 5` | a search: the query, then where the walk is | `SearchState::label` |
| `/query — 2 loaded` | a **local** search, which is never an answer | `local_position` |
| `/query — 2 loaded — searching…` | a local search with the server still working | ditto |
| `/query — match 1 of 5 (first 2)` | a server walk that is still capped | `server_position` |
| `New chat` | the new-chat prompt's bar title, and the results box's title | `kindTitle`, `drawNewChat` |
| `/query — searching…` | the new-chat list while the query is in flight | `newChatStatus` |
| `/query — N candidates — searching…` | ditto, when candidates are already known | `newChatStatus` |
| `/query — N candidates` | the new-chat list once the answer lands, singular and plural | `newChatStatus` |
| `/query — no candidates` | the answer was nobody | `newChatStatus` |
| `/query — no candidates (search failed: reason)` | the request itself failed | `newChatStatus` |
| `/query — N candidates (search failed: reason)` | the request failed with some candidates | `newChatStatus` |
| `new` `chat` | a result row's standing: no conversation yet, or one to focus | `newChatHits` |
| `Profile · you` `Profile · Ada Lovelace` | a card's title; the `(n/m)` is the row the cursor is on, counting drawn rows (the reserved colour slot is not one) and appearing only when rows exist | `cardTitle` |
| `Profile · you` | the account's two shells too, signed out and nothing read yet: the subject is named in every state, with no count | `cardTitle` |
| `· name        Noor Haddad` | a card row: the cue, the label, the value | `CUE` |
| `not signed in` | the shell card's first line, then the reason, wrapped | `NOT_SIGNED_IN` |
| `reading the account…` | the account's shell **before anything has been read**: one dim line, and nothing more; it does not borrow the other one's wording | `READING` |
| `reading their profile…` | the contact card **before their profile has been read**: one dim line, and no rows | `CONTACT_READING` |
| `could not read this profile` | the contact card **when the read failed**; the reason follows, wrapped and dim | `CONTACT_UNAVAILABLE` |
| `Profile · {name}` | a contact shell's title: the subject is named even when there are no rows | `cardTitle` |
| `Set the credentials again with :signin.` | where to put the credentials back | `SET_CREDENTIALS` |
| `signed out` | the status the screen rests on after the reader signs out, until the sign-in field arrives | `apply_logged_out` |
| `Sign out and forget this session? (y/n)` | a screen-wide confirmation, raised by the `logout` row; `y` signs out | the card |
| `not yet: this build cannot add an account` | the `add account` row's refusal | ditto |
| `Not a row that acts: add account and logout are the two.` | `d` on a value row of the self card | ditto |
| `Not editable here: :settings opens the editable profile.` | `e` on the self card: the one route to the editable screen | ditto |
| `p: a card has no buffer to paste into` | why `p` is unbound, written to the hint's own row | ditto |
| `1 row(s) yanked — register filled, OSC 52 offered` | `yy`: the register first, then the offer to the terminal | ditto |
| `3 row(s) selected — Esc clears` | a selection of rows, stating the unit | `cardSel` |
| `Profile · editable` | the other screen: it edits, the cards read | `drawSettings` |
| `Sign in to Telegram` | the sign-in view's first line | `drawSignin` |
| `Phone` `Login code` | the two rows every sign-in has, in order | ditto |
| `two-factor password (3 attempts left)` | the third row, drawn **only** when Telegram answered `SESSION_PASSWORD_NEEDED`; the count starts at three and drops on each refusal | ditto |
| `Password hint: …` | the account's own hint, drawn under the password step when it has one | ditto |
| `Checking…` | a request is in flight; `⏎` is refused and a second `⏎` does nothing | ditto |
| `still checking — the answer is on its way` | the one `⏎` a request in flight will answer | ditto |
| `[ok]` / `[wrong code]` | a row Telegram accepted, and one it refused | ditto |
| `[ ⏎: sign in again ]` | the row's offer after `AUTH_KEY_UNREGISTERED` | ditto |
| `cancelling discards the code Telegram sent; ⏎ asks for a new one` | `Esc` at the code step, naming what it costs | ditto |
| `the code did not survive; ⏎ asks for a new one` | back after `Tab`/`Ctrl+w`: the step is kept, the code is not | ditto |
| `televim has no application credentials. …` | no `api_id`/`api_hash`: a sentence, not a form | `drawNoCreds` |
| `that code is not the one Telegram sent` | `PHONE_CODE_INVALID` | `AUTH` |
| `that code has expired — ⏎ asks for a new one` | `PHONE_CODE_EXPIRED` ✱ | ditto |
| `that is not a phone number Telegram will accept` | `PHONE_NUMBER_INVALID` | ditto |
| `Telegram has banned that number` | `PHONE_NUMBER_BANNED` | ditto |
| `too many attempts — wait, then try again` | `PHONE_NUMBER_FLOOD` | ditto |
| `that password is not right (2 attempts left)` | `PASSWORD_HASH_INVALID`, with the row's own count put in | ditto |
| `this account has no two-factor password` | `PASSWORD_MISSING` | ditto |
| `this session was revoked — sign in again; and, for the log, the stored session is discarded` | `SESSION_REVOKED` ✱ | ditto |
| `the stored session is no longer valid — sign in again` | `AUTH_KEY_UNREGISTERED` ✱ | ditto |
| the raw error text | anything else, verbatim | ditto |
| `Session file` | the launch box's title: the session file is encrypted and no key is available | `drawPass` |
| `The session file is encrypted and no key is available, so televim needs the passphrase to read its chats.` | the one sentence the launch box asks with and why | ditto |
| `Passphrase` `New passphrase` `Repeat passphrase` | the three concealed bar titles, one per line kind | `kindTitle` |
| `· cached` | the chat list's title note while the session is locked and the list is the local cache | `drawList` |
| `Deriving the key from the passphrase; ⏎ is refused while it runs.` | the launch box while the key is derived | `drawPass` |
| `Checking… — the key is being derived` | the status row while the key is derived | `statusText` |
| `that passphrase did not open the session file; the file was left as it was` | a wrong passphrase, kept on the box | `PASS_BAD_FILE` |
| `Session unlocked.` | the status after the right passphrase | `passAnswer` |
| `the session stays locked — :unlock asks again` | the status after `Esc` at the launch question | `passCancel` |
| `Change the session passphrase? (y/n)` | the change's confirmation, on the status row | `beginChange` |
| `the session is locked: :unlock first` | `:passphrase` on a locked session | `PASS_LOCKED` |
| `the two entries differ, so nothing was changed and the file was left as it was` | a repeat that differs, on the new-passphrase box | `PASS_DIFFER` |
| `Enter the same passphrase again.` | the box while the repeat is asked | `drawPass` |
| `The session passphrase has been changed.` | the success box's first sentence, with the reminder beneath it | `drawPass` |
| `Session passphrase changed.` | the success status | `PASS_CHANGED` |
| `Session passphrase not changed: the file was left as it was.` | the failure status | `PASS_NOT_CHANGED` |
| `the file could not be written (permission denied)` | the failure reason, set into the failure box | `PASS_WRITE_FAILED` |
| `the passphrase was not changed; nothing was written` | `Esc` during a change | `PASS_CANCEL_CHANGE` |

**`SESSION_PASSWORD_NEEDED` is not a refusal**: it is the answer that puts the password row up.
The three sentences marked ✱ are Telegram's own doing or the account's own doing, so they never
say "you"; nothing else in the sign-in view does either.

**No emoji in the chrome.** The emoji catalog is for the reader's *own text*, and
`emojis` costs 0.5 MB of binary and 2 MB of RSS. A `👤` in a settings title is
the one place that cost would be spent for nothing.

### Media placeholders

Decided: the seven tokens are **unchanged**. This section adds what the table above
could not say, the ink, and the constraints the row vocabulary already implies.

| `MediaKind` | Token | Carries | Ink |
| :---------- | :---- | :------ | :-- |
| `Photo` | `[image]` | a photo | `text-dim` |
| `Video` | `[video]` | a video | `text-dim` |
| `Gif` | `[gif]` | an animated GIF | `text-dim` |
| `Voice` | `[voice]` | a voice note | `text-dim` |
| `Sticker` | `[sticker]` | a static sticker | `text-dim` |
| `AnimatedSticker` | `[animated sticker]` | a TGS or WebM sticker | `text-dim` |
| `File` | `[file]` | a document, contact, or any kind the build does not model | `text-dim` |

The binary has six `MediaKind`s: it does not yet separate `AnimatedSticker`, and an
animated sticker draws `[file]` there. The row is the model's, ahead of the code.

**The ink is `text-dim`, not body `text`.** One role for all seven, no new token, and
no bold, italic or underline (there is no second weight or slant in this surface).

- *Why not body ink.* The token is the program's word for something the sender did
  **not** write. In body ink it is indistinguishable from a person who typed
  `[image]`, and a reader cannot tell a photo from a four-character message. Everything
  else the program says inside a conversation row is already `text-dim`: the
  `[you]`/`[them]` tag, a reply's `> quote ‖`, `[sending…]`, `[delivered]`, the time. A
  placeholder is the same kind of speech, so it takes the same role. `text-dim` is
  documented as "decoration, hints"; this widens it to "what the program says about a
  message", which is the reading the quote prefix already relies on.
- *Why not a new colour.* The palette has fifteen roles because the TUI has fifteen. A
  placeholder is not a state, a mode or a match, and a sixteenth would be fiction.
- *Contrast.* `#7b8496` on the dark ground is 5.17:1; `#6b7280` on a white light
  ground is 4.83:1. Both clear the 4.5:1 body floor, so the dimming costs no legibility.
- *The cost, stated.* A message with a placeholder is dim from tag to time, so a
  media-only row reads quieter than its text neighbours. That is intended: the message
  has no words to read. The tag and the token stay apart by position (the gutter is
  seven columns, the body starts at column nine).
- *A caption wins, and is body ink.* A captioned message shows the caption and nothing
  else; nothing about it is dim.

**Ink in each state** (the same rules as the quote prefix, which this follows):

| State | The placeholder |
| :---- | :-------------- |
| plain | `text-dim` |
| cursor row | the whole row is `selection` (reverse video); the token is `background` on `text`, like the tag. Never a dim reversed token |
| char/message selected (Visual) | the token takes the selection's own ink, `#d7dce5` on `selection-bg`. The dimming is lost under the highlight, as the quote's already is: the selection owns the cell |
| search match | `match` + bold on the matched characters; the rest stays dim. A placeholder is searchable (`/image` finds `[image]`) |
| beside `[sending…]` / `[failed: …]` | the status keeps its own ink and its own (last) row rule |

**Constraints** (each one is already true of the engine; this records them):

1. **The placeholder sits on the message's first row, or its only row.** It is the
   body, so it starts the body column on the row that carries `[you]`/`[them]` (when
   the message opens its group) and a reply's quote prefix. It never moves to a row of
   its own below a caption or a status.
2. **Soft-wrap rules are unchanged.** Rows break by cell width, at a space or after a
   ZWNJ (U+200C), in logical order. A token has no space and is seven cells at most, and the body panel is never
   narrower than `MIN_BODY_WIDTH` (8), so a token is **never split**: it is one atomic
   row. What can wrap is its neighbours: when the last row has no room for a trailing
   note, the note takes a row of its own below it (the `[failed: file exceeds the 2 GB
   limit]` scene), exactly as for text. The placeholder never takes a continuation
   row's place and never carries a note's wrap.
3. **The placeholder is selectable and yankable as text.** It is the message's `text`:
   `v` selects it, `yy` copies the literal string `[image]` into the register (the
   literal token, not a description), `r` quotes it as `> [image] ‖`, and `/` finds it.
   It is not drawn from a separate field, so none of this needs a second code path.
4. **A caption replaces the placeholder; the two are never on screen together.**
5. **The wording is not localised** and does not change with the kind of file (no
   filename, size or duration inside the brackets). That is a decision for another day,
   not this one.

**A download draws a token beside the placeholder, never in place of the body.** While
a file is moving, the message's row carries the kind's own placeholder with its
progress inside, in `text-dim` like every other thing the program says about a message:

| State | Token |
| :---- | :---- |
| in flight, size declared | `[image… 42%]` |
| in flight, no size declared | `[image… 2.0 MB]` |
| failed | `[failed: reason] (o retries)` |

The kind word follows the placeholder (`[video… 7%]`, `[file… 1.3 MB]`). The percentage
is capped at 100. The failure token's reason is truncated to the room and wins over the
hint: a row too narrow for both is clipped at its right edge, which takes `(o retries)`
and leaves the reason. The status line carries no percentage; see rank 8 under the
status ranking.

**A static sticker draws as a bounded block, not a token.** The decoded image is
scaled to fit a box 24 columns wide and 8 rows high, keeping its shape and never
stretching a smaller image up, and set left-aligned in the body column. Each
terminal row carries two rows of the picture as half-block cells (the graphics
protocol where the terminal speaks one, half-blocks otherwise). Eight rows is the
middle of the 6–10 acceptance band: the sticker reads at a glance and the
conversation around it stays on screen.

**The block is body ink, and takes no new role.** The picture is what the sender
sent, the way a caption is, so the scene paints it in `text` — the binary paints
true colours where the model paints `text`, and the decision the scene pins is
"content, not chrome". The `[sticker]` token underneath is the program's word for
the same thing and stays `text-dim`, like the other six.

**Ink in each state** (the block follows the message rules, row for row):

| State | The block |
| :---- | :-------- |
| plain | `text` |
| cursor row | every block row is `selection` (reverse video), tag cell included |
| message selected (Visual) | the block takes the selection's own ink; the range stays message-granular |
| search match | a `/sticker` hit can land the cursor on the message but paints no match marks on the picture |
| beside `[sending…]` / `[failed: …]` | the note keeps its own ink on the last block row, by the trailing-note rule |

**Whole-selection yanks the literal `[sticker]`.** There is no text on the block
to select character by character, so `vy` over the message yanks the token — the
same string a reply quotes (`> [sticker] ‖`) and the fallback row shows. A
captioned sticker shows the caption and no block, by the caption rule above.

**An animated sticker is a token, never a picture.** A TGS or a WebM is not
decoded inline: it draws `[animated sticker]` in `text-dim`, by the token rule, and
`o` opens its cached file in the viewer. Telegram's preview for one is an SVG
outline or a stripped JPEG, not a WebP, and neither is decoded here — so the
token is the representation, not a fallback from a missing picture.

**`[sticker]` is the static sticker's fallback.** Flag off, no bytes, or a failed
decode of a *static* sticker all draw the token row in `text-dim`, exactly like
`[image]`: the message's whole `text` is the token, so search, yank and reply need
no second path. An animated sticker has no inline block to fall back from; its
token is the only row it ever draws.

Scenes: `media` in the engine (start `media`, a chat put first by the scene), eight
frames at 80×24: `[image]` alone, `[voice]`, `[file]`, a caption suppressing the
placeholder, the wrapped-note row, a Visual selection over a placeholder, the yank,
and all five kinds in one window; plus `sticker` in the engine (starts `sticker`
and `stickerFallback`, the same chat put first by each), three frames at 80×24: the
inline block with the cursor on it, the Visual yank of the block, and the `[sticker]`
fallback token with the flag off or decoding failed.

### Jump to the quoted message

A motion, not a component: nothing is drawn that was not drawn before. The
cursor moves to the message a reply quotes; the window, the rows, the
`> quote ‖ body` prefix, the colours and the status line's ink are unchanged.

**The key is `gd`.** Normal mode in the conversation, on a message that carries a
quote.

- *Why `gd`.* It is Vim's "go to definition": from a use to the place it refers
  to, which is what a quote is. `g` is already a prefix key here (`gg`), so
  `gd` costs no new pending state and collides with none of `j k Ctrl-d Ctrl-u gg G
  v i a r e dd D / n N Esc Enter Tab h l A S`. `d` after `g` is not `dd`: the pending
  key decides, and `gd` is read the way `gg` is.
- *Runner-up: `gf`*, Vim's "go to the file under the cursor", which is closer to
  "follow" and has the same cost. It lost because a quote is a definition of
  context rather than a path, and because `gd` reads as a pair with `Ctrl-o`,
  which is how Vim teaches `gd` in the first place.
- Rejected: `Enter` (bound, and means "discard a draft" here), `'` and `` ` `` (marks,
  which this program does not have), and `K` (Vim's "look up", which a reader would
  expect to show the quote rather than move).

**Behaviour**

1. The target is a message **id**, never text or a row. A reply carries `to`; the
   quote shown is only what was said.
2. **Target in the window:** the cursor moves to it. Nothing is fetched and nothing
   else changes.
3. **Target not in the window:** the status line says
   `Jumping to the quoted message…`, a page around the target replaces the loaded
   window, and the cursor lands on the target, in the middle of the page
   (`Loading older…` / `Loading newer…` rows say which ends are still the server's).
   The cursor does not move until the page arrives, and the old window is kept until
   then. Any search is cleared, because its hits were rows of the old window.
4. **In flight, only `Esc` answers**: it drops the fetch and the reader is where they
   were. Every other key waits, as in the sign-in's *Checking…*.
5. **A message that is not a reply**, or a reply whose target the client does not
   hold at all, is a refusal written to the status line (`flash`, rank 7).

**Jumplist (Ctrl-o, Ctrl-i, Ctrl-n)** follows Vim: every `gd` leaves a mark on the
message the reader left, `Ctrl-o` goes back one mark, `Ctrl-i` goes forward, and a
new jump discards the marks ahead of the position. The first `Ctrl-o` from the end
of the list marks where the reader is, so `Ctrl-i` can return. A mark is a message
id, so it survives the window being replaced; going back to a message the window no
longer holds fetches it, and says `Jumping back…` (or `Jumping forward…`). Marks
are per conversation **and survive leaving it**: the list stays in memory across a
conversation switch, so `Ctrl-o` in the new chat walks back into the old one's marks
through the same fetch path. The list's depth is handed in by `app` and defaults to
100. Window-replacing jumps other than `gd` (day and date jumps) record the
cursor's message as the origin too, under the rule in *Day-boundary and date
jumps*; in-window moves never record one. `Ctrl-o` with no marks, and `Ctrl-i`
(`Ctrl-n`) at the end, do nothing. `Tab` stays the pane switch; no hint row
changes.

**Forward key decision (settled, Q-02).** `Ctrl-i` and `Tab` share a byte on many
terminals, so forward gets a second, unambiguous key: **`Ctrl-n`**. It walks the
jumplist forward exactly as `Ctrl-i` does, in every mode where `Ctrl-i` is bound;
`Ctrl-i` keeps working wherever the terminal reports it apart, so nothing changes
for readers whose terminal distinguishes the two. `Tab` is not rebound — it is the
pane switch in every context, and a key that is a motion in one place and a pane
switch in the next is a key a reader has to learn twice. `Ctrl-n` was chosen over
`Ctrl-f` (already a plain motion — opening the forward prompt is a different
surface, and Vim reserves `Ctrl-f` for paging) and over `Ctrl-]` (kept free for
future use): an audit of the normal-mode chord space shows `Ctrl-n` unbound, while
`Ctrl-d`/`Ctrl-u` page, `Ctrl-w` is a prefix, `Ctrl-c` preempts, and
`Ctrl-o`/`Ctrl-i` already own the jumplist. Where the terminal speaks the kitty
keyboard protocol or `modifyOtherKeys`, `Ctrl-i` suffices and `Ctrl-n` is simply a
second name for the same step.

**The label.** `JUMP_LABEL` (`Jumping to first unread…`) is not reused: it names a
destination, and this one is another. A reader who pressed `gd` and read "first
unread" would be told the program had done something else. The three new labels
follow its pattern, `Jumping to <where>…`, in the same ink as every other
status-line sentence (`text`), at the same rank (5).

| Label | When | Columns |
| :---- | :--- | ------: |
| `Jumping to the quoted message…` | `gd`, target not loaded | 31 |
| `Jumping back…` | `Ctrl-o`, mark not loaded | 14 |
| `Jumping forward…` | `Ctrl-i`, mark not loaded | 17 |

All are well inside the 71 columns the status line has. **No hint row changes**:
see the note under the hint table.

Scenes: `jump` in the engine, eight frames at 80×24 on Ada Lovelace's chat: the
loaded target, the fetch in flight, the page arrived with the cursor on the
target, `Ctrl-o` back (loaded), `Ctrl-i` forward, `Ctrl-o` needing a fetch, that
fetch landed, and `gd` on a message with no quote.

### Day-boundary and date jumps

A motion, not a new component: `[N]gg`, `[N]G` and `:date <ISO>` move the cursor to
the **first message of a target day**; separators are decoration and the cursor never
rests on one. Counting is relative to the day the cursor stands in: the day
separator nearest above the cursor is the first boundary, so `[N]gg` steps up across
N day boundaries and `[N]G` is the mirror across N upward/downward days. Bare `gg`
and `G` keep their old meanings (first unread, falling back to the top of the
window; newest message / bottom); only a numeric prefix makes them day jumps.
Absolute "Nth day of the conversation" indexing was considered and rejected as
harder to aim and un-Vim-like.

- *Landing rule.* Every day and date jump lands on the **first message of the target
  day**; the separator above it is kept in view by the usual rule, and nothing else
  changes (window, rows, colours, status ink).
- *Beyond the loaded window.* A day or date jump that needs history fetches through
  the date anchor (first message at or after the target day) and then lands per the
  rule; the page replaces the loaded window, and the cursor rests on the target in
  the middle of the page, with `Loading older…` / `Loading newer…` saying which ends
  are still the server's. Edges behave as in *Jump to the quoted message*:
  `Esc` drops the fetch in flight.
- *No such day.* When the server holds no message at or after the target day, the
  jump flashes the unavailable sentence rather than landing.
- *`:date <ISO>`.* Accepts `YYYY-MM-DD` only, read in the reader's current zone
  (the single-offset policy); it is a window-replacing jump and lands per the rule
  above. Bad input is a flash under the `run_command` precedent (STAGE-04).
- *Record rule (Q-04).* Day and date jumps are window-replacing jumps, so each
  records the cursor's message as the origin before it moves, exactly as `gd` does;
  in-window moves (plain `j`/`k`, paging) never record one, and a non-positive
  (placeholder) origin is never recorded (G-10). The result is that `Ctrl-o` after
  `:date` returns where the reader was.
- *Labels.* Follow the documented pattern `Jumping to <where>…`, in the same ink as
  every other status-line sentence (`text`), at rank 6. `<where>` uses the
  day-separator vocabulary, so the sentences are things like
  `Jumping to Sep 26, 2026…`, `Jumping to Yesterday…`, `Jumping to Today…`.
- *The typed count is visible.* While a count is being typed it sits on the
  conversation's title as `Conversation (n/m) · N`, straight after the cursor and
  total and before the selection and search notes. It is a bare number in `text`,
  with no noun, because the next key may clear it, and it is not a status-line
  rank: the status line keeps its ranks. A digit extends it, and a `0` only when a
  count is already open. The first `g` only arms the pending `gg`, so `2gg` keeps
  the count through both presses. `gg` and `G` read it, and any other key clears
  it. The count is the conversation's own, apart from the card's row count. The
  design model holds no day-boundary data, so it shows the armed count and clears
  it as those keys read it, and does not walk; the `Jumping to <day>…` label above
  is unchanged and is for a day that is not loaded. The `grouping` scene has one
  frame for it, `2` typed in Grace Hopper's chat.
- *Scenes.* `jump` covers the fetch-in-flight shape; the day/date variants add
  nothing new to draw (same page, same cursor position on the first message of a
  day), so no new frames are required — the label sentence is the new thing, and it
  shares the rank and ink.

## Starting a conversation with someone new

The chat list holds private conversations, and until now there was no way to
begin one: a person the reader had never messaged had no row, so `/` — which is
the conversation's find and only the conversation's — could not reach them.
This surface adds that second question without touching the first.

**Invocation.** `/` while the chat list has the focus, and `:new <query>` from
anywhere `:` reaches, which opens the same prompt already holding the query.
There is no new global key. Inside a conversation `/` keeps meaning *search the
open conversation*, because the reader who presses it is looking at messages,
and a key that answered "start a chat" there would answer a different question
from the one the screen is about. On the chat list `/` is the only question the
pane is about.

**The prompt.** A line like `Find` and `Command`: bar title `New chat`, the
shared `/ ` prefix on its first row, the reader's query in `text`, the caret in
the bar. The prefix is the one `App::prompt_prefix` gives the find line: a person
search and a message search are both queries, so they are told apart by the
status label, not by a second glyph. It is chrome: never permuted and never part
of the draft. There is no live narrowing: the query is sent on `⏎` and the
candidates arrive afterwards, so the prompt draws no list while it has the focus.

**The results list.** Drawn only after `⏎`, a transient overlay over the
chat-list column alone: a bordered box titled `New chat`, growing up from that
column's bottom and never taller than the column itself. Each row is the
`display name` at the left in body ink, the `@username` beside it in dim ink when
there is one — and nothing at all when there is not — and the standing at the
right in dim ink: `chat` when a conversation with that person already exists,
`new` when choosing them would create one. The fragment the reader typed is inked
with `match`: inside the display name for a name query, inside the handle for a
`@username` query. The highlighted row is reverse video; on it the quieter text
takes body ink, never dim, because a dim foreground under reverse video would
become a dim background. Rows are ordered closest match, then
no-conversation-first, then by name. `j`/`k` or `↑`/`↓` move the highlight and
wrap; `⏎` opens the highlighted person; `Esc` dismisses. Long names truncate at a
character boundary, and the standing is always shown.

**The status.** While the list is up the status line carries the query and the
request's state, just under a selection and above the conversation's own find,
in six forms: `/query — searching…` in flight, or `/query — N candidates —
searching…` when candidates are already known; `/query — N candidates` once the
answer lands; `/query — no candidates` when it was nobody; `/query — no
candidates (search failed: reason)` when the request itself failed; and
`/query — N candidates (search failed: reason)` when it failed with some. The
dash is an em dash and the ellipsis one character, as in the find sentence.

**What a choice does.** A query is a username when it opens with `@` and a
display name otherwise (a name query falls back to the username, so `gracer`
finds Grace Hopper). An exact username match, or a query with a single result,
opens at once, without a list; anything else is the list the reader walks. When
the chosen person already has a conversation, that conversation is focused — the
chat list does not grow a second row for the same person. When they have none, a
new, empty conversation is created at the top of the list and opened; its title
reads `Conversation (0/0)` and its body is empty, because there is nothing said
yet.

**Palette.** No new roles. The prompt and the overlay reuse exactly the roles
the find prompt and the completion already use: the highlighted result row is
reverse video, a matched substring inside that row is `match` on the reversed
row, the standing and the username are `text-dim`, the name is the body ink, and
the box is the ordinary border. A colour that would have to be invented to say
"this row is a new chat" is not used; the word `new` says it.

Scenes: `newchat` in the engine, seven frames: the prompt holding a query, the
query in flight after `⏎`, the candidates with one highlighted, no candidates, a
failed search, an exact username opening a new empty conversation, and an exact
username that focuses an existing conversation instead of duplicating it. A query
opening with `!` stands in for the failed search, so the failure sentence has a
frame to be seen in.

## Global message search: the five states after the list

The overlay's grammar does not change: 80 by 24, the box over the body, one rule
per chat, reverse video on the cursor, the hint on the box's bottom edge, the
status line for the state. This section adds what happens when a hit needs more
than one line, when the private-chat rule discards hits, when the query carries
filters, when the server says wait, and when the next page arrives by itself.

**Two lines per hit.** A hit draws up to two lines of its body, 62 cells each,
the second indented to the snippet's column. Time and `[you]`/`[them]` are on
the first line only, and that first line's time is what marks where a hit
starts. The window is chosen around the first match and each cut end is `…`. A
match is `match` ink in bold on either line. A message whose body fits one line
draws one row. The cursor reverses every row its hit occupies, because those
rows are one hit: a half-reversed hit would read as two rows to a reader moving
one hit at a time with `j`.

- **Why two lines.** One line showed the match and nothing around it, so the
  reader had to open the chat to learn whether the hit was the message they
  meant, which is the one cost the overlay exists to avoid. Two is the smallest
  number that shows a match and the sentence it sits in.
- **What it costs.** The 21 interior rows hold nine hits and three headers (19
  rows) where one line per hit held eighteen hits. Half as many hits fit on a
  screen, which is why the next page is fetched without a key, below, and why
  the count in the status matters more than it did.
- **Not two lines for the cursor row only.** A row's height would then depend on
  where the cursor was, and every `j` would relayout the list under the reader.

**Dropped hits.** `messages.searchGlobal` searches every chat and this client is
private correspondents, so a hit in a group or a channel cannot be opened here.
Those hits are discarded before the list is built and counted in the status, in
one comma clause after the count:

| Case | Status line, as drawn |
| :--- | :-------------------- |
| 9 kept, 3 dropped of 12 | `?tickets — 9 results in 3 chats, 3 dropped` |
| nothing kept, 4 dropped | `?tickets — no matches, 4 dropped` |

- **One clause, one place.** `, N dropped` is the same clause in both, so a
  reader learns it once. `no matches` is the overlay's existing sentence for an
  empty answer, unaltered; the clause after it explains an emptiness the reader
  did not expect.
- **`dropped`, not `filtered` or `hidden`.** The search never held these: they
  were discarded before the list existed. That is what `dropped` says and what
  `filtered` does not.

**Filters are typed into the same query.** `messages.searchGlobal` takes a media
`filter` and `min_date`/`max_date`, and no sender, so media and dates are the
only filters offered and there is no sender token. The reader writes them as
tokens in the prompt: `media:photo_video`, `since:2026-01-01`,
`until:2026-02-01`. Media values are `photo_video`, `document`, `music`,
`voice`, `url`.

- **The prompt is the box's last interior row:** two columns of indent, the
  query, the insert caret. The overlay owns rows 0 to 22, so the screen's own
  bar cannot hold a prompt while the box is up; the box draws it on its bottom
  interior row and keeps `Esc: close` on its bottom edge. The mode label reads
  `INSERT`, as it does for every line being typed. (Chosen here: the existing
  section says the prompt is the bar at the foot, and the geometry rules the
  screen's bar out.) The binary does not follow this: its prompt stays on the
  input bar, the row beside the body rather than a line inside the box, so the
  box needs no second line editor (`docs/decisions.md`, "Why the global-search
  prompt stays on the input bar"). The frames below draw the box's placement.
- **While typing, the status names the vocabulary.** The vocabulary is the one
  thing the frame cannot show and the one thing the reader needs at that moment:
  `Filters: photo_video, document, music, voice, url — since:, until:`
- **With filters active the tokens leave the echo** and are restated as a clause
  before the count, in the answer's reading rather than the prompt's grammar:
  the values are the reader's own words, the `media:`/`since:`/`until:` names
  are the prompt's: `?tickets — photo_video, 2026-01-01 to 2026-02-01 — 9
  results`. A one-sided range reads `from 2026-01-01` or `to 2026-02-01`.
- **With filters active the count loses `in N chats`.** The clause costs 37 of
  the line's 69 cells, and a date range makes the chat count the least
  interesting half of the count. (Chosen here; without filters the count keeps
  both numbers.)
- **A malformed token refuses the search without a request.** The prompt keeps
  the query so it can be fixed, and the flash names the token and one legal
  value: `media:photo is not a media filter: try photo_video.` An unknown value
  and an unparseable date take the same shape; a date before 1970 and a
  `from:`, `sender:` or `by:` token are refused too. A word with a colon that is
  not a filter name (`12:30`, a URL) stays free text. (Chosen
  here: one legal value is named, not five, because five do not fit beside the
  token in 69 cells.)

**`FLOOD_WAIT` is a pause, not a failure.** It is the only limit
`messages.searchGlobal` has, there is no daily quota, and what the server
returns is a number of seconds, not an error.

- **The overlay stays up and keeps its rows.** A reader who has results does not
  lose them to a wait: the rows stay, the cursor stays, and `j`/`k`, `{`/`}` and
  `⏎` still answer. Only the next request is withheld.
- **The status says paused and counts down** in whole seconds:
  `?tickets — 9 results in 3 chats, paused: retry in 42s`. With nothing held the
  clause stands alone, `?tickets — paused: retry in 42s`; the count is absent
  because there is no count.
- **The foot does not change.** With rows up it is the full hint, because every
  key on it still answers; with none it is `Esc: close`, as in every state with
  no list. A wait is not a reason to stop reading, so no word in the foot says
  "waiting": the status is the only place the wait is written.
- **The retry is automatic.** When the delay runs out the same query with the
  same filters is sent again; no key repeats it.

**The next page is fetched, not requested.** When the cursor reaches the last hit
and the server has more, the next page is requested and appended, grouped by
chat as the first page was, and the cursor stays on the row it was on: that row
does not move and the list grows under it.

- **In flight** the status says so and nothing else changes:
  `?gate — 7 results in 2 chats, loading more…`. No row is inserted for the
  fetch, because an inserted row would move the row the cursor is on, and the
  foot is unchanged.
- **Once the last page is in** the count is the full total and no "more" marker
  appears: `?gate — 9 results in 3 chats`. Nothing on the screen asks the reader
  to act, which is the point: a reader who pages by walking should never be told
  to press a key for the rest.
- **`100 of 412` is gone.** That state described a cap the reader had to live
  with; the cap is a page size now, and the list grows by itself.

**The status line's budget.** Every sentence above is measured against the 69
cells the line has after ` NORMAL `, its space and the connection dot. When one does not fit, the
query's free text yields first and is cut with `…`; the filters and the count
never yield, because they are the answer; when the clauses alone are too long,
their tail goes instead, so the pause is cut before the count. No hint string changes: the foot's
hint is the existing 53-cell row and `Esc: close` is its existing short form.

**What this replaces.** Two rows of the existing status table.
`search failed (flood wait 30s)` is not a failure and is drawn as the pause
above. `100 of 412 results in 3 chats` is a page size, not a cap the reader
meets. No other row changes.

**Palette.** No new roles. The second line is body ink and `match` as the first
is, the cursor row is reverse video on both of its rows, the prompt is body ink
with the insert caret, and the flash is body ink on the status line.

Frames: eleven in `global-search-states.html`, their rows in
`global-search-states-frames.js`: two for the two-line hit, two for the
dropped count, three for filters (the prompt, the filtered answer, a refused
token), two for the wait (with results and without), two for the page boundary
(in flight, and the last page in). They are drawn from these rules, not from
`televim-engine.js`; a `globalsearch` scene in the engine is the next step.

## Forwarding a message

A message one reader wants to show in another conversation. Forwarding is an
operation on a selection, so it starts from one, and the surface it adds is a
destination picker.

**Invocation.** `s` while a Visual selection is up raises the picker over that
selection, and the Visual hint names it: `s: forward`. `f` in Normal mode raises
the same picker over the message under the cursor, first making that message the
selection, so the picker sees the same thing either way. There is no global key
and no command: the conversation's own selection is the only place a forward can
begin, because the messages being sent are the ones on screen.

**The selection.** The reader reaches Visual the ordinary way (`v`, extended with
`j`/`k`, `gg`/`G`), and the pane's title counts what is held (`· 3 selected`).
`s` raises the picker and does not clear the selection, so `Esc` returns to
exactly the range the reader had.

**The picker.** A transient bordered box over the whole conversation column, the
rectangle the conversation draws in; the chat list beside it is untouched. Its
first interior row is the header, `Forward N messages to…`, where N is the number
of numbered messages the selection covers. Each chat is one row: a `📌` before the
title when the chat is pinned, the title, two spaces, and its newest message as a
preview in dim ink. A chat with no messages shows no preview. `j`/`k` or `↑`/`↓`
move the highlight and wrap at either end. The highlighted row is reverse video.
`⏎` queues the forward into the highlighted chat. `Esc` puts the picker away and
returns to the selection it was raised from, unchanged. A deleted account is not
a row: the picker leaves it out, though the chat list still shows it.

**What a send does.** `⏎` queues the forward and ends the selection: the picker
goes, the mode returns to Normal, and the reader stays in the conversation they
were in. **The open chat does not change.** The forward stays in the current chat;
the destination is not opened, and the outcome is reported on the status line
when the network answers (`Forwarded N message(s) to …`). Choosing the
conversation the reader is already in forwards to itself, which is the same
operation and needs no special case.

**Refusals.** A selection with nothing forwardable (only placeholders) is refused
with the reason on the status line, and the selection clears to Normal, as a
deletion's refusal does. So is a list with no chat to forward into. A source chat
that Telegram content-protects refuses the forward on the wire; the status line
says `<chat> does not allow forwarding`. Nothing checks for that before the send.

**Palette.** No new roles. The header, the title and the `📌` are the body ink, the
preview is dim, the highlighted row is reverse video, and the box is the ordinary
border.

Scenes: `forward` in the engine, two frames: a Visual selection of three messages
with the forward key named in the hint, and the picker raised over the
conversation, with its header, a pinned chat, and a preview.

## Chat and contact actions

Six actions share one surface: the `:` line. They add no key to the chat list,
whose hint is already 69 of 71 columns. The contact card is the one place that
gains a key, `d`, because three actions have no other home.

| Command | Acts on | Asks first? | Sentence |
| :------ | :------ | :---------- | :------- |
| `:archive` | the highlighted chat | no — `:archived` undoes it | `archived {name}` |
| `:archived` | — | no | lists the archived chats; `⏎` unarchives |
| `:block` / `:unblock` | the card's person, else the highlighted chat | yes | `Block {name}? (y/n)` / `Unblock {name}? (y/n)` |
| `:rename [name]` | the card's person, else the highlighted chat | no — bare `:rename` clears | `renamed {name}` / `local name cleared` |
| `:delete-chat [both]` | the highlighted chat | yes | `Delete this conversation? (y/n)` or `… for both sides?` |
| `:report [reason]` | the card's person, else the highlighted chat | yes, naming the reason | `Report {name} to Telegram as {reason}? (y/n)` |
| `:contacts` | — | no | lists the account's contacts; `⏎` opens the card |

**Confirmation is for what cannot be undone from the screen.** Block, unblock,
delete and report ask; archive and rename do not, because `:archived` brings the
chat back and bare `:rename` restores the peer's name. A confirmation captures
its subject when it is raised, so `y` cannot act on a different row, and `n` or
`Esc` changes nothing. The prompt is the status line; the bar is empty; the mode
label is `CONFIRM` in the one red role. The archive and rename results are
`flash` sentences, not prompts.

**The subject rule is one rule.** A contact card on show wins; otherwise the
chat-list highlight is the subject, and opening a conversation moves that
highlight to it. `:archive` and `:delete-chat` need a chat, so a card over a
person with no conversation is refused with `nothing to act on: highlight a
chat, open a conversation, or open a card`. The line opens over the card, so
the card stays the subject until the command has run. On a contact's card `d`
acts only on `rename`, `block`/`unblock` and `report`; on a value row it is
dropped without a sentence.

**Archive is a place, not a deletion.** The chat leaves the visible column and
is kept in the archive folder. `:archived` draws a small bordered list over the
chat column, title `Archived · {n}`, the selected row in reverse video; `j`/`k`
walk it, `⏎` unarchives and puts the chat back in the list's own order, and `Esc`
closes it. When the last row leaves, the overlay closes with it. If the open
conversation is archived, the conversation closes, because it is no longer in
the list that owns it.

**A local name is a name over the peer's, not a replacement of it.** The chat
row, the message bar, the card and the contacts list all draw the local name;
the peer's own name is what a bare `:rename` brings back. The name is
account-scoped and stored with the history file, so it survives a restart and a
refetch and goes when the reader signs out. A save that cannot be written says
`rename: could not save — {reason}` and leaves the local name on screen until
the next launch, which is the honest state: the screen has it, the file does not.

**Block is a property of the person, not a second list.** The card's action row
flips between `block` and `unblock` from the profile read; the chat stays where
it is and the card is the place the state is visible. The status sentence names
the result. There is no blocked badge and no blocked-people screen.

**Delete has two depths.** `:delete-chat` asks the local question and removes the
row, the open conversation, the cached messages, the local name, the draft and
the per-chat media keys. `:delete-chat both` asks the both-sides question and
sends the revoke form; the local cleanup happens only when Telegram answers. A
failure changes nothing and says `delete chat: {reason}`.

**Report carries a reason the reader can read.** The ten tokens are `spam`,
`violence`, `pornography`, `child-abuse`, `other`, `copyright`,
`geo-irrelevant`, `fake`, `illegal-drugs` and `personal-details`; absent means
`other`. `child-abuse` is drawn in the prompt as `child abuse`, because the
hyphen is the wire's spelling. An unknown token is refused before the question:
`unknown report reason: {token} — use spam, violence, …`.

**The contacts list is the directory, not a search.** It draws over the chat
column, title `Contacts · {n}`, one row per person — the local name, `@username`
when there is one, and `chat`/`new` only when the fact is already in hand. The
server's order is kept; there is no sort, no query and no second cache. `⏎`
opens the person's card even when no conversation exists, which is the reason
the list is separate from the chat list; the card's own `⏎` opens a conversation.
`Esc` closes the list. The cursor stops at either end. A failed fetch leaves the
previous list and says `contacts: {reason}`; one in flight says
`contacts — searching…`. Every key the list does not use passes through, so `:`
still opens the line and `Tab` still moves the focus; `:archived` closes the
contacts list, and `:contacts` closes the archive.

**The status line ranks the open list above selection and search.** The confirm
prompt still outranks everything. `contacts — {n}` and `archived — {n}` are the
list labels; a failure is the same line with its reason, so the reader never
loses the place they were in.

Scenes: `manage` in the engine, twenty-one frames, each reached by the keys a
reader presses: archive and unarchive, the block and unblock questions, a contact
card with the three action rows, rename from the card and from the command line,
both delete questions, a report naming its reason, an unknown-reason refusal, and
the contacts list with a person who has no chat.

**The model is behind the build in four places**, recorded rather than hidden:
the model's `archiveChat` leaves the conversation open where the build closes it;
its unarchive splices the chat back at the index it remembered where the build
puts it in the list's own order; an empty `:archived` opens in the model where
the build refuses it with `archived — 0`; and while a list is up the model
draws its key help (`j/k: contact  ⏎: card  Esc: close`, and the archive's
equivalent) on the status row where the build draws the label, `contacts — {n}`
or `archived — {n}`, because an open list owns that row. The frames show the
model's behaviour in those four, and the build's is the one this document
describes.

## Components

Ten, with their states, and the `TestBackend` assertion that would catch each
regressing.

1. **Panel** — `Chats`, `Conversation`, `Input`. States: focused, unfocused,
   empty, titled with a count, titled with notes, titled with the peer's typing
   note (see *Peer typing*).
2. **Message row** — states: incoming, outgoing, reply, sending, failed, search
   match, char-selected, message-selected, wrapped across rows, a reply whose
   quote has been jumped to (nothing changes on the row; the cursor moves). Also: first or
   later in a group (the label on the first only), last of a group (the time, and
   for an outgoing one `[delivered]` or `[read]` in place of nothing, `[sending…]`
   or `[failed: …]` in place of both), and the **day separator** beside it, a row
   that is not a message.
3. **Mode label** — the four above, plus the line's own `VISUAL` and `NORMAL`,
   which are *different modes that share a word*. The label belongs to whichever
   mode the keys about to be pressed will mean, and a confirmation is a question
   about the whole screen.
4. **Hint row** — nineteen variants, one per state of the screen, all under
   `ASSUMED_WIDTH - MODE_LABEL_WIDTH`. Three keys mean something else while a
   `:shortcode` completion is up, and the status line names them for as long as
   it is. The settings hint and the card hints lose to a confirmation and to an
   active search, the same way the other hints do.
5. **Prompt** — `Command`, `Find`, `Reply`, `Edit`, `New chat`, `Passphrase`, `New passphrase`,
   `Repeat passphrase`, and the bar's own title switching between `Message to …` and ` draft `.
   The three passphrase titles are concealed: every character draws as `•`, and the text
   never appears on the bar, in the box, or in the status row.
6. **Status line** — the ranking, which is the whole of it: a **confirmation**
   outranks a **selection** outranks a **search**, and a `flash` is not a state
   at all. A refusal written while any of the three is up is a line the reader
   never sees. That is why an operation that finishes in Visual leaves Visual,
   and why a prompt carries its counts rather than flashing them. Below the three
   sit a jump in flight, the full reason a failed message failed, then
   whatever was last written to the status, and last a cached conversation's
   wait for its newest page, which outranks only the resting hint.

   **Above all of them is a keystroke inside the line — all but a sign-in
   refusal, which outranks even the hint.** A key being pressed
   cannot be answered by a sentence about a state the reader is in the middle of
   changing, so the hint wins; a refusal is Telegram's answer about the value
   just typed, and a reader who cannot see it types the same one again. The
   full order, highest first:

   | Rank | What it says | Example |
   | ---: | :----------- | :------ |
   | 0 | a sign-in refusal — above even the line's hint | `that code is not the one Telegram sent` |
   | 1 | a key inside the line — the line's own hint | ` i/a: ins  w/b/e  …` |
   | 2 | a confirmation | `Quit televim? (y/n)` |
   | 3 | a selection, **with its unit** | `2 message(s) selected — Esc clears`, or `3 row(s) selected — Esc clears` on a card |
   | 4 | the new-chat list, while it is up | `/ad — searching…`, `/ad — 2 candidates` |
   | 5 | an active search | `/bench — match 1 of 2` |
   | 6 | a jump in flight | `Jumping to first unread…`, `Jumping to the quoted message…` |
   | 7 | why the message under the cursor failed | the send's own reason |
   | 8 | a `flash` — a refusal, a status worth reading; also the persistent sentence while a download runs, with no percentage | anything just written, or `downloading media…` |
   | 9 | a cached conversation waiting for its newest page | `Cached messages — loading the latest…` |
   | 10 | the hint, or `televim` | the resting state. The editable profile and both cards use their own hint here, not `televim` |

   Beside the ranking, always drawn: a `●` in the connection's colour — green
   while the feed delivers, yellow while a bring-up or a rebuild is under way,
   red once the budget is spent. It is not a rank, so whatever sentence wins
   above still says what happened in words, and the dot still says which of
   the four holds. A selection or a confirmation hiding the retry sentence
   hides nothing the dot was saying.

   Rank 6 is one rank with six sentences now: first unread, the quoted message,
   back, forward, a day or date jump, and an unloaded search match. A jump in
   flight is one kind of state and the sentence names where it is going; see
   *Jump to the quoted message* and *Day-boundary and date jumps*. A search above
   it hides the label, which is the ranking working: the reader who pressed `gd`
   with a search up still sees the search until the page lands.

   Rank 9 sits below everything written to the status so that a cache can never
   talk over a failure: a revalidation that fails says `history:` like any other
   page, and `offline:` keeps the line it always had. It is said only while both
   halves hold — the window came from the cache, and its newest page is in
   flight — so an offline reader looking at cached rows is told `offline:`, not
   that something is coming.

   The unit in rank 3 is not decoration: three characters, three rows and three
   messages are all "3", and a reader who has just pressed `v` has to be able to
   tell which of the three they are holding.

7. **Profile card** — one widget over two subjects, drawn in the right column.
   States: the self card, a contact's card, the selection row, the inline position,
   a charwise selection, a selection of rows, a value wrapped over rows, a row
   the peer did not give (absent, not empty), a dim action row, a reserved slot
   drawn as nothing, and the four shell states (not signed in, nothing read yet, and
   the contact's not-read-yet and read-failed).
   What would catch it regressing: the selection row is reverse video and the
   normal caret *on it* is the ground colour, so the reader can always see where
   the cursor is; and the title's `(n/m)` counts the rows the peer gave, so a
   row that disappears when a privacy setting hides it changes the count.

8. **Sign in** — the one view that is not the chat: a box over the right column, two rows, and a
   third only when Telegram asks for it. States: the phone (pre-filled from configuration when
   one is set), `Checking…` (a request in flight, `⏎` refused once and a second `⏎` doing
   nothing), the login code, a refused code (`[wrong code]`), the two-factor password with its
   `(n attempts left)` counter, a refused password, the cancelled code (`Esc`), the paused flow
   after `Tab`/`Ctrl+w` (the step survives and the code does not, and the return says so), and
   the stale session whose row offers `[ ⏎: sign in again ]`. A no-2FA account has two rows for
   the whole flow: the password row is never drawn speculatively. When the machine has no
   `api_id`/`api_hash` the view is not a form at all — it is a sentence and there is no phone
   row, because a field the reader cannot use is worse than no field. What would catch it
   regressing: the password row is absent at steps 0 and 1, and the `(n attempts left)` on it is
   the same three the refusals decrement.

9. **New-chat results** — the transient list over the chat-list column that the
   new-chat prompt raises. Drawn only after `⏎` has sent the query; never while
   the prompt line has the focus. States: the query in flight, no candidates, a
   failed search, several rows, one row standing for an existing conversation
   (`chat`), a row standing for a person with none (`new`), the highlighted row,
   and a matched substring inside it. Not a panel and not a mode: it is an overlay
   dismissed by `Esc`, and the conversation behind it is unchanged. What would
   catch it regressing: the box never leaves the chat-list column, and the
   highlighted row is reverse video with its quieter text in body ink.

10. **Forward picker** — the transient list `s` in Visual, or `f` in Normal,
    raises over the conversation column it forwards from. Drawn over the
    conversation, never over the chat list. States: several chats, a pinned chat's
    `📌` marker, a last-message preview, the highlighted row, and the header naming
    how many messages are being sent. Not a panel and not a mode: it is an overlay
    dismissed by `Esc`, which returns to the selection it was raised from. What
    would catch it regressing: the header counts the selection, the box never
    leaves the conversation column, and the highlighted row is reverse video.

11. **Session passphrase at launch** — the question asked when the session file is
    encrypted and no key is available from `TELEVIM_SESSION_PASSPHRASE`, the config's
    `session_passphrase`, or the OS keyring. A box over the right column, the same shape as
    the sign-in box, and the bar below it titled `Passphrase`, concealed. The box's first
    line is the one sentence that says what is asked and why: *the session file is
    encrypted and no key is available, so televim needs the passphrase to read its chats.*
    States: the question over the cached list (the list's title carries the note `· cached`),
    the same question over an empty list (`Chats (0)`, no note), a first `⏎` refused while
    `Checking…` stands, a refusal kept on the box (`that passphrase did not open the session
    file; the file was left as it was`), and unlocked (the box goes, the list is live, the
    status reads `Session unlocked.`). `Esc` leaves at once, the focus returns to the pane it
    came from, and the session stays locked: the status says `the session stays locked —
    :unlock asks again`. The two panes keep their widths, and the box is never a mode. The
    passphrase line has no Normal mode, because a Normal-mode key could move a secret into view.
    What would catch it regressing: the passphrase never appears in clear on any row, and the
    cached list is still drawn beneath the box.

12. **Change the session passphrase** — `:passphrase` on an open session, and only then.
    The first step is the screen-wide y/n row the quit confirmation uses:
    `Change the session passphrase? (y/n)`, on the status row at the confirmation's rank. `n`
    and `Esc` leave the file untouched. After `y` the new passphrase is entered concealed, and
    then again: a typo here would lock the file under a key nobody can recall, so the repeat is
    the design's own requirement, not an option. A repeat that differs changes nothing and asks
    the new passphrase again from the start, with `the two entries differ, so nothing was
    changed and the file was left as it was`. Success: the status reads `Session passphrase
    changed.` and the box says what to update, `TELEVIM_SESSION_PASSPHRASE` or
    `session_passphrase`, if the old passphrase was set there, or the next launch will not
    open the file. Failure: the status reads `Session passphrase not changed: the file was left
    as it was.` and the box names the reason. A locked session refuses with `the session is
    locked: :unlock first`. `:passphrase` and `:unlock` are in the shipped command set:
    `run_command` dispatches them to `run_passphrase` and `run_unlock`. What would
    catch it regressing: the change is never reached without the y/n row, and no
    failure path writes the file.

## Profile

**A profile row is the same kind of object a message is.** That is the whole of
this screen, and it is not a new idea: it is the conversation's model,
generalised. The conversation already has a cursor on an item, a charwise
selection inside one item, `v` to select, `y` to yank, `j`/`k` between items,
`gg`/`G` the ends, and `d` to delete. The profile panel is that widget over
different items, so one interaction model serves a chat, a settings card and a
person.

What differs between "my profile" and "someone else's profile" is then a
**capability** question, not a layout question: *does this row have a home on this
card?* The self card has two rows that act; a contact's card has the three
actions that have no home in a conversation or a chat-list key — rename, block
and report. Reply, search and delete stay in the conversation, so the card does
not repeat them. Nothing else about the two cards differs, and neither is a new
widget.

### The row

A row is one of three kinds. Every row carries the same one-column cue on its
first line — `·`, present, dim, and blank on a wrapped row's continuation lines.
It is the device the conversation already uses when it puts `[you]` or `[them]`
on a message's first row: a mark on the row it belongs to, in the row's own
columns. The conversation writes a word; a card writes one cell.

| Kind | Draws | Answers |
| :--- | :---- | :------ |
| value | the label in `text-dim`, the value in `text`, wrapped at the value column | `h`/`l` inline, `v`, `y`, `yy` |
| action | the label alone, dim | `d` on either card; the contact rows are `rename`, `block`/`unblock` and `report` |
| reserved | nothing | nothing: the colour slot, held and not drawn |

**A row exists only when the peer says something.** An empty bio, a missing
username and a birthday their privacy hides are all one case: the row is absent,
not empty. That is why a contact's birthday row is usually missing, and why the
panel must never draw a label with nothing beside it — an empty field and an
absent field would look the same, and only one of them is true.

**The bio wraps and is never truncated with an ellipsis.** A value that does not
fit the panel is the panel's problem, and an ellipsis on a value the reader came
to read is a lie about how much of it there is. The same rule governs the reason
on the signed-out card, which is a chain and is routinely longer than the panel.

**A row's inline position is visible, always.** The selection row is reverse video,
the way a message's is, and the normal caret on it is drawn in the
ground colour so that it can be seen against that reverse. A position that moves
inside a row and cannot be seen is one nobody knows.

### The self card

Rows, in order: `name`; `username`; `bio`; `birthday`; `id`; `session`; then
`add account`, dim, which refuses, and `logout`, which signs out.

- `username` is the row's label, as in the binary's card; its value is the joined
  identity, `@username` and the phone number with ` · `
  (`@noorh · +44 7700 900142`). The identity is one thing and one row, so the handle
  and the phone share this one row and there is no `phone` row. With neither, it
  reads `not set`. A half-filled identity shows the half it has.
- `bio` and `birthday` are drawn only when present.
- `id` is the account's numeric id, written after the label.
- `session` says where the session is kept: `OS keyring`. Had the model placed it
  in a file it would read `encrypted file <path>`.
- `birthday` is written as the month, day and year, then the age in
  parentheses: `Oct 19, 2001 (24 years old)`. **Without the year when there is
  no year** — `10 December` — because a year is a disclosure the account was
  not required to make, and a birthday with no year has no age to state.
- `add account` refuses with `not yet: this build cannot add an account`.
- `logout` raises a screen-wide confirmation, `Sign out and forget this
  session? (y/n)`, and **confirms before it throws away the only secret the
  program holds** — a panel that only flashed would have taught the reader the
  wrong thing about that key. `y` queues the sign-out; the session is discarded,
  the list empties, and the sign-in field comes back for the phone. The
  confirmation outranks a transient status and both are the same row.

### A contact's card

Rows, in order: `name`; the reserved colour slot; `username`; `bio`; `birthday`,
each only when the peer gives one; then the three action rows `rename`,
`block`/`unblock` and `report`. **No phone**: a client cannot ask a contact
for their number. No `add account`, no `logout`, no birthday-forcing. The colour
slot sits immediately after `name` and not lower, because it is the name's own
property — the thing that backs the name in the chat list and inks `[them]` in a
conversation — so it belongs beside the row it qualifies, not under the rows that
merely describe the person.

A person cannot be replied to, searched for, or deleted *from their own card* —
all three already exist in the conversation, and duplicating them is worse than
not having them. The three rows that do act are the ones with no other home:
`rename` sets a local name, `block`/`unblock` changes whether the person can
message the reader, and `report` sends a reason to Telegram. `d` runs the row
under the cursor; `e` still refuses because nothing here is editable. `y` yanks
the character selection; `yy` yanks the row's whole value into the register and
then offers it to the system clipboard by OSC 52, the same two-part rule the
conversation's `y` already follows.

A rename is local: the card's `name` row draws the reader's name, the peer's own
name is what the row replaces, and a bare `:rename` puts the peer's name back.
The card is the one place both are visible at once, which is why the action lives
here rather than only on the command line.

A contact's card has no rows at all until the profile is read. It does not open
on a half-filled name row: the shell sentence replaces the card. Before the read
there is one dim line, `reading their profile…`. If the read failed, the card
says `could not read this profile` and wraps the reason under it in dim text.
There is deliberately no `Set the credentials again with :signin.` line: a
contact's problem is never credentials, so naming `:signin` would send the
reader to check something that is not the problem. The account's signed-out
shell and the contact's read-failed shell never borrow each other's wording.
Both shells still name the subject in the title, because the subject is named
even, and especially, when there are no rows to show.

### The reserved colour row

The program will one day let the reader give each person a colour, shown as a
background behind their name in the chat list and as the indicator ink on
`[them]` inside a conversation, because there are no profile pictures and colour
is the only identity signal available. **The row is reserved, and it is not
designed here.** Its slot is held between the name and the username, it draws
nothing, and nothing sets it. The constraints it will have to satisfy, stated now
because they are constraints and not choices:

1. **A fixed set of named colours, never a free picker.** This is a sixteen-colour
   terminal, and a reader-chosen background behind body text is illegible for
   anything but near-black. A free picker is a contrast-bug generator, and a set
   this program names is the only form the set can be checked in: a picker has no
   floor to test against, and a named colour has one value to hold against it.
2. **A contrast floor against the body text, and the name stays in the body
   text.** Colour only ever reinforces the name, it never carries it. Colour alone
   is not an accessible identity channel, and a card whose name *is* the colour
   fails the moment the terminal cannot render it. That is also why
   `Color::Indexed` was rejected for the palette above: on the terminal this
   program hands a name to, the index is the one form that renders as something
   other than what was chosen, and a colour that carries meaning cannot be the form
   that degrades.
3. **One stated winner between the person's colour and the chat list selection
   row's background, and the winner is the selection row.** The selection row
   already carries a background, reverse video, and reverse video belongs to the
   cursor alone. If the person's colour took that row's background, the reader
   would no longer be able to tell which row the cursor is on. So the colour is
   not drawn on the selection row at all, and it returns the moment the cursor
   moves. The winner is decided here, once, rather than row by row.
4. **It is local state this program owns**, keyed by the peer's identifier and
   persisted in its own file. Telegram has no field for it.
5. **It arrives with a light value as well as a dark one, like every other role.**
   Every colour in the palette table above is specified twice, once per terminal,
   because a value that is right on one is often invisible on the other. A peer
   colour is the role likeliest to be chosen on a dark terminal and then read on a
   light one, so it needs the same pair or it becomes the single thing in the
   interface with no light-terminal answer.

### Entry and exit

Every key, with the reason beside it, judged against what Vim itself does.

| Key | Does | Reason |
| :-- | :--- | :----- |
| `A` | opens the *contact's* card, from the conversation and from the chat list | `A` is unbound everywhere else in this program, and it reads as "about". Vim's `A` is insert-at-end and has nothing to say about a card. |
| `l` | from the conversation, opens the contact's card | the right-hand column holds two contents, and `l` is the one that means "the other one" |
| `S` | opens **your own** profile, from either pane | the key the account panel always had; the panel changed shape, the key did not |
| `h` | goes back — **but `h` is also inline motion** | at a card row's first cell the inline position has nowhere left to move, so `h` takes its other meaning there. Vim leaves a motion at the buffer edge doing nothing; a card is not a buffer, and a reader with no way back has a stuck screen. Everywhere else on the card it is a motion. |
| `Esc` | the way back, and the only one that needs no argument | `<Esc>` leaves a mode in Vim; here it leaves the card |
| `Tab` | walks the panes in the order they are drawn, as it already does | unchanged |
| `Ctrl-w h` / `Ctrl-w l` | pane navigation | because `h`/`l` are inline. Bare `Ctrl-w` keeps its existing meaning, leaving the input line: a prefix key with a bare fallback, the pattern `g`/`gg` already uses in this program. |
| `j` `k` `gg` `G` | between rows, plus `{count}j` | counts cost nothing: the digits are otherwise unbound on a card |
| `h` `l` | **inline**, within the row's text | exactly as a message moves within its text. The value is one logical line, so these are single-cell motions and the wrap is only how it is drawn. |
| `v` | charwise selection, extended by `j`/`k` to become a selection of *rows* | a set, not a range of characters: the same distinction the conversation makes |
| `y` / `yy` | the selection, or the row's whole value | `y` is an operator and `yy` is linewise, which is what Vim means by them. With a selection up, `y` acts at once, as it does in Visual. |
| `d` | acts on an action row: the self card's two, or a contact's `rename`, `block`/`unblock` and `report` | `d` is the conversation's act; here it acts on the row the cursor is on, and a value row is not an action |
| `e` | refuses | the editable profile is a separate screen, and the refusal names it: `:settings` |
| `p` | **unbound, and says why in the hint area** | there is no buffer in a card to paste into, and a bound key that cannot work is worse than an absent one. The reason is written to the status row, which is the row the hint is on. |
| `zz` `zt` `zb` | no-ops | a card cannot scroll, and pretending it can is a lie about state. The key is dropped rather than answered with motion that moves nothing. |

Two decisions about the screen's shape, stated beside the `h`-is-both rule because
they are the same kind of thing — what a key means changes when the screen changes
shape under it:

- **`Ctrl-w l` does nothing, and that is a statement about the screen.** A card is
  the last thing drawn in the right column: there is nothing to its right to move
  to. `Ctrl-w l` is therefore dropped rather than answered with motion that moves
  nothing, exactly as `zz`/`zt`/`zb` are on a card that cannot scroll. Bare
  `Ctrl-w` keeps the meaning it has everywhere else in this program — it leaves
  the input line — and it is a prefix with a bare fallback for the reason
  `g`/`gg` is one: `Ctrl-w h` walks the panes, and any other follower makes
  `Ctrl-w` mean what it always meant.
- **`/` on a card hands the search to the conversation.** A search whose scope
  depends on which pane the reader happens to be standing in is a search the
  reader has to stop and think about, and a card is a handful of rows — seven at
  its longest — with nothing in it worth a second scope. So `/` is the
  conversation's key wherever it is pressed: a card borrows the conversation's
  search rather than growing one of its own, and the hits are drawn where the
  search lives.

Two rules that are easy to get backwards:

- **The conversation keeps its message cursor across a round trip; the card starts
  at the top every time.** `l`, then back, then `l` must land the card on its
  first row. One is a document and the reader's place in it matters; the other is
  a card of fixed shape. The scenes `ljjjh` and `ljjjhl` show it: the first lands
  in the conversation at `(6/16)`, exactly where the reader left it, and the
  second lands back on the card's first row.
- **The `Esc` ladder is the same ladder as everywhere else, and the card is not a
  stack.** Nothing being edited: one press closes the card. A line open over it:
  four rungs, one `Esc` each — insert, the line's Normal, the card, closed. The
  card remembers nothing between openings, which is why it is not a stack.

### Transport

Both cards are the same request. `users.getFullUser` takes any `InputUser`: your
own is `UserSelf`, a contact's is `User(id, access_hash)`, and the
`access_hash` is already in the peer cache the chat list fills. The shared rule is
the row rule above — a row exists only when the peer says something — which is why
a contact's birthday row is absent whenever their privacy hides it, and why the
same rule governs an empty bio and a missing username. The card's own write is
the block flag, read from the same answer; rename is local and never reaches the
wire, and report is its own request with its own confirmation. Every write is the
escape hatch, and it forwards straight to the protocol layer.

### The shell cards

Before either card there is the state a reader on a machine with no credentials
sees, and it is the easiest one to get wrong: an empty panel is indistinguishable
from a widget that has gone wrong. So the panel is never empty. Its title is
`Profile · you`, and then, on its own line, `not signed in`, then the reason — wrapped, never clipped,
because a bring-up failure carries a chain and a reason cut at the edge is a
reason the reader cannot act on — then a line saying where to set the credentials:
`Set the credentials again with :signin.`

There is a third state, **before anything has been read**, and it must not borrow
the other one's wording: the title is again `Profile · you`, and the body is one
dim line, `reading the account…`. It does not say the session is missing, because
nothing has established that yet. Its hint names `q` only, and the signed-out
card's hint names `::signin` and `q`: the shell cards are the whole program, so
there is nothing behind them to go back to and `Esc` has nowhere to land.

The contact's shells are not the account's. A contact's card always shows
`CARD_CONTACT_HINT` in every state, known, unfetched, or unavailable, because it
is a card with a way back: the chat list is still there behind it and `Esc`
lands on a row. Only the account's shells, which have no list behind them,
borrow the `q` hint. And because a contact's card can have no rows at all, its
title takes the subject-name form: the subject is named even, especially, when
there is nothing else to name. The account's shells take it too: the title is
`Profile · you` with no count, because the count appears only when rows exist.

## Principles

Six, each argued for in a code comment or an `AGENTS.md` section. Not
aspirational: each is a decision the code makes and a test asserts.

1. **Refuse rather than answer a different question.** A key that answered a
   different question than the one asked, while the screen said something else,
   is worse than a refusal. `r` in Visual says no, in two distinct wordings,
   because its two reasons are genuinely different.
2. **Nothing typed is ever lost to an `Esc`.** Two stages: stop typing, then
   leave. `Ctrl+w` steps back out of the line from any of its modes without
   throwing anything away.
3. **A refusal says why, and an unhandled key is *dropped*, not refused.** A
   dropped key is silence, and silence is the one bug this rule exists to
   prevent: `gg` and `G` were dropped by the editor before they were
   implemented, and they arrived as nothing happening.
4. **A screen that cannot draw cannot ask.** `Ctrl-C` does **not** confirm — it
   is the way out when the program is wedged, and a terminal that is not
   answering cannot draw the question either.
5. **The honest fallback beats the pretty one.** No cache, no second source of
   truth, no layout computed twice. A frame is built from the current window or
   it is not built.
6. **The lightest version that works.** The runtime is current-thread, chosen
   explicitly even though `tokio`'s `full` features are on. The global allocator
   is the system allocator, chosen on measured margins and not by default — and
   the declaration that once implied otherwise is gone. Declared-but-unused is
   named as a gap, never counted as a feature.

## Peer typing

A signal that the other person in the open private conversation is typing. It is
not a message and not a row: the body of the conversation is the record, and a
line that appears and then goes would be a row that is not one.

**Surface: the conversation's title.** ` Conversation (3/48) · typing `. The
status line was rejected, for three reasons. It is already the highest-ranked
thing the reader is told while composing (rank 1, the line's own hint), which is
exactly when a typing signal is most wanted, so it would be hidden when it
matters. It is the reader's channel for their own state: a selection, a search,
a jump, a refusal each outrank it, so it would also lose to every state the
reader can be in. And adding a ninth rank to a table whose point is that it is
short would make "typing" compete with confirmations. The title belongs to the
panel the signal is about, is drawn in every state, and is not ranked against
anything: the ranking table is unchanged.

**Ink: `text-dim`.** The count notes (`· 2 selected`, `· 3 match(es)`) stay in
`text`, because they are facts about the reader's own view. This one is the
peer's state, furniture that comes and goes, so the whole note (joiner and word)
is `text-dim`. No new token. There is no ellipsis and no cursor glyph: an
ellipsis here would promise motion, and `…` already means work in flight.

**Order and room.** The note comes after the count and any selection and search
notes. When the title with every note would not fit the panel, the typing note is
dropped whole and never cut: it is the one note that is not the reader's own, and
a half-word is worse than none. In the worst case (a selection and a search
together) the title is full, the note is absent, and the status line is already
speaking for the selection. While composing neither is up, so the note is present.

**Lifetime: state, never an animation.** The note is a counter on the open
conversation, `typing`, set to `TYPING_TICKS` (2) by the peer's typing event and
refreshed by every repeat of it. It ends on whichever comes first:

1. **the peer's next message**, which is what the typing was for;
2. **the conversation closing**: moving to another chat (the counter belongs to
   the chat that was left and is zeroed there, so returning does not revive it);
3. **a bounded deadline on the existing network tick**: each tick (`answer`, the
   same one that lands a jump or a sign-in) decrements it, so a peer who stops
   without a final event vanishes on the second tick.

Nothing repaints on a schedule for it: the frame changes when an event or a tick
arrives, as it does for every other state. The page's specimen drives the tick
with a timer only because it stands in for the network. An event for a
conversation that is not the open one is dropped: the counter would be zeroed on
the reader's arrival anyway.

Scenes: `typing` in the engine, eight frames on Ada Lovelace's chat: the note
appearing, composing a reply with it still up, a selection with a search where it
yields, one tick, the second tick, a repeat before the deadline, the peer's
message arriving, and leaving the chat and coming back.

## Motion

There is none, deliberately.

A frame is drawn from a shared reference and the layout is rebuilt per frame, so
an animation is a second thing to keep in step with the window. Nothing repaints
on a schedule: the program asks the terminal for no blinking cursor, the
scrollbar's thumb and the selection row's reverse video change only when the
reader moves, and the carets are painted cells rather than the terminal's own. A
spinner or a countdown would be the first thing in this programme that repainted
on a schedule rather than on a change, and it is not worth it.

The peer's typing note is not an exception: it is state that an event sets and an
event or a tick clears; see *Peer typing*.

## What this document is for

It is the spec the rest of the design work implements, and it is tracked beside
`AGENTS.md` for the same reason: a design system that lives only inside a tool
drifts from the code with nothing to catch it. When a change makes a section
here wrong, the change is incomplete until this file is updated in the same
commit.
