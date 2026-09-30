/* Regenerate the specimen frames in components.html from televim-engine.js.
   Run:  node design-system/build-specimen.js
   Only frame bodies are rewritten, from the engine; the section prose (headings,
   notes) is hand-written and left alone. A montage is a stack of engine panels:
   the chats panel (24 wide) and the conversation/settings/card panel (56 wide),
   cropped to the height the stack needs. */
'use strict';
const fs = require('fs');
const path = require('path');
const TV = require(path.join(__dirname, '..', 'televim-engine.js'));
const FILE = path.join(__dirname, 'components.html');
const W = TV.W, H = TV.H, LW = 24, RW = 56, LABEL = 9, HINT_W = 71;

const blank = () => Array.from({ length: H }, () => Array.from({ length: W }, () => [' ', 't']));
const st = (keys, start) => { const s = TV.fresh(start); TV.feed(s, keys); return s; };
const G = (keys, start) => TV.render(st(keys, start));

/* rows of a panel cropped to height h: top border, h-2 content rows starting at c0, the panel's own bottom border */
function blit(dst, dy, src, x0, w, h, c0) {
  const rows = [0]; for (let i = 0; i <= h - 3; i++) rows.push(c0 + i); rows.push(19);
  rows.forEach((sy, i) => { for (let x = 0; x < w; x++) dst[dy + i][x0 + x] = src[sy][x0 + x].slice(); });
}
/* the conversation row that carries the cursor, so a short crop can still show it */
function selectionRow(src) { for (let y = 1; y <= H - 2; y++) for (let x = LW; x < W - 1; x++) if (src[y][x][1].includes('r')) return y; return 1; }
/* a mini screen: chats panel (from the top) + conversation panel (windowed on its cursor) at height h */
function mini(dst, dy, src, h) { const c0 = Math.max(1, Math.min(selectionRow(src) - 2, 21 - h)); blit(dst, dy, src, 0, LW, h, 1); blit(dst, dy, src, LW, RW, h, c0); }
/* a mini screen that keeps the status row as its last line: panels + bottom border + status */
function miniStatus(dst, dy, src, h) {
  const rows = [0]; for (let i = 1; i <= h - 3; i++) rows.push(i); rows.push(19); rows.push(H - 1);
  rows.forEach((sy, i) => { for (let x = 0; x < W; x++) dst[dy + i][x] = src[sy][x].slice(); });
}
const rowCopy = (dst, dy, src, sy) => { for (let x = 0; x < W; x++) dst[dy][x] = src[sy][x].slice(); };
function put(g, x, y, str, a) { let i = 0; for (const ch of str) { if (x + i >= 0 && x + i < W) g[y][x + i] = [ch, a]; i++; } }

