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
const STATUS_STATES = ['iHello', '<Tab>q', 'kkkkdd', '/tickets<CR>vj', 'iHello<Esc>vhh', '/tickets<CR>'];
/* the rest of the ranks, second frame: a jump in flight, a failed send, a flash, a cached window, the hint.
   Eleven rows at the two-row spacing do not fit in 24, so the ranks are split across two frames. */
const STATUS_STATES_REST = ['jjgd', 'jjjj', 'gd', '<cached>', '', ['gg', 'unreadFar']];
function statusFrame(states) {
  const g = blank();
  states.forEach((k, i) => {
    const [keys, start] = Array.isArray(k) ? k : [k, undefined];
    rowCopy(g, i * 3, G(keys, start), H - 1);
  });
  return g;
}
const sec06 = () => statusFrame(STATUS_STATES);
const sec06b = () => statusFrame(STATUS_STATES_REST);

/* ---------- output ---------- */
const frameRows = (grid) => grid.map((r) => '    <div class="row">' + TV.toHTML([r]) + '</div>').join('\n');
function replaceFrame(html, id, grid) {
  const re = new RegExp('(<div class="frame" data-od-id="' + id + '"[^>]*>)[\\s\\S]*?</div>(?=\\s*(?:<p class="cap"|<ol class="notes"|<div class="frame"|</section>|</main>))');
  if (!re.test(html)) throw new Error('no frame ' + id);
  return html.replace(re, (m, open) => open + '\n' + frameRows(grid) + '\n  </div>');
}
/* A key string in a caption is text, not markup: `i:th<CR>` written raw is an
   unknown tag the browser swallows, and the caption then reads as `i:th`. */
const escKeys = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
function frameBlock(id, caption, keys, aria, si, vi) {
  const s = TV.scene(si, vi);
  return '<div class="frame" data-od-id="' + id + '" data-terminal="dark" role="img" aria-label="' + aria + ', 80 by 24 cells">\n' + frameRows(TV.render(s)) + '\n  </div>\n  <p class="cap"><span class="d">↑</span> ' + caption + ' · keys <b>' + escKeys(keys) + '</b> · mode ' + TV.modeName(s) + '</p>';
}

let html = fs.readFileSync(FILE, 'utf8');
html = replaceFrame(html, 'panel-frame', sec01());
html = replaceFrame(html, 'message-row-frame', sec02());
html = replaceFrame(html, 'mode-label-frame', sec03());
html = replaceFrame(html, 'hint-row-frame', hintRowFrame());
html = replaceFrame(html, 'hint-measure-frame', hintMeasureFrame());
html = replaceFrame(html, 'prompt-frame', sec05());
html = replaceFrame(html, 'status-line-frame', sec06());
html = replaceFrame(html, 'status-line-rest-frame', sec06b());
html = replaceFrame(html, 'panel-light-frame', sec01());

/* ---------- section 08 · every profile frame, in reading order ----------
   Scenes are named, not numbered. A frame list written as `6, 0` is a claim
   about the order of two arrays that nothing enforces, and it goes stale the
   moment a scene is inserted before the one it points at — which is exactly what
   had happened, and the failure was a `TypeError` rather than a wrong frame. */
