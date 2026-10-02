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
    list: ' j/k: chat  Enter: open  Tab: pane  h: conversation  A:card  S:you',
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
    'Frances Allen': { username: 'fallen', bio: 'Optimise the loop, not the line.' }
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
    const C = (name, unread, msgs, extra) => Object.assign({ name, unread, msgs, cur: Math.max(0, msgs.length - 1), top: 0 }, extra || {});
    const t = (text, o) => Object.assign({ from: 'them', text }, o || {});
    const y = (text, o) => Object.assign({ from: 'you', text }, o || {});
    return [
      C('Ada Lovelace', 0, [
        t('Are you coming tonight?'),
        y('Running late. Save me a seat.'),
        t('The one by the window.', { reply: { quote: 'Save me a seat.' } }),
        t('Bring the tickets, the tickets are in the blue folder.'),
        y('Which tickets? I have the train ones and the concert ones.'),
        t('The concert tickets. The train ones can stay on the fridge.'),
        y('Tickets are under the lamp. Folder found.'),
        t('Sorry, I missed your earlier message.', { reply: { unloaded: true } }),
        t('Doors open at eight, and the side gate is shut after nine, so do not be later.'),
        y('Did that go through?', { status: 'failed: no route' }),
        y('On my way, ten minutes.', { status: 'sending…' }),
        t('I will keep your seat.'),
        t('The side gate locks at nine. If you miss it, ring twice and wait by the lamp.'),
        y('Twice. I have both tickets in the blue folder, concert on top.'),
        t('Leave the train ones on the fridge. I only need the concert pair tonight.'),
        y('Understood. Saving the seat was the whole of the favour.')
      ], { cur: 5, older: true }),
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
      C('Frances Allen', 0, [t('Optimise the loop, not the line.')])
    ];
  }

  function fresh(view) {
    const s = {
      view: 'chat', right: 'conv', focus: 'conv', chat: 0, chats: makeChats(),
      vis: null, search: null, confirm: null, flash: '', pend: '', line: null, draft: null, reg: '',
      sset: 0, signin: null, card: null, count: 0,
      profile: { name: 'Noor Haddad', username: 'noorh', bio: 'Night shift. Log first, news later.', phone: '+44 7700 900142', birthday: 'Oct 19, 2001 (24 years old)' }
    };
    if (view === 'signin') beginSignin(s);
    else if (view === 'stale') beginStale(s);
    else if (view === 'nocreds') beginNoCreds(s);
    else if (view === 'loggedout') beginLoggedOut(s);
    else if (view === 'signedout' || view === 'reading') beginShell(s, view);
    return s;
  }

  /* ---------- helpers ---------- */
  const chat = (s) => s.chats[s.chat];
  const ctx = (s) => (s.right === 'settings' ? 'settings' : 'chat:' + s.chat);
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  function refuse(s, t) { s.search = null; s.flash = t; }
  function range(s) { const c = chat(s); return [Math.min(s.vis.anchor, c.cur), Math.max(s.vis.anchor, c.cur)]; }
  function hitsOf(s) { return s.search ? s.search.hits : []; }

  function wrap(str, w0, w, keep) {
    const rows = [], parts = str.split('\n'); let off = 0;
    for (let pi = 0; pi < parts.length; pi++) {
      const p = parts[pi], last = pi === parts.length - 1; let st = 0;
      for (;;) {
        const width = rows.length === 0 ? w0 : w;
        if (p.length - st <= width) { rows.push({ s: off + st, e: off + p.length, nl: !last }); break; }
        if (keep) {
          const b = p.lastIndexOf(' ', st + width - 1);
          const end = b > st ? b + 1 : st + width;
          rows.push({ s: off + st, e: off + end, nl: false }); st = end;
        } else {
          const b = p.lastIndexOf(' ', st + width);
          if (b > st) { rows.push({ s: off + st, e: off + b, nl: false }); st = b + 1; }
          else { rows.push({ s: off + st, e: off + st + width, nl: false }); st += width; }
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
    if (s.view === 'quit') { const n = fresh(); Object.keys(s).forEach((x) => delete s[x]); Object.assign(s, n); return s; }
    if (s.confirm) { s.flash = ''; confirmKey(s, k); return s; }
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
      case '/': s.focus = 'conv'; convKey(s, k, ''); break;
      case 'A': openCard(s, 'person'); break;
      case 'S': openCard(s, 'self'); break;
    }
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
      case 'command': {
        const c = buf.trim().replace(/^:/, ''); closeLine(s);
        if (c === 'settings') openSettings(s);
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
    s.confirm = null; s.line = null; s.vis = null; s.search = null; s.pend = ''; s.flash = '';
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
  const FOCUS_NAME = { list: 'chat list', conv: 'conversation', settings: 'editable profile', profile: 'profile card', input: 'input bar', nocreds: 'shell' };

  /* ---------- the grid ---------- */
  const newGrid = () => Array.from({ length: H }, () => Array.from({ length: W }, () => [' ', 't']));
  function put(g, x, y, str, a) {
    let i = 0;
    for (const ch of str) { if (x + i >= 0 && x + i < W && y >= 0 && y < H) g[y][x + i] = [ch, a]; i++; }
    return i;
  }
  function fill(g, x, y, w, a) { for (let i = 0; i < w; i++) put(g, x + i, y, ' ', a); }
  function box(g, x, y, w, h, title, lit) {
    const a = lit ? 'f' : 'b';
    put(g, x, y, '┌─', a); put(g, x + 2, y, ' ' + title + ' ', 't');
    const used = 2 + chars(title) + 2;
    put(g, x + used, y, '─'.repeat(w - used - 1), a); put(g, x + w - 1, y, '┐', a);
    for (let r = 1; r < h - 1; r++) { put(g, x, y + r, '│', a); put(g, x + w - 1, y + r, '│', a); }
    put(g, x, y + h - 1, '└' + '─'.repeat(w - 2) + '┘', a);
  }
  const trunc = (t, n) => (chars(t) > n ? Array.from(t).slice(0, n - 1).join('') + '…' : t);

  /* ---------- the bar ---------- */
  function kindTitle(s, L) {
    if (L.kind === 'message') { const c = chat(s); return 'Message to ' + (c ? c.name : ''); }
    return { reply: 'Reply', edit: 'Edit', command: 'Command', find: 'Find', name: 'Name', username: 'Username', bio: 'Bio', phone: 'Phone', code: 'Login code', pw: 'Password' }[L.kind];
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
      return { title: kindTitle(s, { kind: p.kind }), shown, rows, cr, n, start, lit: false, prefix: '', conceal };
    }
    let title = 'Input', buf = '', pos = 0, lit = false, prefix = '', conceal = false;
    if (L) { lit = true; title = kindTitle(s, L); buf = L.buf; pos = L.pos; prefix = L.kind === 'command' ? ': ' : L.kind === 'find' ? '/ ' : ''; conceal = L.kind === 'pw'; }
    else if (d && d.ctx === ctx(s) && s.view === 'chat') { title = 'draft'; buf = d.buf; pos = d.pos; }
    const shown = conceal ? '•'.repeat(buf.length) : buf;
    const rows = wrap(shown, prefix ? 74 : 76, 76, true);
    const cr = caretRC(rows, pos);
    const n = Math.min(BAR_MAX, Math.max(1, rows.length));
    const start = rows.length > n ? clamp(cr[0] - n + 1, 0, rows.length - n) : 0;
    return { title, shown, rows, cr, n, start, lit, prefix, conceal };
  }
  function drawBar(g, s, b, y0) {
    const L = s.line;
    box(g, 0, y0, W, b.n + 2, b.title, b.lit);
    let lo = -1, hi = -2;
    if (L && L.mode === 'visual') { lo = Math.min(L.anchor, L.pos); hi = Math.max(L.anchor, L.pos); }
    for (let i = 0; i < b.n; i++) {
      const ri = b.start + i, r = b.rows[ri], y = y0 + 1 + i; if (!r) continue;
      let x = 2;
      if (ri === 0 && b.prefix) { put(g, 2, y, b.prefix, 't'); x = 4; }
      for (let j = r.s; j < r.e; j++) {
        const ch = b.shown[j];
        let ce = ch, a = b.lit ? 't' : 'd';
        if (b.lit && !b.conceal && ch === ' ') { ce = '·'; a = 'd'; }
        if (j >= lo && j <= hi) a += 's';
        put(g, x + (j - r.s), y, ce, a);
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
    if (s.search) {
      const c = chat(s), h = s.search.hits, at = h.indexOf(c.cur), q = '/' + s.search.q;
      return { t: ' ' + (at >= 0 ? q + ' — match ' + (at + 1) + ' of ' + h.length : q + ' — ' + h.length + ' loaded'), a: 't' };
    }
    if (s.focus === 'settings') return { t: HINT.settings, a: 'd' };
    if (s.focus === 'profile') {
      const C = s.card;
      if (C.vis) return { t: ' ' + cardSel(s) + (C.vis.mode === 'rows' ? ' row(s)' : ' character(s)') + ' selected — Esc clears', a: 't' };
      const h = C.sub === 'self' ? HINT.cardSelf : C.sub === 'person' ? HINT.cardContact : C.sub === 'reading' ? HINT.cardReading : HINT.cardSignedOut;
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
      /* the sender label is the group's, so only its first row carries it */
      wrap(full, CW, CW, false).forEach((r, i) => rows.push({ mi, first: i === 0, lab: i === 0 && head, full, qlen: qp.length, mask, s: r.s, e: r.e, from: m.from }));
      /* the last row of a message carries its own status; the last row of a group also carries
         the peer's read state (outgoing only, never beside a status) and the group's time */
      const rcpt = tail && m.from === 'you' && !m.status && m.rcpt ? m.rcpt : '';
      const suf = m.status ? '[' + m.status + ']' : rcpt ? '[' + rcpt + ']' : '';
      const tm = tail && m.at != null ? clock(m.at) : '';
      if (suf || tm) {
        const sufA = m.status && m.status.startsWith('failed') ? 't' : 'd', last = rows[rows.length - 1];
        const need = chars(suf) + (suf && tm ? 1 : 0) + chars(tm);
        if (last.e - last.s + 1 + need <= CW) Object.assign(last, { suf, sufA, tm });
        else rows.push({ mi, first: false, lab: false, full: '', qlen: 0, mask: [], s: 0, e: 0, from: m.from, suf, sufA, tm });
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
    box(g, x0, 0, w, h, title, s.line ? false : s.focus === 'conv');
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
        const lab = ' ' + r.sep + ' ', lx = Math.floor((52 - chars(lab)) / 2);
        put(g, x0 + 2, y, '─'.repeat(52), 'b');
        put(g, x0 + 2 + lx, y, lab, 'd');
        return;
      }
      const cur = r.mi === c.cur, sel = !cur && r.mi >= lo && r.mi <= hi;
      const mod = cur ? 'r' : sel ? 's' : '';
      fill(g, x0 + 1, y, w - 2, 't' + mod);
      if (r.lab) put(g, x0 + 2, y, r.from === 'you' ? '[you]' : '[them]', (cur ? 't' : 'd') + mod);
      for (let j = r.s; j < r.e; j++) {
        const fg = r.mask[j] ? 'm' : (j < r.qlen && !cur ? 'd' : 't');
        put(g, x0 + 9 + (j - r.s), y, r.full[j], fg + mod);
      }
      const tmw = r.tm ? chars(r.tm) : 0;
      if (r.tm) put(g, x0 + 2 + 52 - tmw, y, r.tm, (cur ? 't' : 'd') + mod);
      if (r.suf) put(g, x0 + 2 + 52 - tmw - (tmw ? 1 : 0) - chars(r.suf), y, r.suf, (cur ? 't' : r.sufA) + mod);
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
  function shellLines(g, x0, h, head, body, action) {
    let y = 1;
    put(g, x0 + 2, y++, head, 't');
    if (body) wrap(body, 52, 52, true).forEach((r) => put(g, x0 + 2, y++, body.slice(r.s, r.e), 'd'));
    if (action) put(g, x0 + 2, y + 1, action, 't');
  }
  function drawCard(g, s, x0, h) {
    const w = RW, C = s.card;
    box(g, x0, 0, w, h, cardTitle(s), s.line ? false : s.focus === 'profile');
    if (C.sub === 'signedout') { shellLines(g, x0, h, NOT_SIGNED_IN, SIGNED_OUT_REASON, SET_CREDENTIALS); return; }
    if (C.sub === 'reading') { shellLines(g, x0, h, READING, READING_NOTE, ''); return; }
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
    if (C.sub === 'signedout' || C.sub === 'reading') return;
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
        put(g, x0 + 2 + 52 - chars(S.action), y, S.action, 't');
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
    const widest = c.hits.reduce((m, h) => Math.max(m, chars(h)), chars(':' + c.q));
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
    drawBar(g, s, b, top);
    drawStatus(g, s);
    return g;
  }

  /* ---------- output ---------- */
  const SPECIAL = /[^\x20-\x7e┌┐└┘─│]/;
  const CLS = { t: 'ft', d: 'fd', m: 'fm', f: 'ff', b: 'fb', N: 'fN', I: 'fI', V: 'fV', X: 'fX', r: 'rv', s: 'sl', c: 'ci', n: 'cn' };
  const esc = (c) => (c === '&' ? '&amp;' : c === '<' ? '&lt;' : c === '>' ? '&gt;' : c);
  function toHTML(g) {
    return g.map((row) => {
      let out = '', run = '', ra = null;
      const flush = () => { if (run) out += '<span class="' + Array.from(ra).map((x) => CLS[x]).join(' ') + '">' + run + '</span>'; run = ''; };
      row.forEach(([ch, a]) => {
        if (a !== ra || /[cn]/.test(a)) { flush(); ra = a; }
        run += SPECIAL.test(ch) ? '<i class="w">' + esc(ch) + '</i>' : esc(ch);
        if (/[cn]/.test(a)) flush();
      });
      flush();
      return out;
    }).join('\n');
  }
  const toText = (g) => g.map((r) => r.map((c) => c[0]).join(''));

  /* ---------- scripted starts: real keystrokes through the real handler ---------- */
  const TOK = { Esc: 'Escape', CR: 'Enter', Tab: 'Tab', BS: 'Backspace', 'C-j': 'C-j', 'C-w': 'C-w', 'S-CR': 'S-Enter' };
  function feed(s, script) {
    for (let i = 0; i < script.length; i++) {
      if (script[i] === '<') {
        const e = script.indexOf('>', i), name = script.slice(i + 1, e);
        if (name === 'wait') answer(s); else key(s, TOK[name]);
        i = e;
      } else key(s, script[i]);
    }
    return s;
  }
  const DRAFT = 'i' + 'I have the concert tickets and the blue folder. If the side gate is shut, I will ring the bell twice.<C-j>Ten minutes, not more.';
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
    { id: 'visual', name: 'Visual', variants: [
      { name: 'Two messages, search live', keys: '/tickets<CR>vj' }] },
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
      { name: 'nothing read yet', start: 'reading', keys: '' }] },
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
      { name: 'No application credentials', start: 'nocreds', keys: '' }] }
  ];
  function scene(si, vi) {
    const sc = SCENES[si], v = sc.variants[vi], s = fresh(v.start || sc.start);
    return feed(s, v.keys);
  }

  root.TV = { W, H, HINT, ALL_HINTS, HINT_W, fresh, key, feed, render, toHTML, toText, modeName, setMode, scene, SCENES, FOCUS_NAME, chat, chars, answer };
  if (typeof module !== 'undefined') module.exports = root.TV;
})(typeof window !== 'undefined' ? window : globalThis);
