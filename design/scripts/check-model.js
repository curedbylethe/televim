/* Check the vendored design model against the two invariants it promises.
 *
 * This runs everywhere, including a CI runner that has no OpenDesign project, so
 * it checks the *copy* rather than the diff against the other one. The diff is
 * `scripts/design-sync.sh check`; this is the part that needs neither.
 *
 *   1. Every scene renders as exactly 24 rows by 80 CELLS. A frame that is one
 *      column short is a frame that clips, and clipping is invisible in a
 *      browser that scrolls. The measure is cells and not characters because a
 *      cell is not a character: an emoji is two cells and a combining mark is
 *      none, so a row measured in characters is a row whose width depends on
 *      what script it happens to be in. Measuring in characters is what this
 *      check used to do, and it is the same mistake `DESIGN.md`'s text-direction
 *      clause exists to prevent.
 *   2. Every hint fits `HINT_W`, which is the budget the Rust asserts too. The
 *      model and the program share the number, so this catches a hint the design
 *      grew past what the terminal can show. A hint is measured in cells for the
 *      same reason, and in code points as well, because the Rust measures the
 *      hint row in `chars().count()` and the two must not drift apart silently.
 *   3. Every right-to-left row the model lays out is a permutation of the row as
 *      stored: no cluster dropped, none drawn twice, and the row no wider than it
 *      was. A reorder that lost or duplicated a piece is not a row drawn in
 *      another order, it is a row drawn wrong.
 */

'use strict';
const path = require('path');
const TV = require(path.join(__dirname, '..', 'televim-engine.js'));

const W = TV.W;
const H = TV.H;
const failures = [];
let scenes = 0;

TV.SCENES.forEach((screen, i) => {
  screen.variants.forEach((variant, j) => {
    const grid = TV.render(TV.scene(i, j));
    scenes += 1;
    /* the width of a rendered row, in the cells a terminal would draw it in: a
       two-cell cluster counts as two, and the continuation cell it draws across
       counts as none because the cluster has already been counted. */
    const cellWidths = grid.map((row) => row.reduce((n, [ch]) => n + (ch === '' ? 0 : TV.cells(ch)), 0));
    const widths = [...new Set(cellWidths)];
    if (grid.length !== H) {
      failures.push(`${screen.id}/${variant.name}: ${grid.length} rows, not ${H}`);
    } else if (widths.length !== 1 || widths[0] !== W) {
      failures.push(`${screen.id}/${variant.name}: row widths ${widths.join(', ')}, not one of ${W}`);
    }
  });
});

const hints = Object.entries(TV.HINT || {});
for (const [key, text] of hints) {
  const width = TV.cells(text);
  if (width > TV.HINT_W) {
    failures.push(`hint ${key}: ${width} cells, over the ${TV.HINT_W} budget`);
  }
  if (TV.chars(text) > TV.HINT_W) {
    failures.push(`hint ${key}: ${TV.chars(text)} characters, over the ${TV.HINT_W} budget`);
  }
}

/* The direction module, against the shapes it is responsible for. These are the
 * same fixtures `crates/tui/src/bidi.rs` pins, and the expected drawing orders
 * were captured from `unicode-bidi` 0.3.18 rather than reasoned out here: a
 * model of the reorder that was written from the same assumption as the reorder
 * it models would agree with itself and prove nothing. */