/* ---------- 01 / 07 · Panel: focused · unfocused · empty · noted ---------- */
function sec01() {
  const g = blank();
  [['<Tab>j'], [''], ['', 'signin'], ['/tickets<CR>']].forEach(([k, start], i) => mini(g, i * 6, G(k, start), 6));
  return g;
}
/* ---------- 02 · Message row: normal · search hits · selection · a failed send ---------- */
function sec02() {
  const g = blank();
  ['', '/tickets<CR>', '/tickets<CR>vj', 'jjjj'].forEach((k, i) => mini(g, i * 6, G(k), 6));
  return g;
}
/* ---------- 03 · Mode label, shown on the status row of a real screen ---------- */
function sec03() {
  const g = blank();
  ['', 'iHello', '/tickets<CR>vj', '<Tab>q'].forEach((k, i) => miniStatus(g, i * 6, G(k), 6));
  return g;
}
/* ---------- 04 · Hint row, drawn from TV.HINT ---------- */
const HINT_ORDER = ['normal', 'list', 'visual', 'draft', 'typing', 'complete', 'lnormal', 'lvisual', 'settings', 'cardSelf', 'cardContact', 'cardSignedOut', 'cardReading'];
const HINT_MODE = { normal: 'NORMAL', list: 'NORMAL', visual: 'VISUAL', draft: 'NORMAL', typing: 'INSERT', complete: 'INSERT', lnormal: 'NORMAL', lvisual: 'VISUAL', settings: 'NORMAL', cardSelf: 'NORMAL', cardContact: 'NORMAL', cardSignedOut: 'NORMAL', cardReading: 'NORMAL' };
const LABEL_ATTR = { NORMAL: 'N', INSERT: 'I', VISUAL: 'V', CONFIRM: 'X' };
const ruler = (g, y) => put(g, 0, y, '1234567890'.repeat(8), 'd');
function hintRowFrame() {
  const g = blank(); ruler(g, 0);
  HINT_ORDER.forEach((k, i) => { const y = 2 + i, m = HINT_MODE[k]; put(g, 0, y, ' ' + m.padEnd(LABEL - 1), LABEL_ATTR[m]); put(g, LABEL, y, TV.HINT[k], 'd'); });
  put(g, 0, 16, 'the budget is ' + HINT_W + ' columns, counted with chars().count()', 'd');
  put(g, 0, 17, 'the hint’s leading space is the gap', 'd');
  put(g, 0, 19, 'a hint that does not fit fails the build', 'd');
  return g;
}
function hintMeasureFrame() {
  const g = blank(); ruler(g, 0);
  HINT_ORDER.forEach((k, i) => { const w = TV.chars(TV.HINT[k]); put(g, LABEL, 2 + i, '├' + '─'.repeat(w) + '┤' + '╌'.repeat(HINT_W - w), 'd'); });
  return g;
}
/* ---------- 05 · Prompt: one real input bar per line kind ---------- */
const BAR_STATES = [':wq', '/north door', 'rOn my way, ten minutes.', 'kkkke', 'iSee you at the north door', 'iHi<Esc><Esc>'];
function sec05() {
  const g = blank();
  BAR_STATES.forEach((k, i) => { const src = G(k), top = H - 1 - 3; for (let r = 0; r < 3; r++) rowCopy(g, i * 4 + r, src, top + r); });
  return g;
}
/* ---------- 06 · Status line: the ranks, each the real row of a real state ---------- */
const STATUS_STATES = ['iHello', '<Tab>q', 'kkkkdd', '/tickets<CR>vj', 'iHello<Esc>vhh', '/tickets<CR>', 'jjjj', ''];
function sec06() {
  const g = blank();
  STATUS_STATES.forEach((k, i) => rowCopy(g, i * 3, G(k), H - 1));
  return g;
}

/* ---------- output ---------- */
const frameRows = (grid) => grid.map((r) => '    <div class="row">' + TV.toHTML([r]) + '</div>').join('\n');
function replaceFrame(html, id, grid) {
  const re = new RegExp('(<div class="frame" data-od-id="' + id + '"[^>]*>)[\\s\\S]*?</div>(?=\\s*(?:<p class="cap"|<ol class="notes"|<div class="frame"|</section>|</main>))');
  if (!re.test(html)) throw new Error('no frame ' + id);
  return html.replace(re, (m, open) => open + '\n' + frameRows(grid) + '\n  </div>');
}
function frameBlock(id, caption, keys, aria, si, vi) {
  const s = TV.scene(si, vi);
  return '<div class="frame" data-od-id="' + id + '" data-terminal="dark" role="img" aria-label="' + aria + ', 80 by 24 cells">\n' + frameRows(TV.render(s)) + '\n  </div>\n  <p class="cap"><span class="d">↑</span> ' + caption + ' · keys <b>' + keys + '</b> · mode ' + TV.modeName(s) + '</p>';
}

let html = fs.readFileSync(FILE, 'utf8');
html = replaceFrame(html, 'panel-frame', sec01());
html = replaceFrame(html, 'message-row-frame', sec02());
html = replaceFrame(html, 'mode-label-frame', sec03());
html = replaceFrame(html, 'hint-row-frame', hintRowFrame());
html = replaceFrame(html, 'hint-measure-frame', hintMeasureFrame());
html = replaceFrame(html, 'prompt-frame', sec05());
html = replaceFrame(html, 'status-line-frame', sec06());
html = replaceFrame(html, 'panel-light-frame', sec01());

