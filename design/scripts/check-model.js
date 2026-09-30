/* Check the vendored design model against the two invariants it promises.
 *
 * This runs everywhere, including a CI runner that has no OpenDesign project, so
 * it checks the *copy* rather than the diff against the other one. The diff is
 * `scripts/design-sync.sh check`; this is the part that needs neither.
 *
 *   1. Every scene renders as exactly 24 rows by 80 columns. A frame that is one
 *      column short is a frame that clips, and clipping is invisible in a browser
 *      that scrolls.
 *   2. Every hint fits `HINT_W`, which is the budget the Rust asserts too. The
 *      model and the program share the number, so this catches a hint the design
 *      grew past what the terminal can show.
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
    const rows = TV.toText(TV.render(TV.scene(i, j)));
    scenes += 1;
    const widths = [...new Set(rows.map((row) => [...row].length))];
    if (rows.length !== H) {
      failures.push(`${screen.id}/${variant.name}: ${rows.length} rows, not ${H}`);
    } else if (widths.length !== 1 || widths[0] !== W) {
      failures.push(
        `${screen.id}/${variant.name}: row widths ${widths.join(', ')}, not one of ${W}`,
      );
    }
  });
});

const hints = Object.entries(TV.HINT || {});
for (const [key, text] of hints) {
  const width = TV.chars(text);
  if (width > TV.HINT_W) {
    failures.push(`hint ${key}: ${width} columns, over the ${TV.HINT_W} budget`);
  }
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
    const spare = TV.HINT_W - TV.chars(text);
    return spare < worst.spare ? { key, spare } : worst;
  },
  { key: null, spare: Infinity },
);

console.log(
  `  ✓ ${scenes}/${scenes} scenes are ${H} rows x ${W} columns, ` +
    `and all ${hints.length} hints fit ${TV.HINT_W}` +
    (tightest.key ? ` (tightest: ${tightest.key}, ${tightest.spare} spare)` : ''),
);