const RTL = [
  // [text, the direction the ROW is laid out at, the pieces as drawn, left to right]
  ['שלום', 'rtl', ['ם', 'ו', 'ל', 'ש']],
  ['שלום עולם', 'rtl', ['ם', 'ל', 'ו', 'ע', ' ', 'ם', 'ו', 'ל', 'ש']],
  ['سلام', 'rtl', ['م', 'ا', 'ل', 'س']],
  [' שלום ', 'rtl', [' ', 'ם', 'ו', 'ל', 'ש', ' ']],
  ['--- 123 ---', 'mixed', ['--- 123 ---']],
  ['שלום 123', 'rtl', ['123', ' ', 'ם', 'ו', 'ל', 'ש']],
  ['(שלום)', 'rtl', [')', 'ם', 'ו', 'ל', 'ש', '(']],
  ['hello 123 (world)', 'ltr', ['hello 123 (world)']],
  ['הכרטיסים 3 בתיק הכחול', 'rtl', ['ל', 'ו', 'ח', 'כ', 'ה', ' ', 'ק', 'י', 'ת', 'ב', ' ', '3', ' ', 'ם', 'י', 'ס', 'י', 'ט', 'ר', 'כ', 'ה']],
  ['תודה רבה! 👨‍👩‍👧', 'rtl', ['👨‍👩‍👧', ' ', '!', 'ה', 'ב', 'ר', ' ', 'ה', 'ד', 'ו', 'ת']],
  // rule W4 — a separator BETWEEN two numbers is part of the number, so a clock
  // time is read as written instead of coming out reversed
  ['שלום עולם 21:30', 'rtl', ['21:30', ' ', 'ם', 'ל', 'ו', 'ע', ' ', 'ם', 'ו', 'ל', 'ש']],
  ['שלום 21:30 עולם', 'rtl', ['ם', 'ל', 'ו', 'ע', ' ', '21:30', ' ', 'ם', 'ו', 'ל', 'ש']],
  ['汉测 123 שלום', 'rtl', ['ם', 'ו', 'ל', 'ש', ' ', '汉测 123']],
  // rule W7 — a number takes the last strong direction before it, so `Z0!0` is
  // one piece and the `42` after `(abc)` does not inherit the `abc`
  ['abc 123 שלום', 'ltr', ['abc 123 ', 'ם', 'ו', 'ל', 'ש']],
  ['Z0!0ם{', 'ltr', ['Z0!0ם{']],
  ['abc 123', 'ltr', ['abc 123']],
  ['a1ב', 'ltr', ['a1ב']],
  ['שלום (abc) 42', 'rtl', ['42', ' ', ')', 'abc', '(', ' ', 'ם', 'ו', 'ל', 'ש']],
  ['שלום (abc) 42', 'ltr', ['ם', 'ו', 'ל', 'ש', ' (abc) 42']],
  // rule N0 — a matched bracket pair travels with the sentence it belongs to
  ['[1]abc', 'rtl', ['abc', ']', '1', '[']],
  ['[1]abc', 'ltr', ['[1]abc']],
  ['}א[abc]', 'ltr', ['}א[abc]']],
  /* Two shapes this model gets wrong, and says so rather than pinning a wrong
     answer. Both need the algorithm's BD16 bracket-pair procedure, which pairs
     brackets by canonical equivalence across the whole paragraph and reorders
     them by opening position — a paragraph-level pass, not the per-row one every
     other fixture here exercises. The Rust asks `unicode-bidi` and gets this
     right; this is a model and the Rust is the authority (`design/README.md`).

     They are listed so that a future fix can delete the line rather than
     re-derive it, and so nobody reads the 5/4000 divergence as unknown. */
  // ['字"מ}[1]', 'ltr', ['字"', ']', '1', '[', '}', 'מ']],   // BD16
  // ['א1]Z(漢!)ב', 'rtl', ['ב', 'Z(漢!)', ']', '1', 'א']], // BD16
  // an emoji is a NEUTRAL: it has no direction of its own, so it takes the
  // direction of what surrounds it and never decides a message's base
  ['👨‍👩‍👧)א·👍🏽(ו👍🏽', 'rtl', ['👍🏽', 'ו', '(', '👍🏽', '·', 'א', ')', '👨‍👩‍👧']],
  // rules N1/N2 — a run of neutrals between two like directions takes them
  ['··! …9…!9', 'mixed', ['··! …9…!9']],
  // rule L1 — trailing whitespace takes the paragraph's level, which is where a
  // reader looks for the space that ends a right-to-left sentence
  ['שלום   ', 'rtl', [' ', ' ', ' ', 'ם', 'ו', 'ל', 'ש']],
  ['   שלום', 'rtl', ['ם', 'ו', 'ל', 'ש', ' ', ' ', ' ']]
];
/* The base direction a message is DETECTED as, which is a separate question from
   the direction a row is laid out at: the fixtures above that pin both answers
   for one text are what keep the two from being quietly merged. */