/* ---------- section 08 · every profile frame, in reading order ---------- */
const CARD_FRAMES = [
  ['profile-self-card-frame', 'self card', 'S', 'profile card, self card', 6, 0],
  ['profile-add-account-frame', 'add account refuses', 'SGkd', 'profile card, add account refuses', 6, 1],
  ['profile-logout-the-confirmation-frame', 'logout: the confirmation', 'SGd', 'profile card, logout: the confirmation', 6, 2],
  ['profile-esc-ladder-frame', 'self card, mid Esc ladder', 'S:<Esc><Esc>', 'profile card, self card mid Esc ladder', 6, 5],
  ['profile-esc-closes-frame', 'Esc Esc Esc: the card closes', 'S:<Esc><Esc><Esc>', 'profile card, Esc Esc Esc closes the card', 6, 6],
  ['profile-contact-card-frame', 'contact card', 'A', 'profile card, contact card', 6, 7],
  ['profile-charwise-selection-frame', 'charwise selection', 'Allllvlllllll', 'profile card, charwise selection', 6, 8],
  ['profile-selection-of-rows-frame', 'selection of rows', 'Avjj', 'profile card, selection of rows', 6, 9],
  ['profile-yy-the-register-fills-frame', 'yy: the register fills', 'Ayy', 'profile card, yy: the register fills', 6, 10],
  ['profile-round-trip-frame', 'l h l: the card starts at the top', 'ljjjhl', 'profile card, l h l round trip', 6, 12],
  ['profile-signed-out-frame', 'not signed in', 'none', 'profile card, not signed in', 6, 15],
  ['profile-signin-frame', ':signin from the card', ':signin<CR>', 'profile card, :signin from the card', 6, 16],
  ['profile-nothing-read-yet-frame', 'nothing read yet', 'none', 'profile card, nothing read yet', 6, 17]
];
const removeBlock = (h, id) => h.replace(new RegExp('\\n[ \\t]*<div class="frame" data-od-id="' + id + '"[^>]*>[\\s\\S]*?</p>'), '');
CARD_FRAMES.forEach(([id]) => { html = removeBlock(html, id); });
const blocks = CARD_FRAMES.map(([id, caption, keys, aria, si, vi]) => frameBlock(id, caption, keys, aria, si, vi)).join('\n  ');
const cue = '<ol class="notes"><li><b>The cue.';
if (!html.includes(cue)) throw new Error('no section-08 notes anchor');
html = html.replace(cue, blocks + '\n  ' + cue);

/* ---------- section 09 · the caret: the four states that matter, one per frame ---------- */
const CARET_FRAMES = [
  ['caret-insert-frame', 'the line composing', 'iHello', 'the line composing, insert caret on a plain ground'],
  ['caret-normal-frame', 'the line’s Normal mode', 'iHello<Esc>', 'the line in its own Normal mode, normal caret on a plain ground'],
  ['caret-inline-frame', 'the card’s inline position', 'Allll', 'the contact card, inline position, normal caret on the selection row'],
  ['caret-row-frame', 'the card, no inline position', 'Sjjjjj', 'the self card on its add account row, the selection row with no caret']
];
const escKeys = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const caretBlock = ([id, caption, keys, aria]) => {
  const s = st(keys);
  return '<div class="frame" data-od-id="' + id + '" data-terminal="dark" role="img" aria-label="' + aria + ', 80 by 24 cells">\n' + frameRows(TV.render(s)) + '\n  </div>\n  <p class="cap"><span class="d">↑</span> ' + caption + ' · keys <b>' + escKeys(keys) + '</b> · mode ' + TV.modeName(s) + '</p>';
};
CARET_FRAMES.forEach(([id]) => { html = removeBlock(html, id); });
const caretAnchor = '<ol class="notes" data-od-id="caret-notes">';
if (!html.includes(caretAnchor)) throw new Error('no section-09 notes anchor');
html = html.replace(caretAnchor, CARET_FRAMES.map(caretBlock).join('\n  ') + '\n  ' + caretAnchor);

fs.writeFileSync(FILE, html);
console.log('regenerated every frame in components.html from the engine');