const sceneIndex = (id) => {
  const i = TV.SCENES.findIndex((s) => s.id === id);
  if (i < 0) throw new Error('no scene ' + id);
  return i;
};
const variantIndex = (id, name) => {
  const si = sceneIndex(id);
  const vi = TV.SCENES[si].variants.findIndex((v) => v.name === name);
  if (vi < 0) throw new Error(`scene ${id} has no variant ${name}`);
  return [si, vi];
};
const CARD_FRAMES = [
  ['profile-self-card-frame', 'self card', 'S', 'profile card, self card', 'Profile · you'],
  ['profile-accounts-frame', 'accounts: every account on this machine, the active one marked', 'S', 'profile card, two accounts on this machine, the active one marked', 'accounts: every account on this machine, the active one marked'],
  ['profile-account-active-frame', 'd on the active account: it says where you are', 'Sjjjjjjd', 'profile card, d on the active account, the sentence on the status row', 'd on the active account: it says where you are'],
  ['profile-account-switch-frame', 'd on another account: the switch lands where the reader was', 'Sjjjjjjjd', 'conversation, a switch landed on the same chat under the other account, its own messages', 'd on another account: the switch lands where the reader was'],
  ['profile-add-account-frame', 'add account: the sign-in that follows', 'SGkd', 'profile card, add account: the sign-in that follows', 'add account: the sign-in that follows'],
  ['profile-logout-the-confirmation-frame', 'logout: the confirmation', 'SGd', 'profile card, logout: the confirmation', 'logout raises the confirmation'],
  ['profile-logged-out-frame', 'logout: signed out', 'SGdy', 'profile card, logout: signed out', 'y signs out: the sign-in field comes back'],
  ['profile-esc-ladder-frame', 'self card, mid Esc ladder', 'S:<Esc><Esc>', 'profile card, self card mid Esc ladder', "Esc Esc: back on the card"],
  ['profile-esc-closes-frame', 'Esc Esc Esc: the card closes', 'S:<Esc><Esc><Esc>', 'profile card, Esc Esc Esc closes the card', 'Esc Esc Esc: the card closes'],
  ['profile-contact-card-frame', 'contact card', 'A', 'profile card, contact card', 'Profile · a person'],
  ['profile-charwise-selection-frame', 'charwise selection', 'Allllvlllllll', 'profile card, charwise selection', 'character selection'],
  ['profile-selection-of-rows-frame', 'selection of rows', 'Avjj', 'profile card, selection of rows', 'selection of rows'],
  ['profile-yy-the-register-fills-frame', 'yy: the register fills', 'Ayy', 'profile card, yy: the register fills', 'yy: the register fills'],
  ['profile-round-trip-frame', 'l h l: the card starts at the top', 'ljjjhl', 'profile card, l h l round trip', 'ljjjhl: the card starts at the top'],
  ['profile-signed-out-frame', 'not signed in', 'none', 'profile card, not signed in', 'not signed in'],
  ['profile-signin-frame', ':signin from the card', ':signin<CR>', 'profile card, :signin from the card', ':signin from the card'],
  ['profile-nothing-read-yet-frame', 'nothing read yet', 'none', 'profile card, nothing read yet', 'nothing read yet'],
  ['profile-contact-reading-frame', 'contact: the profile is not read yet', 'none', 'profile card, contact profile not read yet', 'contact: the profile is not read yet'],
  ['profile-contact-failed-frame', 'contact: the read failed', 'none', 'profile card, contact read failed', 'contact: the read failed']
].map(([id, caption, keys, aria, variant]) => [id, caption, keys, aria, ...variantIndex('profile', variant)]);
const removeBlock = (h, id) => h.replace(new RegExp('\\n[ \\t]*<div class="frame" data-od-id="' + id + '"[^>]*>[\\s\\S]*?</p>'), '');
CARD_FRAMES.forEach(([id]) => { html = removeBlock(html, id); });
const blocks = CARD_FRAMES.map(([id, caption, keys, aria, si, vi]) => frameBlock(id, caption, keys, aria, si, vi)).join('\n  ');
const cue = '<ol class="notes"><li><b>The cue.';
if (!html.includes(cue)) throw new Error('no section-08 notes anchor');
html = html.replace(cue, blocks + '\n  ' + cue);

