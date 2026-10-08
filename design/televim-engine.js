/* televim engine: a modal state machine plus a renderer that draws an exact 80x24 cell grid.
   No DOM in here. A cell is [char, attr]; attr is a string: first letter = foreground role,
   the rest are flags. fg: t text, d dim, m match, f border-focused, b border, N I V X mode labels.
   flags, one name each:
     r  selection      reverse video on the row the cursor is on (read-only surfaces only)
     s  selection-bg   a Visual selection's background, never reversed
     c  caret-insert   the two-column bar, drawn while the line is being composed
     n  caret-normal   the hollow cell: the line's own Normal mode, and a card's inline position */
(function (root) {
  'use strict';
  const W = 80, H = 24, LW = 24, RW = 56, BAR_MAX = 6, LABEL = 9, HINT_W = 71, MIN_BODY_WIDTH = 8;

  /* the hints a reader can see, each of which fits. ALL_HINTS is what the column check iterates. */
  const HINT = {
    normal: ' i:ins  r:rep  e:edit  dd:del  v:vis  /:find  ::cmd  A:card  S:acct',
    list: ' j/k: chat  ⏎: open  Tab: pane  h: conversation  A:card  S:you  p:pin',
    visual: ' d: delete  y: yank  r: reply  Esc: cancel',
    /* a confirmation outranks every hint, so a confirm hint has no state to be shown in. */
    draft: ' ⏎ draft — i to continue, ^J/⏎ to discard',
    typing: ' ⏎: send  ^J: newline  shift+⏎: newline where supported',
    complete: ' ⇥/⏎: pick  ↑/↓: choose  Esc: close',
    lnormal: ' i/a: ins  w/b/e  x: del  dw/cc  p: paste  gg/G: ends  ⏎: send  Esc',
    lvisual: ' y: yank  d: cut  Esc: back',
    settings: ' e/Enter: edit  j/k: field  Esc/Tab/Ctrl+w: leave',
    /* the two cards differ because their rows differ: d acts on the self card's two action
       rows, and only the contact's card has a yy worth teaching. A key named on a card that
       does not answer it tells the reader the key is wrong, so neither names the other's. */
    cardSelf: ' j/k: row  h/l: within  v: vis  y: yank  d: act  Esc: back',
    cardContact: ' j/k: row  h/l: within  v: vis  y/yy: yank  Esc: back',
    cardSignedOut: ' ::signin  q:quit',
    cardReading: ' q:quit'
  };
  const ALL_HINTS = Object.values(HINT);
  /* reader's own text. The popup lists these; the chrome never paints the glyph. */
  const SHORTCODES = [':heart:', ':smile:', ':thinking:', ':thought_balloon:', ':thumbsup:', ':wave:'];
  const COMP_KIND = { message: 1, reply: 1, edit: 1 };

  const chars = (s) => Array.from(s).length;

  /* ---------- cells, clusters, and the direction of a message ----------
     A column is a cell and a character is not a column: an emoji is two, a
     combining mark is none, a CJK ideograph is two. So every width here is a
     count of CELLS, and `chars` above stays what it was — a count of code
     points, which is what a hint budget and a caret offset are measured in.

     The widths below are the ones `wrap::columns` gets from `unicode-width`,
     which is the table the Rust measures with, so a row this engine lays out is
     as wide as the row the binary lays out. A cluster carrying any wide
     character is two cells whatever else is in it: that is what makes a ZWJ
     family, a skin-tone sequence and a VS16 sequence each two columns, and what
     the terminal draws. `Intl.Segmenter` is the cluster boundary, the same
     extended cluster `grapheme::clusters` walks. */
  const SEGMENTER = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  const clusters = (s) => Array.from(SEGMENTER.segment(s), (x) => x.segment);

  const WIDE = [
    [0x1100, 0x115f], [0x2e80, 0x303e], [0x3041, 0x33ff], [0x3400, 0x4dbf],
    [0x4e00, 0x9fff], [0xa000, 0xa4cf], [0xa960, 0xa97f], [0xac00, 0xd7a3],
    [0xf900, 0xfaff], [0xfe10, 0xfe19], [0xfe30, 0xfe6f], [0xff00, 0xff60],
    [0xffe0, 0xffe6], [0x1f300, 0x1f64f], [0x1f680, 0x1f6ff], [0x1f900, 0x1f9ff],
    [0x20000, 0x3fffd]
  ];
  const ZERO_WIDTH = [
    [0x0300, 0x036f], [0x0483, 0x0489], [0x0591, 0x05bd], [0x05bf, 0x05bf],
    [0x05c1, 0x05c2], [0x0610, 0x061a], [0x064b, 0x065f], [0x0670, 0x0670],
    [0x06d6, 0x06dc], [0x0711, 0x0711], [0x0730, 0x074a], [0x07a6, 0x07b0],
    [0x0900, 0x0902], [0x093a, 0x093a], [0x0941, 0x0948], [0x0e31, 0x0e31],
    [0x0e34, 0x0e3a], [0x0eb1, 0x0eb1], [0x200b, 0x200f], [0xfe00, 0xfe0f],
    [0xfe20, 0xfe2f], [0x20d0, 0x20f0]
  ];
  const inRanges = (cp, ranges) => ranges.some(([lo, hi]) => cp >= lo && cp <= hi);
  const cellsOf = (cluster) => {
    if (Array.from(cluster).some((ch) => inRanges(ch.codePointAt(0), WIDE))) return 2;
    return Array.from(cluster).reduce((n, ch) => n + (inRanges(ch.codePointAt(0), ZERO_WIDTH) ? 0 : 1), 0);
  };
  /* the cells a string occupies. This is the width every layout decision below
     makes: the wrap, the box titles, the trailing note, the hint budget. */
  const cells = (s) => clusters(s).reduce((n, c) => n + cellsOf(c), 0);
  /* the code-unit index each cluster begins at, and the end of the last one, so
     a cluster range can be sliced out of the string it came from. */
  function clusterSpans(s) {
    const spans = [];
    let at = 0;
    for (const c of clusters(s)) { spans.push({ s: at, e: at + c.length }); at += c.length; }
    spans.push({ s: at, e: at });
    return spans;
  }

  /* ---------- direction ----------
     Two questions, asked of different things, and this is the same split
     `crates/tui/src/bidi.rs` makes. WHICH WAY a message reads is a property of
     the message as a whole and is answered by the first strongly-directional
     character anywhere in it — not in its first paragraph, because the neutrals
     at the top cannot outvote what follows them. IN WHAT ORDER a row's pieces
     are drawn is a property of that one row and is answered by the algorithm
     over the row alone, at the base direction the message was given.

     `baseDir` returns 'ltr', 'rtl', or 'mixed' for a message with no strong
     character in it at all — which the reader resolves as left-to-right, because
     rule P3 leaves it alone.

     `visualPieces` is the row's answer: the code-unit ranges of the row, in the
     order the terminal is handed them. Left-to-right text comes back as one
     piece covering the whole row, because nothing moved. Right-to-left text
     comes back reversed, one piece per cluster, because a piece is a logical
     slice drawn AS WRITTEN and a Hebrew word is not written in the order it is
     read. An embedded left-to-right run — digits, a bracketed word in an
     otherwise right-to-left row — stays whole, in the order it is written.

     The pieces partition the row: no code unit is dropped, none is drawn twice,
     and their widths sum to `cells(row)`, whatever the permutation did.

     Not here, and not claimed: shaping (Arabic contextual joining is the
     terminal's), Rule L4 glyph mirroring, and line breaking in visual order —
     rows are broken logically, in `wrap`, which is the order the text is stored
     in. `DESIGN.md` says all three under "Text direction". */
  const RTL_RANGES = [
    [0x0590, 0x08ff], [0xfb1d, 0xfdff], [0xfe70, 0xfeff],
    [0x10800, 0x10fff], [0x1e800, 0x1efff]
  ];
  const ARABIC_NUMBER_RANGES = [[0x0660, 0x0669], [0x066b, 0x066c], [0x06f0, 0x06f9]];
  /* ON — the neutrals: whitespace, punctuation, symbols and emoji. A pictograph
     has no direction of its own, so it takes the direction of what surrounds it,
     which is why `👨‍👩‍👧` at the top of a message does not decide its base. */
  const NEUTRAL_RANGES = [
    [0x0000, 0x002f], [0x003a, 0x0040], [0x005b, 0x0060], [0x007b, 0x007e],
    [0x00a1, 0x00bf], [0x00d7, 0x00d7], [0x00f7, 0x00f7], [0x2010, 0x2027],
    [0x2030, 0x205e], [0x2190, 0x2bff], [0xfb00, 0xfdff], [0xfe10, 0xfe6f],
    [0x1f000, 0x1ffff], [0xe0000, 0xe01ef]
  ];
  /* L — the strongly left-to-letter scripts, named rather than assumed. The
     default is NOT left-to-right: a code point in none of these tables is
     unassigned as far as this model is concerned, and the algorithm's own default
     for an unassigned character is its paragraph's direction. Getting this
     backwards makes every emoji read as a left-to-letter character, which then
     decides the base direction of any message that opens with one. */
  const LTR_RANGES = [
    [0x0041, 0x005a], [0x0061, 0x007a], [0x00c0, 0x02b8], [0x0370, 0x058f],
    [0x0900, 0x1fff], [0x2c60, 0x2dff], [0x2e80, 0x2fdf], [0x3005, 0x3006],
    [0x3041, 0x3096], [0x309d, 0x30ff], [0x3105, 0x312f], [0x3131, 0x318e],
    [0x3190, 0x7fff], [0xa000, 0xf8ff], [0xfb00, 0xfb17], [0xff21, 0xff3a],
    [0xff41, 0xff5a], [0x10000, 0x107ff], [0x11000, 0x1e7ff]
  ];
  /* a cluster that is nothing but marks takes the direction of the one it sits
     on: a Hebrew letter with its vowel points is one letter, not a letter and a
     neutral. */
  const isMark = (ch) => inRanges(ch.codePointAt(0), ZERO_WIDTH) || ch === '‍';

  function bidiType(ch) {
    const cp = ch.codePointAt(0);
    if (inRanges(cp, RTL_RANGES)) return 'R';
    if (inRanges(cp, ARABIC_NUMBER_RANGES)) return 'AN';
    if (cp >= 0x30 && cp <= 0x39) return 'EN';
    if (inRanges(cp, NEUTRAL_RANGES)) return 'ON';
    if (inRanges(cp, LTR_RANGES)) return 'L';
    /* unassigned, and no direction of its own: the paragraph decides */
    return 'ON';
    return 'L';
  }

  function baseDir(text) {
    for (const cluster of clusters(text)) {
      for (const ch of cluster) {
        const t = bidiType(ch);
        if (t === 'L' || t === 'R') return t === 'L' ? 'ltr' : 'rtl';
      }
    }
    return 'mixed';
  }

  /* the level of every cluster in a row, at the paragraph level the message was
     given. The rules, in the order the algorithm applies them. */
  function bidiLevels(text, base) {
    const baseLevel = base === 'rtl' ? 1 : 0;
    const cls = clusters(text);
    /* rule W1: a cluster of marks is the direction of the one before it. */
    const type = cls.map((cluster, i) => {
      const first = Array.from(cluster)[0];
      if (i > 0 && isMark(first)) return type[i - 1];
      return bidiType(first);
    });
    /* Rules W4 and W5, which are what keep `21:30` one left-to-right run — a clock
       time read as `30:21` is the failure this exists to prevent — and they have
       to happen before the neutrals are resolved or the `:` is given a direction
       of its own and the time is split around it.

       W4: ONE separator *between* two numbers of the same kind becomes a number,
       so `21:30` and `1,000` hold together. W5: a run of ET (currency signs and
       the like) *next to* a number joins it, so a price does not come apart.
       Neither applies to a separator with a number on only one side — a leading
       `.2` and a trailing `12:` are a number and a punctuation mark, and gluing
       them together is what reverses their order for the reader. */
    const ch = (i) => Array.from(cls[i])[0];
    const isSep = (c) => c === '+' || c === '-' || c === '/' || c === ',';
    const isEt = (c) => c === '#' || c === '$' || c === '£' || c === '€' || c === '¥' || c === '₪';
    const isCs = (c) => c === ',' || c === '.' || c === ':';
    const isEs = (c) => c === '+' || c === '-';
    /* W4 — a lone ES or CS between two like numbers */
    for (let i = 1; i + 1 < type.length; i++) {
      if (type[i] !== 'ON') continue;
      const c = ch(i);
      const before = type[i - 1], after = type[i + 1];
      const cs = isCs(c), es = isEs(c);
      if ((cs || es) && before === 'EN' && after === 'EN') type[i] = 'EN';
      else if (cs && before === 'AN' && after === 'AN') type[i] = 'AN';
    }
    /* W5 — a run of ET adjacent to EN */
    for (let i = 0; i < type.length; i++) {
      if (type[i] !== 'ON' || !isEt(ch(i))) continue;
      let j = i;
      while (j < type.length && type[j] === 'ON' && isEt(ch(j))) j++;
      if ((i > 0 && type[i - 1] === 'EN') || (j < type.length && type[j] === 'EN')) {
        for (let k = i; k < j; k++) type[k] = 'EN';
      }
      i = j - 1;
    }
    /* the separators that did not become numbers are ordinary neutrals from here */
    for (let i = 0; i < type.length; i++) {
      if (type[i] === 'ON' && (isSep(ch(i)) || isCs(ch(i)) || isEs(ch(i)))) type[i] = 'CS';
    }
    /* Rule N0, before W7 and before the neutrals: a MATCHED pair of brackets is set
       to the EMBEDDING direction — the paragraph's — and an unmatched one is left
       as the neutral it is. So `(abc)`, `(1)` and `[س]` all keep their two halves
       at the paragraph's level in a right-to-left row, with whatever they hold
       one level in, which is what stops a bracket pair being torn off the
       sentence it belongs to. This is bracket PAIRING and nothing else: the
       glyph is not mirrored (see DESIGN.md).

       A paired bracket is recorded rather than applied: the pair's CONTENTS are
       skipped by W7 below, because a number outside a pair does not inherit the
       direction of what the pair holds. `(abc) 42` in a right-to-left row is a
       left-to-letter `(abc)`, a right-to-letter `42` and nothing in between —
       without this the `42` would take the `c` and read `24`. */
    const OPEN = { '(': ')', '[': ']', '{': '}', '<': '>' };
    const CLOSE = { ')': '(', ']': '[', '}': '{', '>': '<' };
    const embedding = baseLevel ? 'R' : 'L';
    for (let i = 0; i < type.length; i++) {
      const c = Array.from(cls[i])[0];
      if (type[i] !== 'ON') continue;
      /* A MATCHED pair takes the EMBEDDING direction, whatever it holds: `(abc)`
         in a right-to-left row is a pair at the paragraph's level with its `abc`
         one level inside it, and `(שלום)` is a pair at the paragraph's level with
         a right-to-left word inside it. Whatever the pair holds is resolved on its
         own afterwards, which is why the two read differently without the
         brackets moving. An unmatched bracket is left to N1/N2 like any other
         neutral. This is bracket PAIRING and nothing else — the glyph is not
         mirrored (see DESIGN.md). */
      if (!OPEN[c]) continue;
      const stack = [c];
      let end = -1;
      for (let j = i + 1; j < type.length; j++) {
        const k = Array.from(cls[j])[0];
        if (OPEN[k]) stack.push(k);
        else if (stack[stack.length - 1] === CLOSE[k]) {
          stack.pop();
          if (stack.length === 0) { end = j; break; }
        }
      }
      if (end < 0) continue;
      type[i] = embedding;
      type[end] = embedding;
    }
    /* Rule W7: a European number takes the direction of the last STRONG character
       before it, wherever that was — neutrals in between are skipped, because a
       number and the punctuation beside it are one thing to a reader. So `Z0!0`
       is one left-to-right piece and the second `0` does not start a run of its
       own. A bracket pair is not strong, so the `42` of `(abc) 42` belongs to
       the paragraph rather than to the `abc` inside it. */
    let strong = baseLevel ? 'R' : 'L';
    for (let i = 0; i < type.length; i++) {
      if (type[i] === 'L' || type[i] === 'R') { strong = type[i]; continue; }
      if (type[i] === 'EN' && strong === 'L') type[i] = 'L';
    }
    /* Rules N1 and N2: a run of NEUTRALS — every character that is not strongly
       directional and not a number, including the separators W4 and W5 left
       behind and every space — takes the direction of the two strong characters
       it sits between when they agree, and the paragraph's when they do not or
       when either end of the row is missing. A number counts as
       right-to-letter here, which is why the space between a Hebrew word and a
       number stays with the Hebrew and not with the number.

       Treating the run as one run is the whole rule: a space and the punctuation
       beside it are resolved together, so a `!` and the space after it cannot end
       up on opposite sides of the run they belong to. */
    const isNeutral = (t) => t === 'ON' || t === 'CS' || t === 'ET' || t === 'WS';
    const leans = (t) => (t === 'L' ? 'L' : t === 'R' || t === 'EN' || t === 'AN' ? 'R' : null);
    for (let i = 0; i < type.length; i++) {
      if (!isNeutral(type[i])) continue;
      let j = i;
      while (j < type.length && isNeutral(type[j])) j++;
      const before = i > 0 ? leans(type[i - 1]) : baseLevel ? 'R' : 'L';
      const after = j < type.length ? leans(type[j]) : baseLevel ? 'R' : 'L';
      const fill = before && before === after ? before : baseLevel ? 'R' : 'L';
      for (let k = i; k < j; k++) type[k] = fill === 'L' ? 'L' : 'R';
      i = j - 1;
    }
    /* rules I1 and I2: an even paragraph level lifts a right-to-letter and lifts a
       number by two; an odd one lifts a left-to-letter and a number by one. That
       is what leaves a number readable as written inside a right-to-left row. */
    const level = type.map((t) => {
      if (baseLevel % 2 === 0) {
        if (t === 'R') return baseLevel + 1;
        return t === 'AN' || t === 'EN' ? baseLevel + 2 : baseLevel;
      }
      return t === 'L' || t === 'EN' || t === 'AN' ? baseLevel + 1 : baseLevel;
    });
    /* rule L1: whitespace at the end of the line takes the paragraph's level, which
       puts the space after a right-to-left word where a reader looks for it. */
    for (let i = level.length - 1; i >= 0; i--) {
      if (!/^\s$/.test(Array.from(cls[i])[0])) break;
      level[i] = baseLevel;
    }
    return level;
  }

  /* rule L2: from the highest level down to the lowest odd one, reverse every
     contiguous run of clusters at that level or above. The returned array is the
     cluster index drawn at each visual position, left to right. */
  function reorderVisual(level) {
    let highest = 0, lowestOdd = Infinity;
    for (const l of level) {
      if (l > highest) highest = l;
      if (l % 2 === 1 && l < lowestOdd) lowestOdd = l;
    }
    const order = level.map((_, i) => i);
    if (lowestOdd === Infinity) return order;
    for (let l = highest; l >= lowestOdd; l--) {
      for (let i = 0; i < order.length; i++) {
        if (level[order[i]] < l) continue;
        let j = i;
        while (j + 1 < order.length && level[order[j + 1]] >= l) j++;
        for (let a = i, b = j; a < b; a++, b--) { const t = order[a]; order[a] = order[b]; order[b] = t; }
        i = j;
      }
    }
    return order;
  }

  /* the pieces of one row, in the order the terminal is handed them, as code-unit
     ranges into `text`. A piece only grows while the visual order walks the
     logical string forwards: a reversed run has no logical slice to hand the
     painter, so it is a piece per cluster rather than a piece drawn backwards. */
  function visualPieces(text, base) {
    if (!text) return [];
    const spans = clusterSpans(text);
    const clustersOf = spans.slice(0, -1);
    const order = reorderVisual(bidiLevels(text, base));
    const pieces = [];
    for (const index of order) {
      const span = clustersOf[index];
      const last = pieces[pieces.length - 1];
      if (last && last.e === span.s) last.e = span.e;
      else pieces.push({ s: span.s, e: span.e });
    }
    return pieces;
  }
  /* the same answer, already sliced: each piece as the text of it and the cells
     it occupies. What the painter walks, and what a frame can be read back as. */
  const visualCells = (text, base) =>
    visualPieces(text, base).map((p) => ({ text: text.slice(p.s, p.e), cells: cells(text.slice(p.s, p.e)) }));

  /* The drawn column of a caret at logical column `col` of one row's body, once
     that body is handed to the terminal in visual order. A piece is drawn AS
     WRITTEN, so its logical start sits at its left edge — unless the run it
     belongs to reads right to left, where the logical start sits at its right
     edge. The caret therefore counts the pieces drawn to its left and, in the one
     piece it sits in, the part of that piece drawn before it. */
  function caretCol(text, base, col) {
    const spans = clusterSpans(text).slice(0, -1);
    const level = bidiLevels(text, base);
    let drawn = 0;
    for (const piece of visualPieces(text, base)) {
      const ci = spans.findIndex((sp) => sp.s === piece.s);
      const rtl = ci >= 0 && level[ci] % 2 === 1;
      if (rtl) {
        if (col <= piece.s) drawn += cells(text.slice(piece.s, piece.e));
        else if (col < piece.e) drawn += cells(text.slice(col, piece.e));
      } else {
        if (col >= piece.e) drawn += cells(text.slice(piece.s, piece.e));
        else if (col > piece.s) drawn += cells(text.slice(piece.s, col));
      }
    }
    return drawn;
  }

  /* the profile card's columns, inside the 56-column right panel: cue, label, value */
  const CUE_X = 2, LAB_X = 4, LAB_W = 12, VAL_X = 17, VAL_W = 37;
  const CUE = '·';

  /* what a peer says about itself. A field the peer does not give is an absent row, not an
     empty one: that is the whole of the rule, and it is why a birthday row is usually missing. */
  const PERSON = {
    'Ada Lovelace': { username: 'adalovelace', bio: 'Numbers, engines, and the side gate at nine.' },
    'Grace Hopper': { username: 'gracer', bio: 'Loose nanoseconds on the desk.' },
    'Alan Turing': { bio: 'Reviewing proofs. Not answering the phone.' },
    'Katherine Johnson': { username: 'katj', bio: 'Numbers check out.' },
    'Margaret Hamilton': { username: 'mhamilton' },
    'Barbara Liskov': { username: 'bliskov', bio: 'Substitutable, or it is not a subtype.' },
    'Dennis Ritchie': { username: 'dmr', bio: '-O2. Always -O2.' },
    'Edsger Dijkstra': { username: 'ewd', bio: 'Testing shows the presence of bugs.' },
    'Hedy Lamarr': { username: 'hedyl', bio: 'Frequencies at nine.' },
    'Donald Knuth': { username: 'taocp', bio: 'Volume four is late.' },
    'Radia Perlman': { username: 'radia', bio: 'Loop free. Lovely.' },
    'Frances Allen': { username: 'fallen', bio: 'Optimise the loop, not the line.' },
    /* people the reader is not chatting with: the new-chat search's directory. A name here and
       nowhere in makeChats() is a person with no conversation to focus, which is the only state
       in which opening creates a chat rather than moving to one. */
    'Linus Torvalds': { username: 'torvalds', bio: 'Talk is cheap. Show me the code.' },
    'Shafi Goldwasser': { username: 'shafig', bio: 'Randomness is a tool, not a flaw.' },
    'Vint Cerf': { username: 'vint', bio: 'The protocol was the easy part.' },
    'Tim Berners-Lee': { username: 'timbl', bio: 'This is for everyone.' }
  };
  /* a birthday with no year: a year is a disclosure the account was not required to make */
  PERSON['Ada Lovelace'].birthday = '10 December';

  /* the two shell cards. The reason is a chain, so it is longer than the panel and it wraps. */
  const NOT_SIGNED_IN = 'not signed in';
  const SIGNED_OUT_REASON = 'no session: the OS credential store holds no entry for televim, and there is no plaintext file at ~/.local/state/televim/session';
  const SET_CREDENTIALS = 'Set the credentials again with :signin.';
  const SIGNED_OUT = 'signed out';
  const READING = 'reading the session…';
  const READING_NOTE = 'Nothing has been read yet, so nothing is known either way.';
  /* a contact's shell. Its problem is never credentials, so it names no way back into :signin. */
  const CONTACT_READING = 'reading their profile…';
  const CONTACT_UNAVAILABLE = 'could not read this profile';
  const CONTACT_FAILED_REASON = 'the profile request failed: Telegram did not answer before the fetch timed out';

  /* ---------- sign in: the words this view adds ----------
     The refusals are a Telegram error code mapped to the line the reader sees. Three of them
     are Telegram's doing or the account's own doing, so their sentences never say "you":
     PHONE_CODE_EXPIRED, SESSION_REVOKED, AUTH_KEY_UNREGISTERED. */
  const AUTH = {
    PHONE_CODE_INVALID: 'that code is not the one Telegram sent',
    PHONE_CODE_EXPIRED: 'that code has expired — ⏎ asks for a new one',
    PHONE_NUMBER_INVALID: 'that is not a phone number Telegram will accept',
    PHONE_NUMBER_BANNED: 'Telegram has banned that number',
    PHONE_NUMBER_FLOOD: 'too many attempts — wait, then try again',
    PASSWORD_MISSING: 'this account has no two-factor password',
    SESSION_REVOKED: 'this session was revoked — sign in again; and, for the log, the stored session is discarded',
    AUTH_KEY_UNREGISTERED: 'the stored session is no longer valid — sign in again'
  };
  /* SESSION_PASSWORD_NEEDED is not a refusal: it is the answer that puts the password row up. */
  const SIGNOUT_ROW = '[ ⏎: sign in again ]';
  const CHECKING = 'Checking…';
  /* the peer is typing in the open conversation: a note on the title, in the dim ink, and state
     rather than motion. It is set by the peer's typing event and ends on the peer's next message,
     on the conversation closing, or when the network tick has come round TYPING_TICKS times
     without the event being repeated. Nothing counts it down on a schedule of its own. */
  const TYPING_NOTE = ' · typing';
  const TYPING_TICKS = 2;
  const CANCEL_COST = 'cancelling discards the code Telegram sent; ⏎ asks for a new one';
  const LOST_CODE = 'the code did not survive; ⏎ asks for a new one';
  const NO_CREDENTIALS = 'televim has no application credentials. It needs an api_id and an api_hash in its config file before it can sign in to anything.';
  const attempts = (n) => n + ' attempt' + (n === 1 ? '' : 's') + ' left';
  /* the refusal sentence, with the password's count interpolated from the row's own counter */
  const authSentence = (code, n) => code === 'PASSWORD_HASH_INVALID'
    ? 'that password is not right (' + attempts(n) + ')'
    : AUTH[code] || code;
  const pwLabel = (n) => 'two-factor password (' + attempts(n) + ')';

  /* ---------- time: minutes since the epoch, in UTC so a frame is the same everywhere ----------
     A message with no `at` has no time: it never groups, never gets a separator and never shows
     a timestamp, which is how every row drawn before these treatments existed still reads. */
  const GROUP_MIN = 5;                          /* a gap past this breaks a group */
  const TODAY = [2026, 9, 2];                   /* the specimen's today: Friday 2 October 2026 */
  const NOW = Date.UTC(TODAY[0], TODAY[1], TODAY[2], 21, 40) / 60000;
  const at = (ago, hm) => Date.UTC(TODAY[0], TODAY[1], TODAY[2] - ago, +hm.slice(0, 2), +hm.slice(3)) / 60000;
  const dayOf = (m) => Math.floor(m / 1440);
  const WEEKDAY = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
  const MONTH = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  /* Today, Yesterday, the weekday for the days between, a full date past the week */
  function dayLabel(m) {
    const ago = dayOf(NOW) - dayOf(m), d = new Date(m * 60000);
    if (ago <= 0) return 'Today';
    if (ago === 1) return 'Yesterday';
    if (ago < 7) return WEEKDAY[d.getUTCDay()];
    return MONTH[d.getUTCMonth()] + ' ' + d.getUTCDate() + ', ' + d.getUTCFullYear();
  }
  const clock = (m) => { const d = new Date(m * 60000); return String(d.getUTCHours()).padStart(2, '0') + ':' + String(d.getUTCMinutes()).padStart(2, '0'); };
  /* does b continue a's group? Same side, same calendar day, within the window, and b is not a reply. */
  const joins = (a, b) => !!(a && b && a.at != null && b.at != null && a.from === b.from && !b.reply
    && dayOf(a.at) === dayOf(b.at) && b.at - a.at <= GROUP_MIN);

  /* ---------- data ---------- */
  function makeChats() {
    /* every message has an id, the reply's `to` names one, and `all` (a chat that has one) is what
       the server holds: the loaded window is a slice of it. */
    const C = (name, unread, msgs, extra) => {
      const base = (extra && extra.base) || 0;
      msgs.forEach((m, i) => { if (m.id == null) m.id = base + i + 1; });
      const c = Object.assign({ name, unread, msgs, cur: Math.max(0, msgs.length - 1), top: 0 }, extra || {});
      if (c.history) { c.all = c.history.concat(msgs); delete c.history; }
      return c;
    };
    const t = (text, o) => Object.assign({ from: 'them', text }, o || {});
    const y = (text, o) => Object.assign({ from: 'you', text }, o || {});
    return [
      C('Ada Lovelace', 0, [
        t('Are you coming tonight?'),
        y('Running late. Save me a seat.'),
        t('The one by the window.', { reply: { quote: 'Save me a seat.', to: 102 } }),
        t('Bring the tickets, the tickets are in the blue folder.'),
        y('Which tickets? I have the train ones and the concert ones.'),
        t('The concert tickets. The train ones can stay on the fridge.'),
        y('Tickets are under the lamp. Folder found.'),
        t('Sorry, I missed your earlier message.', { reply: { unloaded: true, to: 94 } }),
        t('Doors open at eight, and the side gate is shut after nine, so do not be later.'),
        y('Did that go through?', { status: 'failed: no route' }),
        y('On my way, ten minutes.', { status: 'sending…' }),
        t('I will keep your seat.'),
        t('The side gate locks at nine. If you miss it, ring twice and wait by the lamp.'),
        y('Twice. I have both tickets in the blue folder, concert on top.'),
        t('Leave the train ones on the fridge. I only need the concert pair tonight.'),
        y('Understood. Saving the seat was the whole of the favour.')
      ], { cur: 5, older: true, base: 100, history: [
        t('Morning. Has the programme arrived?', { id: 91 }),
        y('Not yet. Which hall is it?', { id: 92 }),
        t('The old hall on the canal.', { id: 93 }),
        y('Same as last spring?', { id: 94 }),
        t('Yes, the same hall, a different door.', { id: 95 }),
        y('Then I will meet you at the bridge.', { id: 96 }),
        t('The bridge, then.', { id: 97 }),
        y('Bringing the umbrella.', { id: 98 }),
        t('Bring a coat as well.', { id: 99 }),
        y('Fine, a coat too.', { id: 100 })
      ] }),
      /* the one chat with a clock: grouped runs, five days, every read state. `rcpt` is the peer's
         answer for an outgoing message; it is only drawn on a group's newest. */
      C('Grace Hopper', 2, [
        t('Proofs are back with notes.', { at: at(12, '09:12') }),
        y('Send me the margin ones first.', { at: at(12, '09:13'), rcpt: 'read' }),
        y('Then the footnotes.', { at: at(12, '09:15'), rcpt: 'read' }),
        t('Volume four is on the bench.', { at: at(3, '18:02') }),
        t('The index still needs a pass.', { at: at(3, '18:03') }),
        y('Index tonight, I promise.', { at: at(3, '18:20'), rcpt: 'read' }),
        t('The nanosecond wire is on my desk again.', { at: at(1, '20:11') }),
        y('Bring it Thursday.', { at: at(1, '20:13'), rcpt: 'read' }),
        t('Thursday it is.', { at: at(1, '20:14') }),
        y('Packed the wire.', { at: at(0, '21:02'), rcpt: 'read' }),
        y('And the 12-inch ruler.', { at: at(0, '21:04'), rcpt: 'read' }),
        t('Keep the ruler dry.', { at: at(0, '21:05'), reply: { quote: 'And the 12-inch ruler.' } }),
        y('Leaving now.', { at: at(0, '21:09'), rcpt: 'read' }),
        y('Platform, no train yet.', { at: at(0, '21:16'), rcpt: 'delivered' }),
        y('Train is moving.', { at: at(0, '21:31'), rcpt: 'delivered' }),
        y('Sent from the carriage.', { at: at(0, '21:32'), status: 'sending…' }),
        y('Is the gate open?', { at: at(0, '21:38'), status: 'failed: no route' })
      ]),
      C('Alan Turing', 1, [y('Did the paper come back?'), t('Reviewers want one more proof.')], { newer: true }),
      C('Katherine Johnson', 0, [t('Numbers check out.'), y('Good. Send the table.')]),
      C('Margaret Hamilton', 0, [], { loading: true }),
      C('Barbara Liskov', 0, [t('Substitutable, or it is not a subtype.')]),
      C('Dennis Ritchie', 0, [y('Which compiler flag was it?'), t('-O2. Always -O2.')]),
      C('Edsger Dijkstra', 0, [t('Testing shows the presence of bugs.')]),
      C('Hedy Lamarr', 0, [y('Frequencies at nine?'), t('Nine works.')]),
      C('Donald Knuth', 3, [t('Volume four is late.')]),
      C('Radia Perlman', 0, [y('Spanning tree diagram attached.'), t('Loop free. Lovely.')]),
      C('Frances Allen', 0, [t('Optimise the loop, not the line.')]),
      /* The one conversation in a script that reads from the right. It is here to
         be reached with the same keys as every other chat, because a design model
         that could only show right-to-left text through a private entry point
         would not be showing the program.

         The fixtures are chosen to make the three things a direction has to get
         right visible in one frame, rather than to be a translation of the other
         conversations: a plain right-to-left sentence, which is one reversed run
         with nothing embedded in it to hide a mistake behind; a clock time inside
         one, which stays in the order it is written because a reader reads
         21:30 and not 03:12; and a bracketed phrase, whose brackets travel with
         what they hold. The chat's NAME is the one thing here left-to-right on
         purpose — a peer's name is chrome, and the panel title and the list row
         are not the message body. */
      C('Noa Friedman', 2, [
        t('הכרטיסים בתיק הכחול', { at: at(2, '18:04') }),
        y('הבנתי, תודה.', { at: at(2, '18:06'), rcpt: 'read' }),
        t('השער הצדדי נסגר בשעה 21:30', { at: at(0, '20:12') }),
        t('(המקום שלך שמור)', { at: at(0, '20:13'), reply: { quote: 'הכרטיסים בתיק הכחול' } }),
        t('תודה רבה! \u{1F468}‍\u{1F469}‍\u{1F467}', { at: at(0, '20:14') }),
        y('אני בדרך.', { at: at(0, '21:40'), status: 'sending…' })
      ], { cur: 2 })
    ];
  }

  /* MediaKind::label. A media message with no caption has this as its whole `text` (so search, yank
     and reply read it like any text) and `ph` marks it as the program's wording rather than the
     sender's; a caption replaces it and clears `ph`. */
  const MEDIA_LABEL = { Photo: '[image]', Video: '[video]', Gif: '[gif]', Voice: '[voice]', File: '[file]', Sticker: '[sticker]' };
  const media = (from, kind, o) => {
    const cap = o && o.caption;
    return Object.assign({ from, text: cap || MEDIA_LABEL[kind], ph: !cap, kind }, o || {});
  };
  /* the chat the media scenes read. Sides alternate and times keep groups apart, so every message
     is its own group and carries its own tag. */
  function makeMediaChat() {
    const t = (text, o) => Object.assign({ from: 'them', text }, o || {});
    const y = (text, o) => Object.assign({ from: 'you', text }, o || {});
    return { name: 'Ken Thompson', unread: 0, top: 0, cur: 9, msgs: [
      y('Did the scan come out?', { at: at(0, '20:02') }),
      media('them', 'Photo', { at: at(0, '20:04') }),
      y('And the recording?', { at: at(0, '20:05') }),
      media('them', 'Voice', { at: at(0, '20:06') }),
      y('Send the score too.', { at: at(0, '20:07') }),
      media('them', 'File', { at: at(0, '20:09') }),
      media('you', 'Photo', { caption: 'Proof of the lamp.', at: at(0, '20:11'), rcpt: 'read' }),
      media('them', 'Video', { at: at(0, '20:14') }),
      media('you', 'Gif', { at: at(0, '20:15'), rcpt: 'read' }),
      media('you', 'File', { at: at(0, '20:40'), status: 'failed: file exceeds the 2 GB limit' })
    ] };
  }

  /* A static sticker, drawn inline as a bounded half-block picture. The art is the
     model's stand-in for decoded pixels: 16 columns by 8 rows, inside the 24-by-8
     box DESIGN.md bounds the real block by. `inline: false` is the flag off or a
     failed decode, and the message is the `[sticker]` token instead. */
  const STICKER_ART = [
    '  ▄▄▄▄▄▄▄▄▄▄▄▄  ',
    ' ▄████████████▄ ',
    '████████████████',
    '████●██████●████',
    '████████████████',
    '█████▄▄▄▄▄▄█████',
    ' ▀████████████▀ ',
    '  ▀▀▀▀▀▀▀▀▀▀▀▀  '
  ];
  const sticker = (from, o) => Object.assign({ from, text: '[sticker]', ph: true, kind: 'Sticker', art: STICKER_ART, inline: true }, o || {});
  /* the chat the sticker scenes read. Sides alternate so the sticker is its own
     group and carries its own tag and time. */
  function makeStickerChat(inline) {
    return { name: 'Ken Thompson', unread: 0, top: 0, cur: 1, msgs: [
      { from: 'you', text: 'The new set is out.', at: at(0, '20:02') },
      sticker('them', { at: at(0, '20:04'), inline }),
      { from: 'you', text: 'Send the penguin one next.', at: at(0, '20:05') }
    ] };
  }

  /* Who arranges a right-to-left row: the terminal, or this program. Chosen once,
     here, and never while a frame is being drawn — the layout below is a pure
     function of the window and the panel's width, so a mode read out of mutable
     state mid-draw would make the same conversation two different heights
     depending on when it was asked. It is a fact about the terminal rather than
     about the row: one terminal class reverses the run itself and one does not,
     and only the reader knows which they are on. `terminal` is the default and
     the right one for the terminals that shape, where permuting here would
     reverse a right-to-left run a second time and scramble it. */
  const BIDI_MODES = ['terminal', 'visual'];

  function fresh(view, bidi) {
    const s = {
      view: 'chat', right: 'conv', focus: 'conv', chat: 0, chats: makeChats(),
      vis: null, search: null, newchat: null, confirm: null, flash: '', pend: '', line: null, draft: null, reg: '',
      sset: 0, signin: null, card: null, count: 0,
      bidi: BIDI_MODES.includes(bidi) ? bidi : 'terminal',
      profile: { name: 'Noor Haddad', username: 'noorh', bio: 'Night shift. Log first, news later.', phone: '+44 7700 900142', birthday: 'Oct 19, 2001 (24 years old)' }
    };
    if (view === 'signin') beginSignin(s);
    else if (view === 'stale') beginStale(s);
    else if (view === 'nocreds') beginNoCreds(s);
    else if (view === 'loggedout') beginLoggedOut(s);
    else if (view === 'signedout' || view === 'reading') beginShell(s, view);
    else if (view === 'contactReading') beginContactShell(s, 'personReading');
    else if (view === 'contactFailed') beginContactShell(s, 'personFailed');
    else if (view === 'media') s.chats.unshift(makeMediaChat());
    else if (view === 'sticker') s.chats.unshift(makeStickerChat(true));
    else if (view === 'stickerFallback') s.chats.unshift(makeStickerChat(false));
    return s;
  }

  /* ---------- helpers ---------- */
  const chat = (s) => s.chats[s.chat];
  const ctx = (s) => (s.right === 'settings' ? 'settings' : 'chat:' + s.chat);
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  function refuse(s, t) { s.search = null; s.flash = t; }
  function range(s) { const c = chat(s); return [Math.min(s.vis.anchor, c.cur), Math.max(s.vis.anchor, c.cur)]; }
  function hitsOf(s) { return s.search ? s.search.hits : []; }

  /* The rows a string occupies, as code-unit ranges into it — so a row is
     `str.slice(r.s, r.e)` and nothing else, and a range that fell inside a
     grapheme cluster would not be one that can be sliced.

     A row is measured in CELLS, one cluster at a time, which is what makes a
     row of emoji hold five of them in a ten-column panel and makes a break fall
     where the terminal's would. A cluster that does not fit the room left ends
     the row before it, so a row can finish short of its width; the one row
     wider than the panel is a cluster wider than the row it starts.

     Where a row is broken is the text's business and not the direction's: rows
     are broken in logical order, at the last space that fits, and a
     right-to-left message is broken exactly as a left-to-right one is. That is
     the ceiling GAPS G8 records — the break points are logical even though the
     row is read from the right. */
  function wrap(str, w0, w, keep) {
    const rows = [], parts = str.split('\n'); let off = 0;
    for (let pi = 0; pi < parts.length; pi++) {
      const p = parts[pi], last = pi === parts.length - 1; let st = 0;
      for (;;) {
        const width = rows.length === 0 ? w0 : w;
        /* One pass over the clusters from `st`, measuring cells: `used` is the
           column this cluster starts in, so a cluster that does not fit the room
           left begins the next row, and a space is breakable while the row still
           has room after it — which is one column more on the arm that hands the
           space to neither row than on the arm that leaves it on this one. */
        let used = 0, cut = st, space = -1, ranOut = false;
        for (const span of clusterSpans(p).slice(0, -1)) {
          if (span.e <= st) continue;
          const w2 = cellsOf(p.slice(span.s, span.e));
          if (p.slice(span.s, span.e) === ' ' && used <= width - (keep ? 1 : 0)) space = span.s;
          if (used > 0 && used + w2 > width) { cut = span.s; ranOut = true; break; }
          used += w2;
          cut = span.e;
        }
        if (!ranOut) { rows.push({ s: off + st, e: off + p.length, nl: !last }); break; }
        if (keep) {
          const end = space > st ? space + 1 : cut;
          rows.push({ s: off + st, e: off + end, nl: false }); st = end;
        } else {
          const end = space > st ? space : cut;
          rows.push({ s: off + st, e: off + end, nl: false }); st = space > st ? space + 1 : cut;
        }
      }
      off += p.length + 1;
    }
    return rows;
  }
  function caretRC(rows, pos) {
    for (let i = 0; i < rows.length; i++) {
      const r = rows[i];
      if (pos >= r.s && pos < r.e) return [i, pos - r.s];
      if (pos === r.e && (i === rows.length - 1 || r.nl)) return [i, pos - r.s];
    }
    const r = rows[rows.length - 1]; return [rows.length - 1, Math.max(0, r.e - r.s)];
  }

  /* ---------- keys ---------- */
  function key(s, k) {
    /* leaving a conversation closes it, and a closed conversation is no longer being typed in */
    const was = s.chats && s.chats[s.chat];
    keyIn(s, k);
    if (was && s.chats[s.chat] !== was) was.typing = 0;
    return s;
  }
  function keyIn(s, k) {
    if (s.view === 'quit') { const n = fresh(); Object.keys(s).forEach((x) => delete s[x]); Object.assign(s, n); return s; }
    if (s.confirm) { s.flash = ''; confirmKey(s, k); return s; }
    /* a fetch is in flight: Esc drops it and every other key waits for the answer */
    if (s.jump) { if (k === 'Escape') s.jump = null; return s; }
    if (s.view === 'signin') {
      const A = s.signin;
      /* a request is in flight: the first ⏎ is refused, and every later key does nothing. */
      if (A.checking) {
        if (k === 'Enter' && !A.checkN) { A.checkN = 1; s.flash = 'still checking — the answer is on its way'; }
        return s;
      }
      if (!A.away) {
        if (k === 'Tab' || k === 'C-w') { signinAway(s); return s; }
        if (k === 'Escape' && s.line && s.line.from === 'signin' && A.step === 1 && s.line.mode === 'insert') { signinCancel(s); return s; }
      } else if (!s.line) {
        if (k === 'Tab' || k === 'C-w') { signinBack(s); return s; }
        if (common(s, k)) return s;
        return s;   /* the sign-in is paused; only Tab, : and q answer */
      }
    }
    s.flash = '';
    if (s.line) { lineKey(s, k); return s; }
    const p = s.pend; s.pend = '';
    if (s.view === 'nocreds') { common(s, k); return s; }
    if (s.focus === 'settings') settingsKey(s, k, p);
    else if (s.focus === 'profile') cardKey(s, k, p);
    else if (s.focus === 'newchat') newchatKey(s, k, p);
    else if (s.focus === 'list') listKey(s, k, p);
    else if (s.vis) visKey(s, k, p);
    else convKey(s, k, p);
    return s;
  }

  function common(s, k) { /* keys that mean the same in the list, the conversation and the panel */
    switch (k) {
      case ':': openLine(s, { kind: 'command' }); return true;
      case 'q': ask(s, 'Quit televim? (y/n)', (x) => { x.view = 'quit'; }); return true;
    }
    return false;
  }
  function togglePane(s) {
    if (s.focus !== 'list') { s.focus = 'list'; return; }
    s.focus = s.right === 'settings' ? 'settings' : s.right === 'profile' ? 'profile' : 'conv';
  }
  function openSettings(s) { s.right = 'settings'; s.card = null; s.focus = 'settings'; s.sset = 0; }
  /* the card: the same widget as the conversation over different items. It starts at the top
     every time and remembers nothing, where the conversation keeps the reader's place. */
  function openCard(s, sub) {
    s.right = 'profile'; s.focus = 'profile';
    s.card = { sub, chat: s.chat, row: 0, col: 0, vis: null };
  }
  function leaveCard(s) { s.card = null; s.right = 'conv'; s.focus = 'conv'; }
  function beginShell(s, sub) {
    s.view = sub; s.right = 'profile'; s.focus = 'profile';
    s.chats = []; s.chat = 0;            /* no client until credentials arrive, so no chats */
    s.card = { sub, chat: 0, row: 0, col: 0, vis: null };
  }

  /* a contact's shell is not the account's: the chat list stays, only the card changes subject */
  function beginContactShell(s, sub) {
    s.view = 'chat'; s.right = 'profile'; s.focus = 'profile'; s.chat = 0;
    s.card = { sub, chat: 0, row: 0, col: 0, vis: null };
  }
  function ask(s, prompt, run) { s.confirm = { prompt, run }; }

  function confirmKey(s, k) {
    if (k === 'y') { const c = s.confirm; s.confirm = null; c.run(s); }
    else if (k === 'n' || k === 'Escape') s.confirm = null;
  }

  function move(s, d) { const c = chat(s); if (c.msgs.length) c.cur = clamp(c.cur + d, 0, c.msgs.length - 1); }

  function listKey(s, k, p) {
    if (common(s, k)) return;
    switch (k) {
      case 'j': case 'Down': s.chat = clamp(s.chat + 1, 0, s.chats.length - 1); break;
      case 'k': case 'Up': s.chat = clamp(s.chat - 1, 0, s.chats.length - 1); break;
      case 'g': if (p === 'g') s.chat = 0; else s.pend = 'g'; break;
      case 'G': s.chat = s.chats.length - 1; break;
      case 'Enter': s.card = null; s.right = 'conv'; togglePane(s); break;
      case 'l': case 'h': case 'Tab': togglePane(s); break;
      case 'i': case 'a': s.focus = 'conv'; convKey(s, k, ''); break;
      case '/': openNewChat(s, ''); break;
      case 'A': openCard(s, 'person'); break;
      case 'S': openCard(s, 'self'); break;
    }
  }

  /* ---------- jump to the quoted message ----------
     `gd` follows a reply's quote. A target the window holds only moves the cursor; one it does not
     is fetched as a page around it, which replaces the window, and the cursor lands on it. Every
     jump that moves the reader leaves a mark, and Ctrl-o / Ctrl-i walk the marks as Vim's jumplist
     does. A mark is a message id, never a row: the window it was set in may be gone. */
  const JUMP_QUOTE = 'Jumping to the quoted message…', JUMP_BACK = 'Jumping back…', JUMP_FORWARD = 'Jumping forward…';
  const PAGE_RADIUS = 3;
  const hereId = (c) => (c.msgs[c.cur] ? c.msgs[c.cur].id : null);
  function mark(c, id) {
    const jl = c.jl || (c.jl = { list: [], i: 0 });
    if (id == null) return jl;
    jl.list = jl.list.filter((x) => x !== id); jl.list.push(id); jl.i = jl.list.length;
    return jl;
  }
  /* move to message `id`: the cursor if the window holds it, a fetch if the server does. `from` is
     the mark to leave behind once the cursor has actually moved, or null for a walk of the list. */
  function goTo(s, id, label, from) {
    const c = chat(s), at = c.msgs.findIndex((m) => m.id === id);
    if (at >= 0) { if (from != null) mark(c, from); c.cur = at; return; }
    if (!c.all || !c.all.some((m) => m.id === id)) { refuse(s, 'That message is no longer available.'); return; }
    s.jump = { chat: s.chat, id, label, from };
  }
  function landJump(s) {
    const J = s.jump, c = s.chats[J.chat]; s.jump = null; s.search = null;
    const at = c.all.findIndex((m) => m.id === J.id), lo = Math.max(0, at - PAGE_RADIUS), hi = Math.min(c.all.length - 1, at + PAGE_RADIUS);
    c.msgs = c.all.slice(lo, hi + 1); c.cur = at - lo; c.top = 0; c.older = lo > 0; c.newer = hi < c.all.length - 1;
    if (J.from != null) mark(c, J.from);
    return s;
  }
  function jumpQuote(s) {
    const c = chat(s), m = c.msgs[c.cur];
    if (!m) return;
    if (!m.reply || m.reply.to == null) { refuse(s, 'Not a reply: gd jumps to the message a reply quotes.'); return; }
    goTo(s, m.reply.to, JUMP_QUOTE, hereId(c));
  }
  function jumpBack(s) {
    const c = chat(s), jl = c.jl;
    if (!jl || !jl.list.length) return;
    if (jl.i === jl.list.length) { mark(c, hereId(c)); jl.i = jl.list.length - 1; }
    if (jl.i <= 0) return;
    jl.i--; goTo(s, jl.list[jl.i], JUMP_BACK, null);
  }
  function jumpForward(s) {
    const c = chat(s), jl = c.jl;
    if (!jl || jl.i >= jl.list.length - 1) return;
    jl.i++; goTo(s, jl.list[jl.i], JUMP_FORWARD, null);
  }

  function convKey(s, k, p) {
    if (common(s, k)) return;
    const c = chat(s), m = c.msgs[c.cur];
    switch (k) {
      case 'j': case 'Down': move(s, 1); break;
      case 'k': case 'Up': move(s, -1); break;
      case 'C-d': move(s, 5); break;
      case 'C-u': move(s, -5); break;
      case 'g': if (p === 'g') c.cur = 0; else s.pend = 'g'; break;
      case 'C-o': jumpBack(s); break;
      case 'C-i': jumpForward(s); break;
      case 'G': c.cur = Math.max(0, c.msgs.length - 1); break;
      case 'v': if (m) s.vis = { anchor: c.cur }; break;
      case 'i': case 'a': openLine(s, { kind: 'message' }); break;
      case 'r': if (m) openLine(s, { kind: 'reply', ref: { quote: m.text } }); break;
      case 'e':
        if (!m) break;
        if (m.from !== 'you') refuse(s, 'Only your own messages can be edited.');
        else openLine(s, { kind: 'edit', buf: m.text, ref: { idx: c.cur } });
        break;
      case 'd':
        if (p === 'g') { jumpQuote(s); break; }
        if (p !== 'd') { s.pend = 'd'; break; }
        if (!m) break;
        if (m.from !== 'you') refuse(s, 'Not yours: dd deletes your own messages. D dismisses.');
        else askDelete(s, [c.cur]);
        break;
      case 'D': if (m) { c.msgs.splice(c.cur, 1); c.cur = clamp(c.cur, 0, c.msgs.length - 1); s.flash = 'Dismissed on this side only.'; } break;
      case '/': openLine(s, { kind: 'find' }); break;
      case 'n': case 'N': stepSearch(s, k === 'n' ? 1 : -1); break;
      case 'Escape': s.search = null; break;
      case 'Enter': case 'C-j': if (s.draft && s.draft.ctx === ctx(s)) s.draft = null; break;
      case 'Tab': case 'h': togglePane(s); break;
      case 'l': openCard(s, 'person'); break;
      case 'A': openCard(s, 'person'); break;
      case 'S': openCard(s, 'self'); break;
    }
  }

  function visKey(s, k, p) {
    const c = chat(s);
    switch (k) {
      case 'j': case 'Down': move(s, 1); break;
      case 'k': case 'Up': move(s, -1); break;
      case 'g': if (p === 'g') c.cur = 0; else s.pend = 'g'; break;
      case 'G': c.cur = c.msgs.length - 1; break;
      case 'Escape': case 'v': s.vis = null; break;
      case 'y': {
        const [lo, hi] = range(s); s.reg = c.msgs.slice(lo, hi + 1).map((m) => m.text).join('\n');
        s.vis = null; s.flash = (hi - lo + 1) + ' message(s) yanked'; break;
      }
      case 'r': {
        const [lo, hi] = range(s);
        if (hi > lo) { s.flash = 'Reply takes one message; ' + (hi - lo + 1) + ' are selected.'; break; }
        s.vis = null; openLine(s, { kind: 'reply', ref: { quote: c.msgs[lo].text } }); break;
      }
      case 'd': {
        const [lo, hi] = range(s); const own = [];
        for (let i = lo; i <= hi; i++) if (c.msgs[i].from === 'you') own.push(i);
        if (!own.length) s.flash = 'None of the selected messages are yours.';
        else askDelete(s, own);
        break;
      }
    }
  }

  function askDelete(s, idxs) {
    const prompt = idxs.length === 1 ? 'Delete your message from both sides? (y/n)' : 'Delete ' + idxs.length + ' of your messages? (y/n)';
    ask(s, prompt, (x) => {
      const c = chat(x);
      for (let i = idxs.length - 1; i >= 0; i--) c.msgs.splice(idxs[i], 1);
      c.cur = clamp(idxs[0], 0, c.msgs.length - 1); x.vis = null;
    });
  }

  /* ---------- search ---------- */
  function runSearch(s, q) {
    q = q.trim();
    if (!q) { s.search = null; return; }
    const c = chat(s), lq = q.toLowerCase(), hits = [];
    c.msgs.forEach((m, i) => { if (m.text.toLowerCase().includes(lq)) hits.push(i); });
    s.search = { q, hits };
    if (hits.length) { const at = hits.find((i) => i >= c.cur); c.cur = at === undefined ? hits[0] : at; }
  }
  function stepSearch(s, d) {
    const c = chat(s), h = hitsOf(s);
    if (!s.search) { s.flash = 'No search yet: / to find.'; return; }
    if (!h.length) return;
    const nxt = d > 0 ? h.find((i) => i > c.cur) : h.slice().reverse().find((i) => i < c.cur);
    c.cur = nxt === undefined ? (d > 0 ? h[0] : h[h.length - 1]) : nxt;
  }

  /* ---------- starting a conversation with someone new ----------
     The directory is everyone the account knows, and a person is a person whether or not a
     conversation already exists: the search runs over display names and usernames the same way,
     and each result carries the fact the choice turns on. The query is not filtered as the reader
     types: it goes out on ⏎ and the candidates arrive afterwards, so the list is a thing the
     network answers, not a thing the prompt draws. An exact username, or a single answer, opens
     the conversation at once; anything else is a list the reader walks. When the chosen person
     already has a chat, that chat is focused, because a second conversation with the same person
     would not be a new one. */
  function dirEntries() {
    return Object.keys(PERSON).map((name) => ({ name, username: PERSON[name].username || '' }));
  }
  function hasChat(s, name) { return s.chats.some((c) => c.name === name); }
  /* The query is a username when it opens with @, a display name otherwise; a name query falls
     back to the username so `gracer` still finds Grace Hopper. Scored so the closest match sorts
     first, then no-conversation-first, then by name. */
  function newChatHits(s, q) {
    const raw = (q || '').trim(), uname = raw[0] === '@';
    const lq = (uname ? raw.slice(1) : raw).toLowerCase();
    if (!lq) return [];
    const out = [];
    dirEntries().forEach((e) => {
      const u = e.username.toLowerCase(), n = e.name.toLowerCase();
      let score = -1;
      if (uname) { if (u === lq) score = 0; else if (u.startsWith(lq)) score = 1; else if (u.includes(lq)) score = 2; }
      else { if (n.startsWith(lq)) score = 0; else if (n.includes(lq)) score = 1; else if (u.startsWith(lq)) score = 2; else if (u.includes(lq)) score = 3; }
      if (score < 0) return;
      out.push({ name: e.name, username: e.username, has: hasChat(s, e.name), score, exact: uname && u === lq });
    });
    out.sort((a, b) => (a.has ? 1 : 0) - (b.has ? 1 : 0) || a.score - b.score || a.name.localeCompare(b.name));
    return out;
  }
  /* Opening is the one place a chat is created: an existing conversation is focused, and only a
     person with none gets a new, empty one at the top of the list. */
  function openPersonChat(s, name) {
    let i = s.chats.findIndex((c) => c.name === name);
    if (i === -1) { s.chats.unshift({ name, unread: 0, msgs: [], cur: 0, top: 0 }); i = 0; }
    s.chat = i; s.right = 'conv'; s.focus = 'conv';
    s.line = null; s.newchat = null; s.search = null; s.vis = null;
  }
  /* The prompt's two openings: `/` on the chat list, and `:new <query>` prefilled from anywhere. */
  function openNewChat(s, prefill) {
    openLine(s, { kind: 'newchat' });
    const p = prefill || '';
    s.line.buf = p; s.line.pos = p.length;
  }
  /* ⏎ from the prompt: the query goes out, and the candidates come back on the network tick. A
     single answer, or an exact username, opens at once and never becomes a list. A query opening
     with `!` stands in for a failed search, so the failure sentence has a frame to be seen in. */
  function runNewChat(s, q) {
    const raw = (q || '').trim();
    s.line = null;
    const failed = raw[0] === '!';
    const hits = failed ? [] : newChatHits(s, raw);
    if (!failed && (hits.length === 1 || hits.some((h) => h.exact))) {
      openPersonChat(s, (hits.find((h) => h.exact) || hits[0]).name);
      return;
    }
    s.newchat = { q: raw, hits, at: 0, pending: true, failed: failed ? 'timeout' : '' };
    s.focus = 'newchat';
  }
  /* The status line's sentence while the prompt's list is up: the query, then where the request is.
     The candidate count and the failure sit here rather than in a flash, because the list is a
     state the reader is looking at. */
  function newChatStatus(s) {
    const n = s.newchat, q = '/' + n.q;
    if (n.pending) return q + ' - searching...';
    if (n.failed) return q + ' - no candidates (search failed: ' + n.failed + ')';
    if (!n.hits.length) return q + ' - no candidates';
    return q + ' - ' + n.hits.length + ' candidate' + (n.hits.length === 1 ? '' : 's');
  }
  /* the results list's own keys: move and wrap, open, close. */
  function newchatKey(s, k) {
    if (common(s, k)) return;
    const n = s.newchat && !s.newchat.pending ? s.newchat.hits.length : 0;
    if (k === 'Escape') { s.newchat = null; s.focus = 'list'; return; }
    if (k === 'j' || k === 'Down') { if (n) s.newchat.at = (s.newchat.at + 1) % n; return; }
    if (k === 'k' || k === 'Up') { if (n) s.newchat.at = (s.newchat.at + n - 1) % n; return; }
    if (k === 'Enter') { const h = s.newchat && s.newchat.hits[s.newchat.at]; if (h) openPersonChat(s, h.name); return; }
  }

  /* ---------- the line: the one input bar ---------- */
  const MULTI = { message: 1, reply: 1, edit: 1 };
  function openLine(s, o) {
    const from = s.focus;
    const d = s.draft;
    const own = o.kind === 'message' || o.kind === 'reply' || o.kind === 'edit' || ['name', 'username', 'bio'].includes(o.kind);
    if (own && d && d.ctx === ctx(s)) {
      if (o.kind === 'message') { s.line = { kind: d.kind, buf: d.buf, pos: d.pos, mode: 'insert', from, ref: d.ref, origin: d.origin }; s.draft = null; s.focus = 'input'; return; }
      refuse(s, 'A draft is waiting: i to continue, ⏎ to discard.'); return;
    }
    const buf = o.buf || '';
    s.line = { kind: o.kind, buf, pos: buf.length, mode: 'insert', from, ref: o.ref, origin: buf, pend: '' };
    s.focus = 'input';
  }
  function closeLine(s) {
    const L = s.line; s.line = null;
    s.focus = MULTI[L.kind] ? 'conv' : ['name', 'username', 'bio'].includes(L.kind) ? 'settings' : L.from;
  }
  function leaveLine(s) {
    const L = s.line;
    if (L.from === 'signin') { L.mode = 'normal'; L.pos = clamp(L.pos, 0, Math.max(0, L.buf.length - 1)); return; }
    const keeps = MULTI[L.kind] || ['name', 'username', 'bio'].includes(L.kind);
    if (keeps && L.buf !== '' && L.buf !== L.origin) s.draft = { kind: L.kind, buf: L.buf, pos: L.pos, ref: L.ref, origin: L.origin, ctx: ctx(s) };
    closeLine(s);
  }

  const cls = (c) => (/\s/.test(c) ? 0 : /\w/.test(c) ? 1 : 2);
  function wFwd(b, p) { let i = p; if (i >= b.length) return i; const c0 = cls(b[i]); if (c0) while (i < b.length && cls(b[i]) === c0) i++; while (i < b.length && cls(b[i]) === 0) i++; return i; }
  function wBack(b, p) { let i = p; if (i > 0) i--; while (i > 0 && cls(b[i]) === 0) i--; const c0 = cls(b[i]); while (i > 0 && cls(b[i - 1]) === c0) i--; return i; }
  function wEnd(b, p) { let i = p + 1; while (i < b.length && cls(b[i]) === 0) i++; if (i >= b.length) return Math.max(0, b.length - 1); const c0 = cls(b[i]); while (i + 1 < b.length && cls(b[i + 1]) === c0) i++; return i; }
  function lineStart(b, p) { return b.lastIndexOf('\n', p - 1) + 1; }
  function lineEnd(b, p) { const i = b.indexOf('\n', p); return i < 0 ? b.length : i; }

  function lineKey(s, k) {
    const L = s.line;
    /* ^W is a prefix with a bare fallback, the pattern g/gg already uses: ^W h and ^W l walk
       the panes, and any other follower makes ^W mean what it always meant — leave the line —
       while that follower is dropped, the way an unfinished command is in Vim. */
    if (L.pendW) {
      L.pendW = false;
      const h = s.view === 'chat' && (k === 'h' || k === 'l');
      leaveLine(s);
      if (h) s.focus = k === 'h' ? 'list' : s.right === 'profile' ? 'profile' : s.right === 'settings' ? 'settings' : 'conv';
      return;
    }
    if (k === 'C-w') { L.pendW = true; return; }
    if (L.mode === 'insert') insKey(s, L, k);
    else if (L.mode === 'normal') norKey(s, L, k);
    else visLineKey(s, L, k);
  }
  function completionOf(L) {
    if (!L || L.mode !== 'insert' || !COMP_KIND[L.kind]) return null;
    const upto = L.buf.slice(0, L.pos);
    const m = upto.match(/:([A-Za-z0-9_]+)$/);
    if (!m) return null;
    const q = m[1].toLowerCase();
    const hits = SHORTCODES.filter((sc) => sc.toLowerCase().startsWith(':' + q));
    if (!hits.length) return null;
    return { start: L.pos - m[0].length, q, hits };
  }
  function syncComp(L) {
    const c = completionOf(L);
    if (!c) { if (L) L.comp = null; return null; }
    const key = c.start + ':' + c.q + ':' + c.hits.join('|');
    const prev = L.comp;
    if (prev && prev.key === key) return prev.dismissed ? null : prev;
    L.comp = { key, start: c.start, q: c.q, hits: c.hits, at: 0, dismissed: false };
    return L.comp;
  }
  function activeComp(L) { return L && L.comp && !L.comp.dismissed ? L.comp : null; }
  function pickComp(L) {
    const c = L.comp, word = c.hits[c.at];
    L.buf = L.buf.slice(0, c.start) + word + L.buf.slice(L.pos);
    L.pos = c.start + word.length;
    L.comp = null;
  }
  function insKey(s, L, k) {
    syncComp(L);
    const live = activeComp(L);
    if (live) {
      if (k === 'Up') { live.at = (live.at + live.hits.length - 1) % live.hits.length; return; }
      if (k === 'Down') { live.at = (live.at + 1) % live.hits.length; return; }
      if (k === 'Tab' || k === 'Enter') { pickComp(L); return; }
      if (k === 'Escape') { live.dismissed = true; return; }
    }
    const b = L.buf;
    switch (k) {
      case 'Escape': L.mode = 'normal'; if (L.pos > 0 && b[L.pos - 1] !== '\n') L.pos--; L.pos = clamp(L.pos, 0, Math.max(0, b.length - 1)); return;
      case 'Enter': submit(s); return;
      case 'S-Enter': case 'C-j': if (MULTI[L.kind]) { L.buf = b.slice(0, L.pos) + '\n' + b.slice(L.pos); L.pos++; } return;
      case 'Backspace': if (L.pos > 0) { L.buf = b.slice(0, L.pos - 1) + b.slice(L.pos); L.pos--; } return;
      case 'Delete': L.buf = b.slice(0, L.pos) + b.slice(L.pos + 1); return;
      case 'Left': L.pos = Math.max(0, L.pos - 1); return;
      case 'Right': L.pos = Math.min(b.length, L.pos + 1); return;
      case 'Home': L.pos = lineStart(b, L.pos); return;
      case 'End': L.pos = lineEnd(b, L.pos); return;
      case 'Up': case 'Down': vert(L, k === 'Up' ? -1 : 1); return;
    }
    if (k.length === 1) { L.buf = b.slice(0, L.pos) + k + b.slice(L.pos); L.pos++; }
    syncComp(L);
  }
  function vert(L, d) {
    const rows = barRows(L), [r, c] = caretRC(rows, L.pos), t = r + d;
    if (t < 0 || t >= rows.length) return;
    L.pos = Math.min(rows[t].s + c, rows[t].e);
  }
  function motion(L, k) {
    const b = L.buf, p = L.pos, top = Math.max(0, b.length - 1);
    switch (k) {
      case 'h': case 'Left': return Math.max(lineStart(b, p), p - 1);
      case 'l': case 'Right': return Math.min(Math.max(lineStart(b, p), lineEnd(b, p) - 1), p + 1);
      case 'w': return Math.min(top, wFwd(b, p));
      case 'b': return wBack(b, p);
      case 'e': return wEnd(b, p);
      case '0': return lineStart(b, p);
      case '$': return Math.max(lineStart(b, p), lineEnd(b, p) - 1);
      case 'G': return top;
    }
    return null;
  }
  function norKey(s, L, k) {
    const p = L.pend; L.pend = '';
    let b = L.buf;
    if (p === 'd' && k === 'w') { const e = wFwd(b, L.pos); s.reg = b.slice(L.pos, e); L.buf = b.slice(0, L.pos) + b.slice(e); L.pos = clamp(L.pos, 0, Math.max(0, L.buf.length - 1)); return; }
    if (p === 'c' && k === 'c') { s.reg = b; L.buf = ''; L.pos = 0; L.mode = 'insert'; return; }
    if (p === 'g' && k === 'g') { L.pos = 0; return; }
    const mv = motion(L, k);
    if (mv !== null) { L.pos = mv; return; }
    switch (k) {
      case 'Escape': leaveLine(s); return;
      case 'Enter': submit(s); return;
      case 'i': L.mode = 'insert'; return;
      case 'a': L.mode = 'insert'; L.pos = Math.min(b.length, L.pos + 1); return;
      case 'x': if (b.length) { s.reg = b[L.pos]; L.buf = b.slice(0, L.pos) + b.slice(L.pos + 1); L.pos = clamp(L.pos, 0, Math.max(0, L.buf.length - 1)); } return;
      case 'p': if (s.reg) { const at = b.length ? L.pos + 1 : 0; L.buf = b.slice(0, at) + s.reg + b.slice(at); L.pos = at + s.reg.length - 1; } return;
      case 'd': case 'c': case 'g': L.pend = k; return;
      case 'v': L.mode = 'visual'; L.anchor = L.pos; return;
    }
  }
  function visLineKey(s, L, k) {
    const mv = motion(L, k);
    if (mv !== null) { L.pos = mv; return; }
    const lo = Math.min(L.anchor, L.pos), hi = Math.max(L.anchor, L.pos);
    if (k === 'Escape') { L.mode = 'normal'; return; }
    if (k === 'y') { s.reg = L.buf.slice(lo, hi + 1); L.mode = 'normal'; L.pos = lo; }
    if (k === 'd') { s.reg = L.buf.slice(lo, hi + 1); L.buf = L.buf.slice(0, lo) + L.buf.slice(hi + 1); L.mode = 'normal'; L.pos = clamp(lo, 0, Math.max(0, L.buf.length - 1)); }
  }

  /* ---------- submit ---------- */
  function submit(s) {
    const L = s.line, buf = L.buf;
    switch (L.kind) {
      case 'message': case 'reply': {
        if (!buf.trim()) return refuse(s, 'Nothing to send.');
        const c = chat(s), m = { from: 'you', text: buf, status: 'sending…', live: true };
        if (c.msgs.length && c.msgs[c.msgs.length - 1].at != null) m.at = Math.max(NOW, c.msgs[c.msgs.length - 1].at);
        if (L.kind === 'reply') m.reply = L.ref;
        c.msgs.push(m); c.cur = c.msgs.length - 1; s.draft = null; s.vis = null; closeLine(s); return;
      }
      case 'edit': chat(s).msgs[L.ref.idx].text = buf; s.draft = null; closeLine(s); return;
      case 'find': closeLine(s); runSearch(s, buf); return;
      case 'newchat': runNewChat(s, buf); return;
      case 'command': {
        const c = buf.trim().replace(/^:/, ''); closeLine(s);
        if (c === 'settings') openSettings(s);
        else if (c === 'new' || c.startsWith('new ')) openNewChat(s, c.replace(/^new\s*/, ''));
        else if (c === 'signin') beginSignin(s);
        else if (c === 'q' || c === 'quit') common(s, 'q');
        else if (c) refuse(s, 'Not a command: ' + c);
        return;
      }
      case 'name': if (!buf.trim()) return refuse(s, 'Name cannot be empty.'); s.profile.name = buf.trim(); s.draft = null; closeLine(s); s.flash = 'Name saved.'; return;
      case 'username': {
        const u = buf.trim().replace(/^@/, '');
        if (!/^[A-Za-z][A-Za-z0-9_]{4,31}$/.test(u)) return refuse(s, 'Usernames are 5 to 32 letters, digits or _.');
        s.profile.username = u; s.draft = null; closeLine(s); s.flash = 'Username saved.'; return;
      }
      case 'bio': if (buf.length > 70) return refuse(s, 'Bio is limited to 70 characters.'); s.profile.bio = buf.trim(); s.draft = null; closeLine(s); s.flash = 'Bio saved.'; return;
      /* the three sign-in steps do not answer here: they enter the in-flight state, and the
         answer (the code, SESSION_PASSWORD_NEEDED, a refusal) lands in answer(). */
      case 'phone': case 'code': case 'pw': {
        if (L.kind === 'phone' && buf.replace(/\D/g, '').length < 7)
          return refuse(s, authSentence('PHONE_NUMBER_INVALID'));
        if (L.kind === 'pw' && !buf) return refuse(s, 'The password cannot be empty.');
        s.signin.pending = { kind: L.kind, value: L.kind === 'code' ? buf.trim() : L.kind === 'pw' ? buf : buf.trim() };
        s.signin.checking = true; s.signin.checkN = 0;
        s.line = null; s.focus = 'list';
        return;
      }
    }
  }
  function openSigninLine(s, kind) {
    /* the phone is the one field configuration can pre-fill; the code and the password are the
       reader's to type, so they open empty. */
    const pre = kind === 'phone' ? s.signin.phone : '';
    s.line = { kind, buf: pre, pos: pre.length, mode: 'insert', from: 'signin', origin: pre, pend: '' };
    s.focus = 'input';
  }
  function beginSignin(s) {
    s.view = 'signin'; s.right = 'conv'; s.confirm = null; s.vis = null; s.search = null; s.draft = null;
    s.chats = []; s.chat = 0; /* no client until credentials arrive, so no chats */
    const out = !!(s.signin && s.signin.out);
    s.signin = {
      out, step: 0, phone: s.profile.phone || '', code: '', bad: '', twofa: true, attempts: 3,
      checking: false, checkN: 0, pending: null, away: false, lost: false, action: ''
    };
    openSigninLine(s, 'phone');
  }
  function beginStale(s) {
    /* the stored session Telegram no longer knows: the row offers the way back in. */
    beginSignin(s);
    s.signin.action = SIGNOUT_ROW;
    s.flash = AUTH.AUTH_KEY_UNREGISTERED;
  }
  function beginLoggedOut(s) {
    /* the reader's own sign-out, not a revoked session: no AUTH_KEY_UNREGISTERED sentence, no
       action in the phone row. The session is gone, the list is empty, the card behind the
       field reads 'not signed in', and the status rests on 'signed out'. */
    beginSignin(s);
    s.signin.out = true;
    s.card = { sub: 'signedout', chat: 0, row: 0, col: 0, vis: null };
    s.flash = '';
  }
  function beginNoCreds(s) {
    /* no api_id and no api_hash: there is nothing to sign in with, so this is not a form. */
    s.view = 'nocreds'; s.right = 'conv'; s.confirm = null; s.vis = null; s.search = null; s.draft = null;
    s.chats = []; s.chat = 0; s.line = null; s.focus = 'nocreds';
  }
  function signinAway(s) {
    const A = s.signin, L = s.line;
    if (L && L.from === 'signin') {
      if (L.kind === 'code') A.lost = true;   /* the code is typed once and is not kept */
      s.line = null;
    }
    A.away = true; s.focus = 'list';
    s.flash = 'sign-in paused; Tab brings it back';
  }
  function signinBack(s) {
    const A = s.signin;
    A.away = false;
    openSigninLine(s, A.step === 0 ? 'phone' : A.step === 1 ? 'code' : 'pw');
    if (A.lost) { A.lost = false; s.flash = LOST_CODE; }
    else s.flash = '';
  }
  function signinCancel(s) {
    const A = s.signin;
    A.step = 0; A.code = ''; A.bad = '';
    openSigninLine(s, 'phone');
    s.flash = CANCEL_COST;
  }
  function signinDone(s) {
    const ph = s.signin.phone, n = fresh(); n.profile.phone = ph;
    Object.keys(s).forEach((x) => delete s[x]); Object.assign(s, n);
  }
  /* the answer to an in-flight request. In the app it arrives over the network; a scene walks it
     with <wait>, and the page's timer walks it live. */
  function answer(s) {
    /* the same tick ages the peer's typing note: it is not repeated, so it runs out */
    const open = s.chats && s.chats[s.chat];
    if (open && open.typing) open.typing--;
    if (s.newchat && s.newchat.pending) { s.newchat.pending = false; return s; }
    if (s.jump) return landJump(s);
    const A = s.signin, p = A && A.pending;
    if (!A || !A.checking || !p) return s;
    A.checking = false; A.checkN = 0; A.pending = null; s.flash = '';
    if (p.kind === 'phone') {
      A.phone = p.value; A.step = 1; A.bad = '';
      openSigninLine(s, 'code');
    } else if (p.kind === 'code') {
      if (p.value !== '42424') {
        A.bad = p.value; s.flash = authSentence('PHONE_CODE_INVALID');
        openSigninLine(s, 'code');
      } else {
        A.code = p.value; A.bad = '';
        if (A.twofa) { A.step = 2; A.attempts = 3; openSigninLine(s, 'pw'); } else signinDone(s);
      }
    } else if (p.value === 'hunter2!') {
      signinDone(s);
    } else {
      A.attempts = Math.max(0, A.attempts - 1);
      s.flash = authSentence('PASSWORD_HASH_INVALID', A.attempts);
      openSigninLine(s, 'pw');
    }
    return s;
  }

  /* the peer's events in the open conversation. A typing event sets the note and renews it; the
     peer's next message ends it, because the message is what the typing was for. */
  function peerTyping(s) {
    const c = s.view === 'chat' && chat(s);
    if (c) c.typing = TYPING_TICKS;
    return s;
  }
  function peerSays(s, text) {
    const c = s.view === 'chat' && chat(s);
    if (!c) return s;
    const rows = c.all || c.msgs, last = rows[rows.length - 1];
    const m = { from: 'them', text, id: rows.reduce((n, x) => Math.max(n, x.id || 0), 0) + 1 };
    if (last && last.at != null) m.at = NOW;
    if (c.all) c.all.push(m);
    if (!c.newer) c.msgs.push(m);
    c.typing = 0;
    return s;
  }

  /* ---------- settings panel ---------- */
  const ITEMS = ['name', 'username', 'bio', 'phone', 'birthday', 'switch', 'signout'];
  function settingsKey(s, k, p) {
    if (common(s, k)) return;
    switch (k) {
      case 'j': case 'Down': s.sset = clamp(s.sset + 1, 0, ITEMS.length - 1); break;
      case 'k': case 'Up': s.sset = clamp(s.sset - 1, 0, ITEMS.length - 1); break;
      case 'g': if (p === 'g') s.sset = 0; else s.pend = 'g'; break;
      case 'G': s.sset = ITEMS.length - 1; break;
      case 'Escape': case 'Tab': case 'C-w': s.right = 'conv'; s.focus = 'conv'; break;
      case 'h': s.focus = 'list'; break;
      case 'i': case 'a': case 'e': case 'Enter': act(s); break;
      case 'S': openCard(s, 'self'); break;
    }
  }
  function act(s) {
    const it = ITEMS[s.sset];
    if (it === 'name' || it === 'username' || it === 'bio') openLine(s, { kind: it, buf: s.profile[it] });
    else if (it === 'phone' || it === 'birthday') refuse(s, (it === 'phone' ? 'Phone' : 'Birthday') + ' is read-only: Telegram does not let an account set it.');
    else if (it === 'switch') ask(s, 'Sign in as someone else? This replaces this account. (y/n)', (x) => beginSignin(x));
    else ask(s, 'Sign out and forget this session? (y/n)', (x) => beginSignin(x));
  }

  /* ---------- modes ---------- */
  function modeName(s) {
    if (s.confirm) return 'CONFIRM';
    if (s.line) return s.line.mode === 'insert' ? 'INSERT' : s.line.mode === 'visual' ? 'VISUAL' : 'NORMAL';
    if (s.card && s.card.vis) return 'VISUAL';
    return s.vis ? 'VISUAL' : 'NORMAL';
  }
  function setMode(s, name) {
    if (s.view !== 'chat') return;
    s.confirm = null; s.line = null; s.vis = null; s.search = null; s.newchat = null; s.jump = null; s.pend = ''; s.flash = '';
    /* a card has no insert stage and its confirmation comes from d on the logout row */
    if (s.card) {
      s.card.vis = name === 'VISUAL' ? { mode: 'char', row: s.card.row, anchorCol: s.card.col } : null;
      return;
    }
    s.right = 'conv'; s.focus = 'conv';
    const c = chat(s);
    if (name === 'INSERT') key(s, 'i');
    else if (name === 'VISUAL') key(s, 'v');
    else if (name === 'CONFIRM') {
      let i = c.cur; while (i >= 0 && c.msgs[i].from !== 'you') i--;
      if (i < 0) { i = c.msgs.findIndex((m) => m.from === 'you'); }
      if (i >= 0) { c.cur = i; key(s, 'd'); key(s, 'd'); } else { key(s, 'q'); }
    }
  }
  const FOCUS_NAME = { list: 'chat list', conv: 'conversation', newchat: 'new-chat results', settings: 'editable profile', profile: 'profile card', input: 'input bar', nocreds: 'shell' };

  /* ---------- the grid ---------- */
  const newGrid = () => Array.from({ length: H }, () => Array.from({ length: W }, () => [' ', 't']));
  /* The grid is 80 cells wide whatever is in it, so a two-cell cluster occupies
     two of them: the cluster in the first, and an empty continuation in the
     second, which is the cell the terminal draws the rest of the glyph across.
     `cells` of an empty continuation is zero, which is what keeps a row of emoji
     80 cells wide and 80 columns short of nothing. A cell is never cut inside a
     cluster — a row that started on a lone `👨` would draw a stranger. */
  const CONT = '';
  function put(g, x, y, str, a) {
    let i = 0;
    for (const cluster of clusters(str)) {
      const w = cellsOf(cluster);
      if (x + i >= 0 && x + i < W && y >= 0 && y < H) g[y][x + i] = [cluster, a];
      for (let k = 1; k < w; k++) if (x + i + k < W) g[y][x + i + k] = [CONT, a];
      i += w;
    }
    return i;
  }
  function fill(g, x, y, w, a) { for (let i = 0; i < w; i++) put(g, x + i, y, ' ', a); }
  function box(g, x, y, w, h, title, lit, note) {
    const a = lit ? 'f' : 'b', n = note || '';
    put(g, x, y, '┌─', a); put(g, x + 2, y, ' ' + title, 't');
    if (n) put(g, x + 3 + cells(title), y, n, 'd');
    put(g, x + 3 + cells(title) + cells(n), y, ' ', 't');
    const used = 2 + cells(title) + cells(n) + 2;
    put(g, x + used, y, '─'.repeat(w - used - 1), a); put(g, x + w - 1, y, '┐', a);
    for (let r = 1; r < h - 1; r++) { put(g, x, y + r, '│', a); put(g, x + w - 1, y + r, '│', a); }
    put(g, x, y + h - 1, '└' + '─'.repeat(w - 2) + '┘', a);
  }
  /* Cut to `n` CELLS, never inside a cluster, and say so with an ellipsis that
     takes the last of them. A two-cell cluster that would straddle the edge is
     dropped whole rather than cut, which can leave the result a column short. */
  function trunc(t, n) {
    if (cells(t) <= n) return t;
    let out = '', used = 0;
    for (const c of clusters(t)) {
      const w = cellsOf(c);
      if (used + w > n - 1) break;
      out += c;
      used += w;
    }
    return out + '…';
  }

  /* ---------- the bar ---------- */
  function kindTitle(s, L) {
    if (L.kind === 'message') { const c = chat(s); return 'Message to ' + (c ? c.name : ''); }
    return { reply: 'Reply', edit: 'Edit', command: 'Command', find: 'Find', newchat: 'New chat', name: 'Name', username: 'Username', bio: 'Bio', phone: 'Phone', code: 'Login code', pw: 'Password' }[L.kind];
  }
  function barRows(L) {
    const prefix = L.kind === 'command' || L.kind === 'find';
    const shown = L.kind === 'pw' ? '•'.repeat(L.buf.length) : L.buf;
    return wrap(shown, prefix ? 74 : 76, 76, true);
  }
  function bar(s) {
    const L = s.line, d = s.draft;
    /* in flight there is no caret and nothing to type; the bar still shows what was sent. */
    if (s.view === 'signin' && s.signin && s.signin.checking) {
      const p = s.signin.pending || { kind: 'phone', value: '' };
      const conceal = p.kind === 'pw';
      const shown = conceal ? '•'.repeat(p.value.length) : p.value;
      const rows = wrap(shown, 76, 76, true), cr = caretRC(rows, shown.length);
      const n = Math.min(BAR_MAX, Math.max(1, rows.length));
      const start = rows.length > n ? clamp(cr[0] - n + 1, 0, rows.length - n) : 0;
      return { title: kindTitle(s, { kind: p.kind }), shown, rows, cr, n, start, lit: false, prefix: '', conceal, base: 'ltr' };
    }
    let title = 'Input', buf = '', pos = 0, lit = false, prefix = '', conceal = false;
    if (L) { lit = true; title = kindTitle(s, L); buf = L.buf; pos = L.pos; prefix = L.kind === 'command' ? ': ' : L.kind === 'find' ? '/ ' : ''; conceal = L.kind === 'pw'; }
    else if (d && d.ctx === ctx(s) && s.view === 'chat') { title = 'draft'; buf = d.buf; pos = d.pos; }
    const shown = conceal ? '•'.repeat(buf.length) : buf;
    const rows = wrap(shown, prefix ? 74 : 76, 76, true);
    /* A draft that reads right to left is permuted as a message row is: the row is
       still broken logically, the prefix stays chrome, and the caret moves onto the
       column it is drawn at. */
    const base = !conceal && s.bidi === 'visual' ? baseDir(shown) : 'ltr';
    let cr = caretRC(rows, pos);
    if (base === 'rtl') { const r = rows[cr[0]]; cr = [cr[0], caretCol(shown.slice(r.s, r.e), base, cr[1])]; }
    const n = Math.min(BAR_MAX, Math.max(1, rows.length));
    const start = rows.length > n ? clamp(cr[0] - n + 1, 0, rows.length - n) : 0;
    return { title, shown, rows, cr, n, start, lit, prefix, conceal, base };
  }
  function drawBar(g, s, b, y0) {
    const L = s.line;
    box(g, 0, y0, W, b.n + 2, b.title, b.lit);
    let lo = -1, hi = -2;
    if (L && L.mode === 'visual') { lo = Math.min(L.anchor, L.pos); hi = Math.max(L.anchor, L.pos); }
    const permute = b.base === 'rtl';
    const ink = (j) => {
      const ch = b.shown[j];
      let ce = ch, a = b.lit ? 't' : 'd';
      if (b.lit && !b.conceal && ch === ' ') { ce = '·'; a = 'd'; }
      if (j >= lo && j <= hi) a += 's';
      return [ce, a];
    };
    for (let i = 0; i < b.n; i++) {
      const ri = b.start + i, r = b.rows[ri], y = y0 + 1 + i; if (!r) continue;
      let x = 2;
      if (ri === 0 && b.prefix) { put(g, 2, y, b.prefix, 't'); x = 4; }
      if (permute) {
        /* the row's own text, in the order the terminal is handed it; the prefix
           above is the bar's chrome and is not permuted. */
        const body = b.shown.slice(r.s, r.e);
        let col = 0;
        for (const piece of visualPieces(body, b.base)) {
          for (let k = piece.s; k < piece.e; k++) {
            const [ce, a] = ink(r.s + k);
            put(g, x + col, y, ce, a);
            col += cells(ce);
          }
        }
      } else {
        for (let j = r.s; j < r.e; j++) {
          const [ce, a] = ink(j);
          put(g, x + (j - r.s), y, ce, a);
        }
      }
      if (b.lit && ri === b.cr[0]) {
        const cx = x + b.cr[1], cell = g[y][cx];
        if (cx < W - 1) g[y][cx] = [cell[0], cell[1] + (L.mode === 'insert' ? 'c' : 'n')];
      }
    }
  }

  /* ---------- status row ---------- */
  function statusText(s) {
    /* a confirmation outranks every hint, so a confirm hint has no state to be shown in. */
    if (s.confirm) return { t: ' ' + s.confirm.prompt, a: 't' };
    if (s.flash) return { t: ' ' + s.flash, a: 't' };
    if (s.view === 'signin' && s.signin && s.signin.checking)
      return { t: ' ' + CHECKING + ' — the request is in flight', a: 't' };
    const L = s.line;
    if (L && s.view === 'signin' && s.signin.out && s.signin.step === 0 && L.from === 'signin')
      return { t: ' ' + SIGNED_OUT, a: 'd' };
    if (L) {
      if (L.mode === 'insert' && activeComp(L)) return { t: HINT.complete, a: 'd' };
      return { t: L.mode === 'insert' ? HINT.typing : L.mode === 'normal' ? HINT.lnormal : HINT.lvisual, a: 'd' };
    }
    if (s.view === 'signin' && s.signin && s.signin.away) return { t: ' sign-in paused; Tab brings it back', a: 'd' };
    if (s.view === 'nocreds') return { t: HINT.cardReading, a: 'd' };
    if (s.vis) return { t: HINT.visual, a: 'd' };
    if (s.focus === 'newchat' && s.newchat) return { t: ' ' + newChatStatus(s), a: 't' };
    if (s.search) {
      const c = chat(s), h = s.search.hits, at = h.indexOf(c.cur), q = '/' + s.search.q;
      return { t: ' ' + (at >= 0 ? q + ' — match ' + (at + 1) + ' of ' + h.length : q + ' — ' + h.length + ' loaded'), a: 't' };
    }
    /* a jump in flight sits below a search and above a failed send's reason */
    if (s.jump) return { t: ' ' + s.jump.label, a: 't' };
    if (s.focus === 'settings') return { t: HINT.settings, a: 'd' };
    if (s.focus === 'profile') {
      const C = s.card;
      if (C.vis) return { t: ' ' + cardSel(s) + (C.vis.mode === 'rows' ? ' row(s)' : ' character(s)') + ' selected — Esc clears', a: 't' };
      const contactShell = C.sub === 'person' || C.sub === 'personReading' || C.sub === 'personFailed';
      const h = C.sub === 'self' ? HINT.cardSelf : contactShell ? HINT.cardContact : C.sub === 'reading' ? HINT.cardReading : HINT.cardSignedOut;
      return { t: h, a: 'd' };
    }
    if (s.focus === 'list') return { t: HINT.list, a: 'd' };
    const c = chat(s), m = c.msgs[c.cur];
    if (m && m.status && m.status.startsWith('failed')) return { t: ' [' + m.status + ']', a: 't' };
    if (s.draft && s.draft.ctx === ctx(s)) return { t: HINT.draft, a: 'd' };
    return { t: HINT.normal, a: 'd' };
  }
  const LABEL_ATTR = { NORMAL: 'N', INSERT: 'I', VISUAL: 'V', CONFIRM: 'X' };
  function drawStatus(g, s) {
    const m = modeName(s), st = statusText(s);
    put(g, 0, H - 1, ' ' + m.padEnd(LABEL - 1), LABEL_ATTR[m]);
    put(g, LABEL, H - 1, trunc(st.t, HINT_W), st.a);
  }

  /* ---------- chat list ---------- */
  function drawList(g, s, h) {
    box(g, 0, 0, LW, h, 'Chats (' + s.chats.length + ')', s.line ? false : s.focus === 'list');
    s.chats.slice(0, h - 2).forEach((c, i) => {
      const cur = i === s.chat, a = cur ? 'tr' : 't', y = 1 + i;
      fill(g, 1, y, LW - 2, a);
      put(g, 2, y, trunc(c.name, 17), a);
      if (c.unread) { const u = String(c.unread); put(g, LW - 3 - u.length + 1, y, u, cur ? 'tr' : 'd'); }
    });
  }

  /* ---------- conversation ---------- */
  const CW = 45; /* body columns after the 7-column tag, inside a 56-column panel */
  function convRows(s, c) {
    const rows = [], q = s.search ? s.search.q.toLowerCase() : '';
    if (c.older) rows.push({ load: 'Loading older…', mi: -1 });
    if (c.loading) rows.push({ load: 'Loading…', mi: -1 });
    c.msgs.forEach((m, mi) => {
      const prev = c.msgs[mi - 1], next = c.msgs[mi + 1];
      /* the first message of a calendar day is preceded by a row naming it: a row of its own, no
         message, so mi is -1 and the cursor never lands on it */
      if (m.at != null && (!prev || prev.at == null || dayOf(prev.at) !== dayOf(m.at))) rows.push({ sep: dayLabel(m.at), mi: -1 });
      const head = !joins(prev, m), tail = !joins(m, next);
      let qp = '';
      if (m.reply) qp = '> ' + (m.reply.unloaded ? '[message not loaded]' : trunc(m.reply.quote, 18)) + ' ‖ ';
      const full = qp + m.text, mask = new Array(full.length).fill(false);
      if (q) { const lt = m.text.toLowerCase(); for (let i = lt.indexOf(q); i >= 0; i = lt.indexOf(q, i + q.length)) for (let j = 0; j < q.length; j++) mask[qp.length + i + j] = true; }
      const first = rows.length;
      /* Which way this message reads is one answer for the whole message, asked
         across every paragraph of it: a message that opens with `---` and then
         says `שלום` is right-to-left, and the neutrals at the top cannot outvote
         it. A row of the message then inherits that base — a row of an all-neutral
         message carries no evidence of its own. */
      const base = baseDir(m.text);
      /* the sender label is the group's, so only its first row carries it */
      if (m.art && m.inline) {
        /* An inline sticker paints its picture instead of its token: one row per art
           line, in body ink — it is what the sender sent, not the program's word for
           it — so the cursor, selection, time and status rules below read them like
           any rows of the message. */
        m.art.forEach((line, i) => rows.push({ mi, first: i === 0, lab: i === 0 && head, full: line, qlen: 0, mask: [], base, s: 0, e: line.length, from: m.from, ph: false }));
      } else wrap(full, CW, CW, false).forEach((r, i) => rows.push({ mi, first: i === 0, lab: i === 0 && head, full, qlen: qp.length, mask, base, s: r.s, e: r.e, from: m.from, ph: !!m.ph }));
      /* the last row of a message carries its own status; the last row of a group also carries
         the peer's read state (outgoing only, never beside a status) and the group's time */
      const rcpt = tail && m.from === 'you' && !m.status && m.rcpt ? m.rcpt : '';
      const suf = m.status ? '[' + m.status + ']' : rcpt ? '[' + rcpt + ']' : '';
      const tm = tail && m.at != null ? clock(m.at) : '';
      if (suf || tm) {
        const sufA = m.status && m.status.startsWith('failed') ? 't' : 'd', last = rows[rows.length - 1];
        const need = cells(suf) + (suf && tm ? 1 : 0) + cells(tm);
        /* The trailing note goes on the last row when the text has left the room
           for it, and that row is measured in cells — the note and the text are
           drawn into the same 45 columns. The extra column is the engine's own
           margin over `wrap_decorated`, which reserves the note's width before the
           text is broken rather than after; it is kept because every row in this
           model was laid out with it, and dropping it moves notes onto rows the
           committed specimen shows elsewhere. */
        if (cells(last.full.slice(last.s, last.e)) + 1 + need <= CW) Object.assign(last, { suf, sufA, tm });
        else rows.push({ mi, first: false, lab: false, full: '', qlen: 0, mask: [], base, s: 0, e: 0, from: m.from, suf, sufA, tm });
      }
      rows[first].hasStart = true;
    });
    if (c.newer) rows.push({ load: 'Loading newer…', mi: -1 });
    return rows;
  }
  function drawConv(g, s, x0, h) {
    const c = chat(s), w = RW, V = h - 2;
    let title = 'Conversation (' + (c.msgs.length ? c.cur + 1 : 0) + '/' + c.msgs.length + ')';
    if (s.vis) { const [lo, hi] = range(s); title += ' · ' + (hi - lo + 1) + ' selected'; }
    if (s.search) title += ' · ' + s.search.hits.length + ' match(es)';
    /* the peer's note comes last and is dropped whole, never cut, when the title has no room for it */
    const typing = c.typing && cells(title) + cells(TYPING_NOTE) <= w - 5;
    box(g, x0, 0, w, h, title, s.line ? false : s.focus === 'conv', typing ? TYPING_NOTE : '');
    const rows = convRows(s, c);
    let fr = rows.findIndex((r) => r.mi === c.cur), lr = -1;
    rows.forEach((r, i) => { if (r.mi === c.cur) lr = i; });
    if (c.cur === 0 && c.older) fr = 0;
    if (fr > 0 && rows[fr - 1].sep) fr--;
    if (fr >= 0) { if (fr < c.top) c.top = fr; else if (lr >= c.top + V) c.top = Math.max(fr, lr - V + 1); }
    c.top = clamp(c.top, 0, Math.max(0, rows.length - V));
    let lo = -1, hi = -2; if (s.vis) [lo, hi] = range(s);
    rows.slice(c.top, c.top + V).forEach((r, i) => {
      const y = 1 + i;
      if (r.load) { put(g, x0 + 2, y, r.load, 'd'); return; }
      if (r.sep) {
        /* a rule the width of the text with the day set into it: no tag column, no fill */
        const lab = ' ' + r.sep + ' ', lx = Math.floor((52 - cells(lab)) / 2);
        put(g, x0 + 2, y, '─'.repeat(52), 'b');
        put(g, x0 + 2 + lx, y, lab, 'd');
        return;
      }
      const cur = r.mi === c.cur, sel = !cur && r.mi >= lo && r.mi <= hi;
      const mod = cur ? 'r' : sel ? 's' : '';
      fill(g, x0 + 1, y, w - 2, 't' + mod);
      if (r.lab) put(g, x0 + 2, y, r.from === 'you' ? '[you]' : '[them]', (cur ? 't' : 'd') + mod);
      /* The row is still one string laid out left to right from the tag gutter,
         whatever direction it reads — GAPS G12 reading (a): the sentence starts
         at the right of its own run, and the block is not right-aligned to the
         panel, because rows lay out from the gutter and the trailing note is
         already at the far end. What the mode changes is the order the pieces
         reach the grid in: `Terminal` hands the row over as it is stored and lets
         the terminal's shaper reverse a right-to-left run, and `Visual` permutes
         the row here. The row's WIDTH is identical either way, which is why one
         wrap serves both. */
      /* a placeholder is the program's word for what the sender did not write, so it is dim like
         the quote and the tag; a match still wins, and the cursor row is reversed whole */
      const ink = (j) => (r.mask[j] ? 'm' : (!cur && (j < r.qlen || r.ph) ? 'd' : 't')) + mod;
      if (r.base === 'rtl' && s.bidi === 'visual') {
        /* The quoted prefix is the conversation's own chrome and is left where the
           program leaves it — drawn first, as its own span, ahead of the text it
           introduces — so only the message's own text is permuted. */
        const head = r.full.slice(r.s, Math.min(r.e, r.qlen));
        const body = r.full.slice(Math.max(r.s, r.qlen), r.e);
        let x = x0 + 9;
        put(g, x, y, head, ink(r.s));
        x += cells(head);
        for (const piece of visualPieces(body, r.base)) {
          const t = body.slice(piece.s, piece.e);
          put(g, x, y, t, ink(Math.max(r.s, r.qlen) + piece.s));
          x += cells(t);
        }
      } else {
        for (let j = r.s; j < r.e; ) {
          const cl = clusters(r.full.slice(j, r.e))[0] || '';
          put(g, x0 + 9 + cells(r.full.slice(r.s, j)), y, cl, ink(j));
          j += cl.length;
        }
      }
      const tmw = r.tm ? cells(r.tm) : 0;
      if (r.tm) put(g, x0 + 2 + 52 - tmw, y, r.tm, (cur ? 't' : 'd') + mod);
      if (r.suf) put(g, x0 + 2 + 52 - tmw - (tmw ? 1 : 0) - cells(r.suf), y, r.suf, (cur ? 't' : r.sufA) + mod);
    });
    /* the body gives up its last interior column, and only when that body is wide enough */
    if (w - 2 >= MIN_BODY_WIDTH) paintScroll(g, x0 + w - 2, 1, V, c.top, rows.length);
  }
  function paintScroll(g, x, y0, track, top, total) {
    for (let i = 0; i < track; i++) put(g, x, y0 + i, '░', 'd');
    if (total <= 0 || track <= 0) return;
    const vis = Math.min(track, total);
    let th = Math.max(1, Math.round(track * vis / total));
    if (th > track) th = track;
    let t0 = 0;
    if (total > track) {
      const span = track - th;
      t0 = Math.round((top / (total - track)) * span);
      if (t0 > span) t0 = span;
    }
    for (let i = 0; i < th; i++) put(g, x, y0 + t0 + i, '█', 't');
  }

  /* ---------- settings ---------- */
  function drawSettings(g, s, x0, h) {
    const w = RW, P = s.profile;
    box(g, x0, 0, w, h, 'Profile · editable', s.line ? false : s.focus === 'settings');
    const rows = []; /* {t, a, item, tag, tagA, lab, labA} */
    const head = (t) => rows.push({ t, a: 'd', item: -1 });
    const blank = () => rows.push({ t: '', a: 't', item: -1 });
    const field = (item, label, val, ro) => {
      const parts = wrap(val, 30, 30, false);
      parts.forEach((r, i) => rows.push({ item, lab: i === 0 ? label : '', labA: ro ? 'd' : 't', t: val.slice(r.s, r.e), a: ro ? 'd' : 't', tag: i === 0 ? (ro ? '[read-only]' : '[edit]') : '', tagA: ro ? 'd' : 't' }));
    };
    const action = (item, label) => rows.push({ item, lab: label, labA: 't', t: '', a: 't' });
    head('Editable');
    field(0, 'Name', P.name, false); field(1, 'Username', '@' + P.username, false); field(2, 'Bio', P.bio, false);
    blank(); head('Read-only');
    field(3, 'Phone', P.phone, true); field(4, 'Birthday', P.birthday, true);
    wrap('Telegram does not let an account set its own phone number or birthday, so televim cannot change them.', 52, 52, false)
      .forEach((r) => rows.push({ t: 'Telegram does not let an account set its own phone number or birthday, so televim cannot change them.'.slice(r.s, r.e), a: 'd', item: -1, ind: true }));
    blank(); head('Account');
    action(5, 'Sign in as someone else'); action(6, 'Sign out');
    rows.slice(0, h - 3).forEach((r, i) => {
      const y = 1 + i, cur = r.item === s.sset, m = cur ? 'r' : '';
      if (r.item >= 0) fill(g, x0 + 1, y, w - 2, 't' + m);
      const ca = (cur ? 't' : null);
      if (r.lab) put(g, x0 + 2, y, r.lab, (ca || r.labA) + m);
      if (r.t) put(g, x0 + (r.item >= 0 ? 12 : 2), y, r.t, (ca || r.a) + m);
      if (r.tag) put(g, x0 + 2 + 52 - r.tag.length, y, r.tag, (ca || r.tagA) + m);
    });
    put(g, x0 + 2, h - 2, 'Esc, Tab, Ctrl+w leave. The cards read; this edits.', 'd');
  }

  /* ---------- the profile card: one widget, two subjects ---------- */
  function cardRows(s) {
    const C = s.card;
    if (!C) return [];
    /* no rows at all until the profile is read: the shell sentence replaces the card */
    if (C.sub === 'personReading' || C.sub === 'personFailed') return [];
    if (C.sub === 'self') {
      const P = s.profile;
      return [
        { kind: 'value', label: 'name', value: P.name },
        { kind: 'value', label: 'username', value: '@' + P.username },
        { kind: 'value', label: 'phone', value: P.phone },
        { kind: 'value', label: 'bio', value: P.bio },
        { kind: 'value', label: 'birthday', value: P.birthday },
        { kind: 'action', label: 'add account', act: 'add' },
        { kind: 'action', label: 'logout', act: 'logout' }
      ];
    }
    const c = s.chats[C.chat];
    if (!c) return [];
    const p = PERSON[c.name] || {}, rows = [{ kind: 'value', label: 'name', value: c.name }];
    /* slot 1 is reserved for the local colour row and is always absent here: Telegram has no
       field for it, the program has not been asked for one, and a row is only drawn when
       something says it. The slot is held so the field lands in place. */
    rows.push({ kind: 'colour', label: 'colour' });
    if (p.username) rows.push({ kind: 'value', label: 'username', value: '@' + p.username });
    if (p.bio) rows.push({ kind: 'value', label: 'bio', value: p.bio });
    if (p.birthday) rows.push({ kind: 'value', label: 'birthday', value: p.birthday });
    return rows;
  }
  const navRows = (rows) => rows.map((r, i) => (r.kind === 'colour' ? -1 : i)).filter((i) => i >= 0);
  const cardAt = (s) => {
    const rows = cardRows(s), nav = navRows(rows);
    const ri = nav.length ? nav[clamp(s.card.row, 0, nav.length - 1)] : -1;
    return { rows, nav, ri, r: rows[ri] || { kind: 'value', label: '', value: '' } };
  };
  function cardSel(t) {
    const C = t.card;
    if (!C.vis) return 0;
    /* charwise stays inside one row; j or k turns it into a set of rows, the same distinction
       the conversation makes between characters and messages. */
    return C.vis.mode === 'rows' ? Math.abs(C.row - C.vis.anchorRow) + 1 : Math.abs(C.col - C.vis.anchorCol) + 1;
  }
  function cardTitle(s) {
    const C = s.card;
    if (C.sub === 'signedout' || C.sub === 'reading') return 'Profile';
    if (C.sub === 'personReading' || C.sub === 'personFailed') return 'Profile · ' + (s.chats[C.chat] ? s.chats[C.chat].name : '');
    const rows = cardRows(s), nav = navRows(rows);
    let t = C.sub === 'self' ? 'Profile · you' : 'Profile · ' + (s.chats[C.chat] ? s.chats[C.chat].name : '');
    t += ' (' + (nav.length ? clamp(C.row, 0, nav.length - 1) + 1 : 0) + '/' + nav.length + ')';
    if (C.vis) t += ' · ' + cardSel(s) + ' selected';
    return t;
  }
  /* the inline position, as a display line and a column on it. The value is one logical line,
     so h and l are single-cell motions and the wrap is only how it is shown. */
  function cardCaret(value, col) {
    const segs = wrap(value, VAL_W, VAL_W, true);
    for (let i = 0; i < segs.length; i++) if (col <= segs[i].e) return [i, Math.max(0, col - segs[i].s)];
    const last = segs[segs.length - 1];
    return [segs.length - 1, Math.max(0, last.e - last.s)];
  }
  function shellLines(g, x0, h, head, body, action, headInk) {
    let y = 1;
    put(g, x0 + 2, y++, head, headInk || 't');
    if (body) wrap(body, 52, 52, true).forEach((r) => put(g, x0 + 2, y++, body.slice(r.s, r.e), 'd'));
    if (action) put(g, x0 + 2, y + 1, action, 't');
  }
  function drawCard(g, s, x0, h) {
    const w = RW, C = s.card;
    box(g, x0, 0, w, h, cardTitle(s), s.line ? false : s.focus === 'profile');
    if (C.sub === 'signedout') { shellLines(g, x0, h, NOT_SIGNED_IN, SIGNED_OUT_REASON, SET_CREDENTIALS); return; }
    if (C.sub === 'reading') { shellLines(g, x0, h, READING, READING_NOTE, ''); return; }
    if (C.sub === 'personReading') { shellLines(g, x0, h, CONTACT_READING, '', '', 'd'); return; }
    if (C.sub === 'personFailed') { shellLines(g, x0, h, CONTACT_UNAVAILABLE, CONTACT_FAILED_REASON, ''); return; }
    const { rows, nav, ri } = cardAt(s);
    const vis = C.vis ? {
      mode: C.vis.mode,
      rows: C.vis.mode === 'rows' ? [Math.min(C.vis.anchorRow, C.row), Math.max(C.vis.anchorRow, C.row)] : null,
      row: C.vis.mode === 'char' ? C.vis.row : -1,
      cols: C.vis.mode === 'char' ? [Math.min(C.vis.anchorCol, C.col), Math.max(C.vis.anchorCol, C.col)] : null
    } : null;
    const lines = [];
    rows.forEach((r, i) => {
      if (r.kind === 'colour') return;   /* the reserved slot draws nothing */
      const segs = r.kind === 'value' ? wrap(r.value, VAL_W, VAL_W, true) : [null];
      segs.forEach((seg, li) => lines.push({ r, i, seg, li }));
    });
    lines.slice(0, h - 2).forEach((ln, k) => {
      const y = 1 + k, cur = ln.i === ri;
      const rowSel = vis && vis.mode === 'rows' && ln.i >= vis.rows[0] && ln.i <= vis.rows[1] && !cur;
      const charSel = !!(vis && vis.mode === 'char' && ln.i === vis.row);
      const mod = cur && !charSel ? 'r' : rowSel ? 's' : '';
      fill(g, x0 + 1, y, w - 2, 't' + mod);
      if (ln.li === 0) {
        /* the cue: present, dim, one column, on the row it belongs to, exactly as the
           conversation puts [you]/[them] on a message's first row. */
        const ink = (cur && !charSel ? 't' : 'd') + mod;
        put(g, x0 + CUE_X, y, CUE, ink);
        put(g, x0 + LAB_X, y, trunc(ln.r.label, LAB_W), ink);
      }
      if (!ln.seg) return;
      for (let j = ln.seg.s; j < ln.seg.e; j++) {
        const inSel = charSel && j >= vis.cols[0] && j <= vis.cols[1];
        put(g, x0 + VAL_X + (j - ln.seg.s), y, ln.r.value[j], inSel ? 'ts' : 't' + mod);
      }
      if (cur && ln.r.kind === 'value') {
        const [li, col] = cardCaret(ln.r.value, C.col);
        if (li === ln.li) {
          const cx = x0 + VAL_X + col;
          if (cx > x0 && cx < x0 + w - 1) g[y][cx] = [g[y][cx][0], g[y][cx][1] + 'n'];
        }
      }
    });
  }

  function yankCard(s, text, n, unit) {
    s.reg = text;                        /* the register, then the offer to the terminal */
    s.card.vis = null;
    s.flash = n + ' ' + unit + ' yanked — register filled, OSC 52 offered';
  }
  function cardAct(s, r) {
    if (s.card.sub === 'self' && r.kind === 'action') {
      if (r.act === 'add') refuse(s, 'not yet: this build cannot add an account');
      /* a confirmation outranks a flash: it asks before it throws away the only secret the
         program holds, and y signs out. */
      else ask(s, 'Sign out and forget this session? (y/n)', beginLoggedOut);
      return;
    }
    if (s.card.sub === 'self') refuse(s, 'Not a row that acts: add account and logout are the two.');
    else refuse(s, 'Not yours: a contact card has no row you can act on.');
  }
  function cardVisKey(s, k, p) {
    const C = s.card, { r } = cardAt(s), val = r.kind === 'value' ? r.value : '';
    const cnt = s.count || 1; s.count = 0;
    if (p === 'y') return;
    if (k === 'g') { if (p === 'g') { C.row = 0; C.col = 0; } else s.pend = 'g'; return; }
    if (k === 'G') { C.row = cnt > 1 ? clamp(cnt - 1, 0, navRows(cardRows(s)).length - 1) : navRows(cardRows(s)).length - 1; C.col = 0; return; }
    if (k === 'j' || k === 'k' || k === 'Down' || k === 'Up') {
      /* a charwise selection extended by j/k becomes a set of rows */
      if (C.vis.mode === 'char') C.vis = { mode: 'rows', anchorRow: C.vis.row };
      const last = navRows(cardRows(s)).length - 1;
      C.row = clamp(C.row + cnt * (k === 'j' || k === 'Down' ? 1 : -1), 0, last); C.col = 0; return;
    }
    if (k === 'h' || k === 'l' || k === 'Left' || k === 'Right') {
      if (C.vis.mode === 'char') C.col = clamp(C.col + (k === 'l' || k === 'Right' ? 1 : -1), 0, Math.max(0, val.length - 1));
      return;
    }
    if (k === 'Escape' || k === 'v') { C.vis = null; return; }
    if (k === 'd' || k === 'e') {
      if (s.card.sub === 'self' && k === 'd') refuse(s, 'Not a row that acts: add account and logout are the two.');
      else if (s.card.sub === 'self') refuse(s, 'Not editable here: :settings opens the editable profile.');
      else refuse(s, 'Not yours: nothing on a contact card is editable.');
      return;
    }
    if (k === 'y') {
      if (C.vis.mode === 'rows') {
        const rows = cardRows(s), nav = navRows(rows);
        const lo = Math.min(C.vis.anchorRow, C.row), hi = Math.max(C.vis.anchorRow, C.row);
        const vals = [];
        for (let i = lo; i <= hi; i++) { const v = rows[nav[i]]; vals.push(v.kind === 'value' ? v.value : v.label); }
        yankCard(s, vals.join('\n'), hi - lo + 1, 'row(s)');
      } else {
        const lo = Math.min(C.vis.anchorCol, C.col), hi = Math.max(C.vis.anchorCol, C.col);
        yankCard(s, val.slice(lo, hi + 1), hi - lo + 1, 'character(s)');
      }
      return;
    }
  }
  function cardKey(s, k, p) {
    if (common(s, k)) return;
    const C = s.card;
    /* the shell cards are the whole program: there is nothing behind them to go back to, so
       only : and q answer, which is what their hint names. */
    if (C.sub === 'signedout' || C.sub === 'reading' || C.sub === 'personReading' || C.sub === 'personFailed') return;
    const { nav, r } = cardAt(s), n = nav.length;
    const val = r.kind === 'value' ? r.value : '';
    /* a count is a motion's multiplier and nothing else here; it costs one line because the
       digits are otherwise unbound on a card. */
    if (/^[1-9]$/.test(k)) { s.count = Math.min(99, (s.count || 0) * 10 + Number(k)); return; }
    const cnt = s.count || 1; s.count = 0;
    /* pending operators, the same shape as g/gg: yy is the row, z is dropped, ^W is a prefix */
    if (p === 'y') { if (k === 'y') yankCard(s, val || r.label, 1, 'row(s)'); return; }
    if (p === 'z') return;
    if (p === 'g') { if (k === 'g') { C.row = 0; C.col = 0; } return; }
    if (p === 'C-w') { if (k === 'h') s.focus = 'list'; return; }   /* nothing is drawn right of the card */
    if (C.vis) { cardVisKey(s, k, p); return; }
    switch (k) {
      case 'j': case 'Down': C.row = clamp(C.row + cnt, 0, n - 1); C.col = 0; break;
      case 'k': case 'Up': C.row = clamp(C.row - cnt, 0, n - 1); C.col = 0; break;
      case 'g': if (p === 'g') { C.row = 0; C.col = 0; } else s.pend = 'g'; break;
      case 'G': C.row = cnt > 1 ? clamp(cnt - 1, 0, n - 1) : n - 1; C.col = 0; break;
      /* h is inline motion and the way back at once: a card is one column of values, so at the
         row's first cell there is nowhere left to move and h takes its other meaning. */
      case 'h': case 'Left': if (C.col > 0) C.col--; else leaveCard(s); break;
      case 'l': case 'Right': if (val) C.col = Math.min(Math.max(0, val.length - 1), C.col + 1); break;
      case 'v': C.vis = r.kind === 'value'
        ? { mode: 'char', row: C.row, anchorCol: C.col }
        : { mode: 'rows', anchorRow: C.row }; break;
      case 'y': s.pend = 'y'; break;
      case 'z': s.pend = 'z'; break;   /* zz/zt/zb: a card cannot scroll, so nothing moves */
      case 'C-w': s.pend = 'C-w'; break;
      case 'd': cardAct(s, r); break;
      case 'e':
        if (C.sub === 'self') refuse(s, 'Not editable here: :settings opens the editable profile.');
        else refuse(s, 'Not yours: nothing on a contact card is editable.');
        break;
      case 'p': refuse(s, 'p: a card has no buffer to paste into'); break;
      case 'A': openCard(s, 'person'); break;
      case 'S': openCard(s, 'self'); break;
      case 'Escape': leaveCard(s); break;
      case 'Tab': togglePane(s); break;
    }
  }

  /* ---------- sign in ---------- */
  function drawSignin(g, s, x0, h) {
    const w = RW, S = s.signin, step = S.step, busy = S.checking;
    box(g, x0, 0, w, h, 'Sign in', false);
    const line = (y, str, a) => put(g, x0 + 2, y, trunc(str, 52), a);
    line(1, 'Sign in to Telegram', 't');
    const expl = 'televim signs in to one account. Telegram asks for what it needs, in order.';
    wrap(expl, 52, 52, false).forEach((r, i) => put(g, x0 + 2, 2 + i, expl.slice(r.s, r.e), 'd'));
    /* the phone row and the code row exist from the start; the password row is drawn only when
       Telegram answered SESSION_PASSWORD_NEEDED, which is the account's own doing. A no-2FA
       account therefore has two rows for the whole flow. */
    const rows = [['Phone', S.phone, 0], ['Login code', S.code, 1]];
    if (step === 2) rows.push([pwLabel(S.attempts), '', 2]);
    rows.forEach(([lab, val, i]) => {
      const y = 5 + i, done = i < step, cur = i === step && !busy;
      put(g, x0 + 2, y, trunc(lab, 52), cur || done ? 't' : 'd');
      if (busy && i === step) { put(g, x0 + 16, y, '· · ·', 'd'); return; }
      if (i === 1 && cur && S.bad) {
        put(g, x0 + 16, y, trunc(S.bad, 20), 'd');
        put(g, x0 + 2 + 52 - 12, y, '[wrong code]', 't');
      } else if (i === 0 && S.action) {
        put(g, x0 + 16, y, trunc(val, 20), 't');
        put(g, x0 + 2 + 52 - cells(S.action), y, S.action, 't');
      } else if (done) {
        put(g, x0 + 16, y, i === 2 ? '•'.repeat(8) : trunc(val, 22), 't');
        put(g, x0 + 2 + 52 - 4, y, '[ok]', 'd');
      }
    });
    const y = 9;
    if (busy) { line(y, CHECKING, 't'); line(y + 1, 'The request is in flight; ⏎ is refused.', 'd'); }
    else if (step === 0) {
      line(y, 'Include the country code.', 'd');
      if (S.phone) line(y + 1, 'The configured number is filled in.', 'd');
    } else if (step === 1) {
      line(y, 'Telegram sent a login code to ' + S.phone + '.', 'd');
    } else {
      line(y, 'Two-step verification is on.', 'd');
      line(y + 1, 'Password hint: street I grew up on', 't');
    }
  }
  function drawNoCreds(g, s, x0, h) {
    /* not a form: one sentence, and no field, because a field the reader cannot use is worse
       than no field. */
    box(g, x0, 0, RW, h, 'Sign in', s.focus === 'nocreds');
    put(g, x0 + 2, 1, 'Sign in to Telegram', 't');
    wrap(NO_CREDENTIALS, 52, 52, true).forEach((r, i) => put(g, x0 + 2, 3 + i, NO_CREDENTIALS.slice(r.s, r.e), 'd'));
  }
  function drawComplete(g, s, y0) {
    const c = activeComp(s.line);
    if (!c) return;
    const n = Math.min(5, c.hits.length);
    const widest = c.hits.reduce((m, h) => Math.max(m, cells(h)), cells(':' + c.q));
    const bw = Math.min(36, Math.max(16, widest + 4));
    const bh = n + 2;
    const y = y0 - bh - 1; /* stay inside the pane; the row above the bar is its border */
    if (y < 1) return;
    const x = 2;
    box(g, x, y, bw, bh, ':' + c.q, false);
    for (let i = 0; i < n; i++) {
      const on = i === c.at, a = on ? 'ts' : 't';
      fill(g, x + 1, y + 1 + i, bw - 2, a);
      put(g, x + 2, y + 1 + i, trunc(c.hits[i], bw - 4), a);
    }
  }
  /* Draw `text` at x,y with every case-insensitive occurrence of `frag` in the match ink; the rest
     takes `base`. A match under the highlighted row keeps the selection background and takes the
     match ink (the palette's a_match_under_a_selection), which is what `mark` is passed. */
  function putMark(g, x, y, text, frag, base, mark) {
    const parts = clusters(text), bounds = [];
    let acc = '';
    parts.forEach((p) => { bounds.push([acc.length, acc.length + p.length]); acc += p; });
    const marks = new Array(parts.length).fill(false), lc = acc.toLowerCase(), lf = (frag || '').toLowerCase();
    if (lf) {
      let from = 0, idx;
      while ((idx = lc.indexOf(lf, from)) !== -1) {
        for (let i = 0; i < bounds.length; i++) if (bounds[i][1] > idx && bounds[i][0] < idx + lf.length) marks[i] = true;
        from = idx + lf.length;
      }
    }
    let col = 0;
    parts.forEach((p, i) => { col += put(g, x + col, y, p, marks[i] ? mark : base); });
    return col;
  }
  /* The new-chat results overlay: the completion's shape, but drawn over the chat-list column
     alone, growing up from that column's bottom and never taller than it. It is drawn only after
     ⏎ has gone out — never while the prompt line has the focus — because the candidates are the
     network's answer, not a filter the prompt keeps. Each row carries the display name, the handle
     when there is one, and whether a conversation already exists. */
  function drawNewChat(g, s, y0) {
    const n = s.newchat;
    if (s.focus !== 'newchat' || !n || n.pending || !n.hits.length) return;
    const rows = Math.min(n.hits.length, Math.max(0, y0 - 2)); /* the column is only so tall */
    if (!rows) return;
    const bw = LW, bh = rows + 2, y = y0 - bh;
    if (y < 0) return;
    box(g, 0, y, bw, bh, 'New chat', false);
    const byU = n.q[0] === '@', frag = byU ? n.q.slice(1) : n.q;
    for (let i = 0; i < rows; i++) {
      const h = n.hits[i], on = i === n.at, yy = y + 1 + i;
      const stand = h.has ? 'chat' : 'new', sw = cells(stand);
      /* reverse video is the highlight; on it the quieter text takes body ink, because a dim
         foreground under reverse video would become a dim background. */
      const base = on ? 'tr' : 't', quiet = on ? 'tr' : 'd', mark = on ? 'mr' : 'm';
      fill(g, 1, yy, bw - 2, base);
      const handle = h.username ? ' @' + h.username : '';
      const budget = bw - 4 - sw; /* the left field, less the standing and the gap before it */
      const name = trunc(h.name, Math.max(1, budget - cells(handle)));
      let col = putMark(g, 2, yy, name, byU ? '' : frag, base, mark);
      const room = Math.max(0, budget - col);
      if (handle && room) col += putMark(g, 2 + col, yy, trunc(handle, room), byU ? frag : '', quiet, mark);
      put(g, bw - 1 - sw, yy, stand, quiet);
    }
  }

  /* ---------- frame ---------- */
  function render(s) {
    const g = newGrid();
    if (s.line) syncComp(s.line);
    if (s.view === 'quit') {
      put(g, 0, 0, '$ televim', 't'); put(g, 0, 1, '$ ', 't'); g[1][2] = [' ', 'tn']; return g;
    }
    const b = bar(s), top = H - 1 - (b.n + 2);
    drawList(g, s, top);
    if (s.view === 'signin' && s.signin.out && s.signin.away) drawCard(g, s, LW, top);
    else if (s.view === 'signin') drawSignin(g, s, LW, top);
    else if (s.view === 'nocreds') drawNoCreds(g, s, LW, top);
    else if (s.right === 'settings') drawSettings(g, s, LW, top);
    else if (s.right === 'profile') drawCard(g, s, LW, top);
    else drawConv(g, s, LW, top);
    drawComplete(g, s, top);
    drawNewChat(g, s, top);
    drawBar(g, s, b, top);
    drawStatus(g, s);
    return g;
  }

  /* ---------- output ---------- */
  const SPECIAL = /[^\x20-\x7e┌┐└┘─│]/;
  const CLS = { t: 'ft', d: 'fd', m: 'fm', f: 'ff', b: 'fb', N: 'fN', I: 'fI', V: 'fV', X: 'fX', r: 'rv', s: 'sl', c: 'ci', n: 'cn' };
  const esc = (c) => Array.from(c).map((x) => (x === '&' ? '&amp;' : x === '<' ? '&lt;' : x === '>' ? '&gt;' : x)).join('');
  function toHTML(g) {
    return g.map((row) => {
      let out = '', run = '', ra = null;
      const flush = () => { if (run) out += '<span class="' + Array.from(ra).map((x) => CLS[x]).join(' ') + '">' + run + '</span>'; run = ''; };
      row.forEach(([ch, a]) => {
        if (a !== ra || /[cn]/.test(a)) { flush(); ra = a; }
        /* A two-cell cluster is one glyph in a box two columns wide, and the
           continuation cell beside it is that second column — an empty one-ch
           box, which is what keeps the row the width the grid says it is. */
        if (ch === CONT) run += '<i class="w"></i>';
        else run += SPECIAL.test(ch) ? '<i class="w">' + esc(ch) + '</i>' : esc(ch);
        if (/[cn]/.test(a)) flush();
      });
      flush();
      return out;
    }).join('\n');
  }
  /* The row as text, for measuring and for reading a frame back. A continuation
     cell contributes nothing, which is the whole reason a row of emoji is 80
     CELLS wide and fewer than 80 characters: the check measures cells, because
     that is what a terminal draws. */
  const toText = (g) => g.map((r) => r.map((c) => c[0]).join(''));

  /* ---------- scripted starts: real keystrokes through the real handler ---------- */
  const PEER_SAYS = 'The side gate is open.';
  const TOK = { Esc: 'Escape', CR: 'Enter', Tab: 'Tab', BS: 'Backspace', 'C-j': 'C-j', 'C-w': 'C-w', 'C-o': 'C-o', 'C-i': 'C-i', 'S-CR': 'S-Enter' };
  function feed(s, script) {
    for (let i = 0; i < script.length; i++) {
      if (script[i] === '<') {
        const e = script.indexOf('>', i), name = script.slice(i + 1, e);
        if (name === 'wait') answer(s);
        else if (name === 'typing') peerTyping(s);
        else if (name === 'peer') peerSays(s, PEER_SAYS);
        else key(s, TOK[name]);
        i = e;
      } else key(s, script[i]);
    }
    return s;
  }
  /* the walk to the right-to-left chat: focus the list, then down to its last row */
  const RTL_WALK = '<Tab>' + 'j'.repeat(12);
  const DRAFT = 'i' + 'I have the concert tickets and the blue folder. If the side gate is shut, I will ring the bell twice.<C-j>Ten minutes, not more.';
  /* a Hebrew draft for the input bar: it reads right to left, so the bar permutes
     it and the caret after it lands on the visual end of the line. */
  const RTL_LINE = 'i' + 'שלום, אני בדרך הביתה בעוד עשר דקות';
  const SCENES = [
    { id: 'conv', name: 'Conversation', variants: [
      { name: 'Normal', keys: '' },
      { name: 'Chat list focused', keys: '<Tab>' },
      { name: 'Loading newer', keys: '<Tab>jjl' },
      { name: 'Loading', keys: '<Tab>jjjjl' }] },
    /* Grace Hopper's chat is the one with a clock. G jumps the cursor to a message, which brings
       its rows (and a day's separator) into view; every frame goes through the real handler. */
    { id: 'grouping', name: 'Groups · days · receipts', variants: [
      { name: 'Newest: [read], [delivered], [sending…], [failed]', keys: '<Tab>j<Tab>' },
      { name: 'Cursor inside a group: one state, one time', keys: '<Tab>j<Tab>kk' },
      { name: 'Oldest: full date, weekday, Yesterday, Today', keys: '<Tab>j<Tab>kkkkkkkkkkkkkkk' }] },
    /* gd follows a reply's quote. Ada's chat: the cursor starts on message 106; `kkk` is the reply
       'The one by the window.', whose target is loaded, and `jj` is 'Sorry, I missed your earlier
       message.', whose target the window does not hold, so it is fetched (<wait> is the answer). */
    { id: 'jump', name: 'Jump to the quoted message', variants: [
      { name: 'gd: the target is loaded, only the cursor moves', keys: 'kkkgd' },
      { name: 'gd: the target is not loaded, a fetch is in flight', keys: 'jjgd' },
      { name: 'gd: the page arrived, the cursor rests on the target', keys: 'jjgd<wait>' },
      { name: 'Ctrl-o: back to where the reader left (loaded)', keys: 'kkkgd<C-o>' },
      { name: 'Ctrl-i: forward again', keys: 'kkkgd<C-o><C-i>' },
      { name: 'Ctrl-o: back needs a fetch too', keys: 'jjgd<wait><C-o>' },
      { name: 'Ctrl-o: back on the reply that was left', keys: 'jjgd<wait><C-o><wait>' },
      { name: 'gd on a message that is not a reply', keys: 'gd' }] },
    { id: 'visual', name: 'Visual', variants: [
      { name: 'Two messages, search live', keys: '/tickets<CR>vj' }] },
    /* Media-only messages. The chat is Ken Thompson's, put first by the `media` start; the cursor
       begins on the last message and `k` walks up to the one each frame is about. */
    { id: 'media', name: 'Media placeholders', start: 'media', variants: [
      { name: '[image] alone', keys: 'kkkkkkkk' },
      { name: '[voice]', keys: 'kkkkkk' },
      { name: '[file]', keys: 'kkkk' },
      { name: 'A caption replaces the placeholder', keys: 'kkk' },
      { name: 'Wrapped: the placeholder row yields its status to the next row', keys: '' },
      { name: 'Visual: [voice] selected, [file] under the cursor', keys: 'kkkkkkvjj' },
      { name: 'Visual, yanked: the placeholder is text', keys: 'kkkkkkvjjy' },
      { name: 'All five kinds in one window', keys: 'kk' }] },
    /* A static sticker. Ken Thompson's chat is put first by the `sticker` start (or
       `stickerFallback` for the token frame); the cursor begins on the sticker and
       every frame goes through the real handler. */
    { id: 'sticker', name: 'Sticker block', start: 'sticker', variants: [
      { name: 'Inline: a static sticker as a bounded block', keys: '' },
      { name: 'Visual, yanked: the block yanks [sticker]', keys: 'vy' },
      { name: 'Fallback: [sticker] when the flag is off or decoding fails', start: 'stickerFallback', keys: '' }] },
    { id: 'insert', name: 'Insert', variants: [
      { name: 'Composing', keys: DRAFT },
      { name: 'Esc: line Normal', keys: DRAFT + '<Esc>' },
      { name: 'Esc Esc: draft kept', keys: DRAFT + '<Esc><Esc>' },
      { name: 'Shortcode', keys: 'i:th' },
      { name: 'Line visual', keys: 'iHello<Esc>vhh' }] },
    { id: 'search', name: 'Search', variants: [
      { name: 'Hit under cursor', keys: '/tickets<CR>' },
      { name: 'Typing the query', keys: '/blue folder' }] },
    { id: 'confirm', name: 'Confirm', variants: [
      { name: 'Delete yours', keys: 'jdd' },
      { name: 'Delete 3 of yours', keys: 'kvjjjjjd' },
      { name: 'Quit, list focused', keys: '<Tab>q' }] },
    { id: 'settings', name: 'Profile · editable', variants: [
      { name: 'Account', keys: ':settings<CR>' },
      { name: 'Editing name', keys: ':settings<CR>i' },
      { name: 'Read-only refused', keys: ':settings<CR>jjji' },
      { name: 'Sign in as someone else', keys: ':settings<CR>jjjjj<CR>' },
      { name: 'Sign out', keys: ':settings<CR>jjjjjj<CR>' },
      { name: 'Name saved', keys: ':settings<CR>e<Esc>ccNour<CR>' }] },
    { id: 'profile', name: 'Profile', variants: [
      /* one widget, two subjects. Every frame here is fed through the real handler. */
      { name: 'Profile · you', keys: 'S' },
      { name: 'add account refuses', keys: 'SGkd' },
      { name: 'logout raises the confirmation', keys: 'SGd' },
      { name: 'y signs out: the sign-in field comes back', keys: 'SGdy' },
      { name: 'Esc: the line\'s Normal, card behind', keys: 'S:<Esc>' },
      { name: 'Esc Esc: back on the card', keys: 'S:<Esc><Esc>' },
      { name: 'Esc Esc Esc: the card closes', keys: 'S:<Esc><Esc><Esc>' },
      { name: 'Profile · a person', keys: 'A' },
      { name: 'character selection', keys: 'Allllvlllllll' },
      { name: 'selection of rows', keys: 'Avjj' },
      { name: 'yy: the register fills', keys: 'Ayy' },
      { name: 'ljjjh: the conversation kept its place', keys: 'ljjjh' },
      { name: 'ljjjhl: the card starts at the top', keys: 'ljjjhl' },
      { name: 'no birthday row', keys: '<Tab>jA' },
      { name: 'no username, no birthday', keys: '<Tab>jjA' },
      { name: 'not signed in', start: 'signedout', keys: '' },
      { name: ':signin from the card', start: 'signedout', keys: ':signin<CR>' },
      { name: 'nothing read yet', start: 'reading', keys: '' },
      { name: 'contact: the profile is not read yet', start: 'contactReading', keys: '' },
      { name: 'contact: the read failed', start: 'contactFailed', keys: '' }] },
    { id: 'signin', name: 'Sign in', start: 'signin', variants: [
      { name: 'Phone, pre-filled', keys: '' },
      { name: 'Checking…', keys: '<CR>' },
      { name: 'Login code', keys: '<CR><wait>' },
      { name: 'Wrong code', keys: '<CR><wait>13579<CR><wait>' },
      { name: 'Password', keys: '<CR><wait>42424<CR><wait>' },
      { name: 'Wrong password', keys: '<CR><wait>42424<CR><wait>letmein<CR><wait>' },
      { name: 'Cancel at the code', keys: '<CR><wait><Esc>' },
      { name: 'Tab out', keys: '<CR><wait><Tab>' },
      { name: 'Tab back: the code is gone', keys: '<CR><wait><Tab><Tab>' },
      { name: 'Stored session unregistered', start: 'stale', keys: '' },
      { name: 'Signed out, at rest', start: 'loggedout', keys: '' },
      { name: 'Signed out: Tab to the card', start: 'loggedout', keys: '<Tab>' },
      { name: 'Signing back in: Phone', start: 'loggedout', keys: '<CR>' },
      { name: 'Signing back in: Login code', start: 'loggedout', keys: '<CR><wait>' },
      { name: 'Signing back in: password', start: 'loggedout', keys: '<CR><wait>42424<CR><wait>' },
      { name: 'Signed back in: the chat list', start: 'loggedout', keys: '<CR><wait>42424<CR><wait>hunter2!<CR><wait>' }] },
    { id: 'nocreds', name: 'No credentials', variants: [
      { name: 'No application credentials', start: 'nocreds', keys: '' }] },
    /* Right to left. Every frame is the same conversation reached with the same
       keys, in the two modes, because the mode is the whole of what differs and a
       scene that showed one of them would be showing a program that only exists
       on one kind of terminal.

       `<Tab>` then thirteen `j`s is the walk a reader makes to the last chat in
       the list, which is the same walk every other scene makes to its own chat.
       The mode is chosen by the scene rather than by a key, because in the
       program it is read once from configuration at start-up and never while a
       frame is drawn. */
    { id: 'rtl', name: 'Right to left', variants: [
      { name: 'Terminal mode: the row as stored', bidi: 'terminal', keys: RTL_WALK + 'l' },
      { name: 'Visual mode: the same row, permuted', bidi: 'visual', keys: RTL_WALK + 'l' },
      { name: 'Visual: a search hit inside a right-to-left row', bidi: 'visual', keys: RTL_WALK + 'l/כרטיסים<CR>' },
      { name: 'Visual: a right-to-left draft in the input bar', bidi: 'visual', keys: RTL_WALK + 'l' + RTL_LINE },
      { name: 'Visual: two messages selected', bidi: 'visual', keys: RTL_WALK + 'lvk' }] },
    /* The peer is typing in Ada's chat (the first, so no walk). <typing> is the peer's event and
       <wait> the network tick; the note is on the title, not in the body, and the status line is
       the reader's own. */
    { id: 'typing', name: 'Peer typing', variants: [
      { name: 'The peer starts typing: a dim note on the title', keys: '<typing>' },
      { name: 'Composing a reply: the note stays, the hint is the line\'s', keys: '<typing>' + DRAFT },
      { name: 'Selection and search: the title has no room, the note yields', keys: '<typing>/tickets<CR>vj' },
      { name: 'One tick: still there', keys: '<typing><wait>' },
      { name: 'Second tick, no repeat: gone', keys: '<typing><wait><wait>' },
      { name: 'Repeated before the deadline: renewed', keys: '<typing><wait><typing><wait>' },
      { name: 'The peer\'s message arrives: gone', keys: '<typing><peer>' },
      { name: 'Left for another chat and back: gone', keys: '<typing><Tab>jk<Tab>' }] },
    /* Starting a conversation with someone the reader is not yet chatting with. The same search
       idiom as the conversation's find: a prompt line and a status label, with the results as a
       transient list over the pane. `/` on the chat list, or `:new <query>` from anywhere, opens
       the prompt. Inside a conversation `/` is untouched: it still means message search. */
    { id: 'newchat', name: 'New chat', variants: [
      { name: 'Typing a query', keys: '<Tab>/ad' },
      { name: 'Searching: the query is in flight', keys: '<Tab>/ad<CR>' },
      { name: 'Candidates: j/k walks the list', keys: '<Tab>/ad<CR><wait>' },
      { name: 'No candidates', keys: '<Tab>/zz<CR><wait>' },
      { name: 'Search failed', keys: '<Tab>/!net<CR><wait>' },
      { name: 'Exact username: the new conversation opens', keys: '<Tab>/@torvalds<CR>' },
      { name: 'The person already has a chat: it is focused, not duplicated', keys: '<Tab>/@adalovelace<CR>' }] }
  ];
  function scene(si, vi) {
    const sc = SCENES[si], v = sc.variants[vi], s = fresh(v.start || sc.start, v.bidi);
    return feed(s, v.keys);
  }

  root.TV = { W, H, HINT, ALL_HINTS, HINT_W, fresh, key, feed, render, toHTML, toText, modeName, setMode, scene, SCENES, FOCUS_NAME, chat, peerTyping, peerSays, chars, cells, clusters, baseDir, visualPieces, visualCells, caretCol, bar, BIDI_MODES, answer };
  if (typeof module !== 'undefined') module.exports = root.TV;
})(typeof window !== 'undefined' ? window : globalThis);