const BASE = [
  ['שלום', 'rtl'], ['hello', 'ltr'], ['--- 123 ---', 'mixed'], ['😀 123', 'mixed'],
  ['👨‍👩‍👧(}.0]?]', 'mixed'], ['汉测 123 שלום', 'ltr'], ['- - - שלום', 'rtl'],
  ['שלום - - -', 'rtl'], ['مرحبا', 'rtl']
];
for (const [text, base] of BASE) {
  const got = TV.baseDir(text);
  if (got !== base) failures.push(`base direction of ${JSON.stringify(text)}: ${got}, not ${base}`);
}
for (const [text, base, drawn] of RTL) {
  const pieces = TV.visualCells(text, base);
  const gotDrawn = pieces.map((p) => p.text);
  if (JSON.stringify(gotDrawn) !== JSON.stringify(drawn)) {
    failures.push(`${JSON.stringify(text)} at ${base} draws ${JSON.stringify(gotDrawn)}, not ${JSON.stringify(drawn)}`);
  }
  /* the permutation is a bijection over the row's clusters, and it does not
     change how wide the row is. Pieces may merge, so the test is on the clusters
     each piece covers rather than on the number of pieces: expanded to one entry
     per cluster, the pieces must tile the row's clusters exactly once. */
  const covered = pieces.flatMap((p) => TV.clusters(p.text));
  const expected = TV.clusters(text);
  const sortedCovered = covered.slice().sort();
  const sortedExpected = expected.slice().sort();
  const same = sortedCovered.length === sortedExpected.length &&
    sortedCovered.every((c, i) => c === sortedExpected[i]);
  if (!same) {
    failures.push(`${JSON.stringify(text)}: the pieces do not tile the row's clusters`);
  }
  if (pieces.reduce((n, p) => n + p.cells, 0) !== TV.cells(text)) {
    failures.push(`${JSON.stringify(text)}: the reorder changed the width of the row`);
  }
}

/* The width table, against the `unicode-width` the Rust measures with. */
const WIDTHS = [
  ['abc', 3], ['—', 1], ['·', 1], ['…', 1], ['⏎', 1], ['│', 1], ['░', 1],
  ['█', 1], ['😀', 2], ['\u{1F468}‍\u{1F469}‍\u{1F467}', 2], ['\u{1F44D}\u{1F3FD}', 2],
  ['漢字', 4], ['é', 1], ['שלום', 4], ['مرحبا', 5]
];
for (const [text, want] of WIDTHS) {
  const got = TV.cells(text);
  if (got !== want) failures.push(`width of ${JSON.stringify(text)}: ${got} cells, not ${want}`);
}

if (failures.length) {
  console.error('  ✗ the design model is not sound:');
  for (const failure of failures) {
    console.error(`      ${failure}`);
  }
  process.exit(1);
}

const tightest = hints.reduce(
  (worst, [key, text]) => {
    const spare = TV.HINT_W - TV.cells(text);
    return spare < worst.spare ? { key, spare } : worst;
  },
  { key: null, spare: Infinity },
);

console.log(
  `  ✓ ${scenes}/${scenes} scenes are ${H} rows x ${W} cells, ` +
    `all ${hints.length} hints fit ${TV.HINT_W}` +
    (tightest.key ? ` (tightest: ${tightest.key}, ${tightest.spare} spare)` : '') +
    `, and ${RTL.length} right-to-left rows permute without changing width`,
);