/* ---------- section 01 · the peer's typing note, on the conversation's title ---------- */
const TYPING_FRAMES = [
  ['panel-typing-frame', 'the peer is typing', '<typing>', 'conversation panel, the peer is typing', 'The peer starts typing: a dim note on the title'],
  ['panel-typing-composing-frame', 'composing a reply, the note stays', '<typing> + a draft', 'conversation panel, composing a reply while the peer types', 'Composing a reply: the note stays, the hint is the line\'s'],
  ['panel-typing-yields-frame', 'selection and search: the note yields', '<typing>/tickets<CR>vj', 'conversation panel, the typing note yields to a selection and a search', 'Selection and search: the title has no room, the note yields']
].map(([id, caption, keys, aria, variant]) => [id, caption, keys, aria, ...variantIndex('typing', variant)]);
TYPING_FRAMES.forEach(([id]) => { html = removeBlock(html, id); });
const typingAnchor = '<ol class="notes"><li><b>Four real screens, stacked.';
if (!html.includes(typingAnchor)) throw new Error('no section-01 notes anchor');
html = html.replace(typingAnchor, TYPING_FRAMES.map(([id, caption, keys, aria, si, vi]) => frameBlock(id, caption, keys.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;'), aria, si, vi)).join('\n  ') + '\n  ' + typingAnchor);

/* ---------- section 09 · the caret: the four states that matter, one per frame ---------- */
const CARET_FRAMES = [
  ['caret-insert-frame', 'the line composing', 'iHello', 'the line composing, insert caret on a plain ground'],
  ['caret-normal-frame', 'the line’s Normal mode', 'iHello<Esc>', 'the line in its own Normal mode, normal caret on a plain ground'],
  ['caret-inline-frame', 'the card’s inline position', 'Allll', 'the contact card, inline position, normal caret on the selection row'],
  ['caret-row-frame', 'the card, no inline position', 'Sjjjjj', 'the self card on its add account row, the selection row with no caret']
];
const caretBlock = ([id, caption, keys, aria]) => {
  const s = st(keys);
  return '<div class="frame" data-od-id="' + id + '" data-terminal="dark" role="img" aria-label="' + aria + ', 80 by 24 cells">\n' + frameRows(TV.render(s)) + '\n  </div>\n  <p class="cap"><span class="d">↑</span> ' + caption + ' · keys <b>' + escKeys(keys) + '</b> · mode ' + TV.modeName(s) + '</p>';
};
CARET_FRAMES.forEach(([id]) => { html = removeBlock(html, id); });
const caretAnchor = '<ol class="notes" data-od-id="caret-notes">';
if (!html.includes(caretAnchor)) throw new Error('no section-09 notes anchor');
html = html.replace(caretAnchor, CARET_FRAMES.map(caretBlock).join('\n  ') + '\n  ' + caretAnchor);

/* ---------- section 10 · text direction: the mode, then the bar ----------
   Its own section rather than a seventh bar in section 05, because direction is
   not a line kind: section 05's frame and prose are organised by kind, and its
   states are reached through a helper that takes no direction at all. Direction
   has no other home either — 02 lays out a message row and 09 names the caret's
   two shapes, and neither is about the order characters are handed over in.

   The engine already models it this way, as a whole `rtl` scene group whose every
   variant is the same thing in a different mode, so the frames come from that
   group by name through `frameBlock` — the same helper section 08 uses. The keys
   shown are the variant's own `keys`, read back off the scene rather than written
   out again here, so the caption cannot drift from the keys that drew the frame. */
const BIDI_FRAMES = [
  ['bidi-terminal-frame', 'the row as stored', 'right-to-left chat, terminal mode, the row as it is stored', 'Terminal mode: the row as stored'],
  ['bidi-visual-frame', 'the same row, permuted', 'right-to-left chat, visual mode, the same row permuted', 'Visual mode: the same row, permuted'],
  ['bidi-bar-frame', 'a right-to-left draft in the input bar', 'right-to-left chat, visual mode, a right-to-left draft permuted in the bar, the caret on its visual cell', 'Visual: a right-to-left draft in the input bar']
].map(([id, caption, aria, variant]) => {
  const [si, vi] = variantIndex('rtl', variant);
  return [id, caption, TV.SCENES[si].variants[vi].keys, aria, si, vi];
});
BIDI_FRAMES.forEach(([id]) => { html = removeBlock(html, id); });
const bidiAnchor = '<ol class="notes" data-od-id="bidi-notes">';
if (!html.includes(bidiAnchor)) throw new Error('no section-10 notes anchor');
html = html.replace(bidiAnchor, BIDI_FRAMES.map((f) => frameBlock(...f)).join('\n  ') + '\n  ' + bidiAnchor);

/* ---------- sections 11 and 12 · the session passphrase: one full screen per state ----------
   The prose and the captions are hand-written in components.html; these are the frame bodies only,
   each the whole 24-row screen of a named scene variant. */
const PASS_SCREENS = [
  ['passphrase-ask-frame', 'passphrase', 0], ['passphrase-empty-frame', 'passphrase', 1],
  ['passphrase-typing-frame', 'passphrase', 2], ['passphrase-checking-frame', 'passphrase', 3],
  ['passphrase-refused-frame', 'passphrase', 4], ['passphrase-unlocked-frame', 'passphrase', 5],
  ['passphrase-esc-frame', 'passphrase', 6], ['passphrase-esc-unlock-frame', 'passphrase', 7],
  ['change-confirm-frame', 'passchange', 0], ['change-entry-frame', 'passchange', 1],
  ['change-repeat-frame', 'passchange', 2], ['change-mismatch-frame', 'passchange', 3],
  ['change-success-frame', 'passchange', 4], ['change-failure-frame', 'passchange', 5],
  ['change-locked-frame', 'passchange', 6]
];
PASS_SCREENS.forEach(([id, scene, vi]) => { html = replaceFrame(html, id, TV.render(TV.scene(sceneIndex(scene), vi))); });

fs.writeFileSync(FILE, html);
console.log('regenerated every frame in components.html from the engine');
