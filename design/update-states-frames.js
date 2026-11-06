/* The update offer: frames as 80-cell rows, the same grammar and the same roles
   global-search-states-frames.js uses (ft body ink, fd dim, fb border, ff border-focused, fN/fX the
   mode label, rv the highlighted cell, ci the insert caret). No role is added and nothing here moves:
   progress is static text redrawn on an event. Every row is checked to be exactly 80 cells and every
   frame to be 24 rows at load, so a frame that does not fit fails here and not in the page.
   Every character used is one cell wide. */
(function () {
  'use strict';
  const W = 80, H = 24, LW = 24, LABEL_W = 8, STATUS_MAX = 69;

  const HINT = {
    normal: ' i:ins  r:rep  e:edit  dd:del  v:vis  /:find  ::cmd  A:card  S:acct',
    list: ' j/k: chat  ⏎: open  Tab: pane  h: conversation  A:card  S:you  p:pin',
    typing: ' ⏎: send  ^J: newline  shift+⏎: newline where supported',
    draft: ' ⏎ draft — i to continue, ^J/⏎ to discard',
    offer: ' h/l: choose  ⏎: confirm  Esc: later',
    progress: ' Esc/⏎: cancel'
  };
  const INDICATOR = 'televim 0.9.0 available — :update';
  const LABEL = { NORMAL: [' NORMAL ', 'fN'], CONFIRM: ['CONFIRM ', 'fX'] };

  const len = (s) => Array.from(s).length;
  const grid = () => Array.from({ length: H }, () => Array.from({ length: W }, () => [' ', 'ft']));
  function put(g, x, y, s, role) {
    Array.from(s).forEach((ch, i) => {
      if (x + i < 0 || x + i >= W || y < 0 || y >= H) throw new Error('"' + s + '" leaves the grid at ' + (x + i) + ',' + y);
      g[y][x + i] = [ch, role];
    });
  }
  const fill = (g, x, y, w, role) => put(g, x, y, ' '.repeat(w), role);

  /* a box: corners and rules in the border role, an optional title and an optional edge text */
  function box(g, x, y, w, h, title, edge, role) {
    put(g, x, y, '┌─ ', role); put(g, x + 3, y, title, 'ft'); put(g, x + 3 + len(title), y, ' ', role);
    put(g, x + 4 + len(title), y, '─'.repeat(w - 5 - len(title)) + '┐', role);
    for (let r = 1; r < h - 1; r++) { put(g, x, y + r, '│', role); put(g, x + w - 1, y + r, '│', role); }
    if (edge) {
      put(g, x, y + h - 1, '└─ ', role); put(g, x + 3, y + h - 1, edge, 'ft'); put(g, x + 3 + len(edge), y + h - 1, ' ', role);
      put(g, x + 4 + len(edge), y + h - 1, '─'.repeat(w - 5 - len(edge)) + '┘', role);
    } else put(g, x, y + h - 1, '└' + '─'.repeat(w - 2) + '┘', role);
  }

  /* ---------- the screen under every frame: Ada Lovelace's conversation, 80 by 24 ---------- */
  const CHATS = [['Ada Lovelace', ''], ['Grace Hopper', '2'], ['Alan Turing', '1'], ['Katherine Johnson', ''],
    ['Margaret Hamilton', ''], ['Barbara Liskov', ''], ['Dennis Ritchie', ''], ['Edsger Dijkstra', ''],
    ['Hedy Lamarr', ''], ['Donald Knuth', '3'], ['Radia Perlman', ''], ['Frances Allen', ''], ['Noa Friedman', '2']];
  const MESSAGES = [
    ['them', 'Are you coming tonight?'],
    ['you', 'Running late. Save me a seat.'],
    ['them', '> Save me a seat. ‖ The one by the window.'],
    ['them', 'Bring the tickets, the tickets are in the'],
    [null, 'blue folder.'],
    ['you', 'Which tickets? I have the train ones and the'],
    [null, 'concert ones.'],
    ['them', 'The concert tickets. The train ones can stay'],
    [null, 'on the fridge.'],
    ['you', 'Tickets are under the lamp. Folder found.'],
    ['them', 'Doors open at eight, and the side gate is'],
    [null, 'shut after nine, so do not be later.'],
    ['you', 'On my way, ten minutes.'],
    ['them', 'I will keep your seat.']
  ];

  /* o: focus ('list' | 'conv' | 'bar' | 'none' when an overlay owns the keys), bar: {title, text, lit},
     indicator: bool, mode, status, dot: always drawn */
  function screen(o) {
    const g = grid();
    const listRole = o.focus === 'list' ? 'ff' : 'fb', convRole = o.focus === 'conv' ? 'ff' : 'fb';
    box(g, 0, 0, LW, 20, 'Chats (13)', null, listRole);
    box(g, LW, 0, W - LW, 20, 'Conversation (14/14)', null, convRole);
    CHATS.forEach(([name, n], i) => {
      const cur = i === 0, role = cur ? 'rv ft' : 'ft';
      fill(g, 1, 1 + i, LW - 2, role);
      put(g, 2, 1 + i, name, role);
      if (n) put(g, 21, 1 + i, n, cur ? 'rv ft' : 'fd');
    });
    MESSAGES.forEach(([who, text], i) => {
      const y = 1 + i, cur = i === MESSAGES.length - 1, role = cur ? 'rv ft' : 'ft';
      if (cur) fill(g, 25, y, 53, role);
      if (who) put(g, 26, y, who === 'you' ? '[you] ' : '[them]', cur ? role : 'fd');
      put(g, 33, y, text, role);
    });
    for (let y = 1; y < 19; y++) put(g, 78, y, y < 16 ? '█' : '░', 'fd');
    /* the bar: always a draft or a hint, never absent. Lit while it is being typed in. */
    const b = o.bar || { title: 'Input', text: '', lit: false };
    const bRole = b.lit ? 'ff' : 'fb';
    box(g, 0, 20, W, 3, b.title, null, bRole);
    if (b.text) {
      Array.from(b.text).forEach((ch, i) => put(g, 2 + i, 21, b.lit && ch === ' ' ? '·' : ch, b.lit && ch === ' ' ? 'fd' : (b.lit ? 'ft' : 'fd')));
      if (b.lit) put(g, 2 + len(b.text), 21, ' ', 'ci');
    }
    /* The update indicator sits on the bar's bottom border, above the status row. */
    if (o.indicator) {
      put(g, 78 - len(INDICATOR) - 1, 22, ' ' + INDICATOR + ' ', 'fd');
    }
    if (o.overlay) o.overlay(g);
    /* the status row: the mode label, the ranked sentence, a space, the connection dot */
    const lb = o.input ? [' INSERT ', 'fI'] : LABEL[o.mode || 'NORMAL'];
    if (len(o.status) > STATUS_MAX) throw new Error('status is ' + len(o.status) + ' cells: ' + o.status);
    put(g, 0, 23, lb[0], lb[1]);
    put(g, LABEL_W, 23, ' ', 'ft');
    put(g, LABEL_W + 1, 23, '●', 'ft');
    put(g, LABEL_W + 2, 23, ' ', 'ft');
    put(g, LABEL_W + 3, 23, o.status, 'ft');
    return g;
  }

  /* ---------- the dialog: a box over the conversation, 53 by n, flush with the panel's inner edge ---------- */
  const DX = 25, DW = 53;
  function dialog(title, edge, lines, buttons, hot) {
    return (g) => {
      const h = lines.length + 4;
      const y0 = 3;
      box(g, DX, y0, DW, h, title, edge, 'ff');
      for (let r = 1; r < h - 1; r++) fill(g, DX + 1, y0 + r, DW - 2, 'ft');
      lines.forEach((l, i) => { if (l) put(g, DX + 3, y0 + 2 + i, l[0], l[1] || 'ft'); });
      /* the actions, centred on the last interior row: reverse video marks the highlighted one,
         and the brackets stay on both, so the highlight is never the only cue */
      const cells = buttons.map((b, i) => ({ t: '[ ' + b + ' ]', hot: i === hot }));
      const total = cells.reduce((n, c) => n + len(c.t), 0) + 2 * (cells.length - 1);
      let x = DX + Math.floor((DW - total) / 2);
      cells.forEach((c) => { put(g, x, y0 + h - 2, c.t, c.hot ? 'rv ft' : 'ft'); x += len(c.t) + 2; });
    };
  }
  const kv = (k, v) => [k.padEnd(12) + v];
  const OFFER_LINES = [
    ['A newer stable release is available.'], null, kv('installed', '0.8.0'), kv('available', '0.9.0'), null,
    ['Update downloads the verified archive and'], ['replaces the binary. This window keeps'],
    ['running 0.8.0 until you restart.'], null
  ];
  const DRAFT = 'I have the concert tickets and the blue folder.';
  const draftBar = { title: 'draft', text: DRAFT, lit: false };
  const typingBar = { title: 'Message to Ada Lovelace', text: DRAFT, lit: true };

  const FIX = [['Retry with :update, or rerun the installer'], ['from the README.']];
  const FRAMES = [
    { id: 'indicator', state: 'indicator',
      label: 'Chat list open, a draft being typed. The indicator is the bar\'s bottom edge; nothing took the focus.',
      mode: 'NORMAL', input: true, status: HINT.typing, focus: 'bar', bar: typingBar, indicator: true },
    { id: 'offer', state: 'offer',
      label: 'The offer, raised with :update over a kept draft. Two actions; Later is highlighted, the bar is not.',
      mode: 'NORMAL', status: HINT.offer, focus: 'none', bar: draftBar, indicator: true,
      overlay: dialog('Update televim', null, OFFER_LINES, ['Update', 'Later'], 1) },
    { id: 'progress', state: 'progress',
      label: 'After Update: static stage text for verification. Cancel works between stages; the draft is where it was.',
      mode: 'NORMAL', status: HINT.progress, focus: 'none', bar: draftBar, indicator: true,
      overlay: dialog('Update televim', null, [
        ['Updating televim to 0.9.0.'], null, ['[update… verifying]'], null,
        ['download  > verify  install'], null, kv('installed', '0.8.0'), kv('available', '0.9.0'), null
      ], ['Cancel'], 0) },
    { id: 'success', state: 'success',
      label: 'Installed. One action that quits, and Later, which is highlighted: nothing closes without a keystroke.',
      mode: 'NORMAL', status: HINT.offer, focus: 'none', bar: draftBar, indicator: false,
      overlay: dialog('televim 0.9.0 is installed', null, [
        ['Restart televim to use 0.9.0. This window'], ['keeps running 0.8.0 until you do.'], null,
        ['Restart asks first. Later leaves'], ['the install in place; your draft is kept.'], null
      ], ['Restart', 'Later'], 1) },
    { id: 'failure-checksum', state: 'failure',
      label: 'Checksum failure: the archive was discarded, nothing was changed, and the sentence says so.',
      mode: 'NORMAL', status: HINT.offer, focus: 'none', bar: draftBar, indicator: true,
      overlay: dialog('Update failed', null, [
        ['Checksum failed — nothing was installed.'], null,
        ['The archive did not match its published'], ['checksum and was discarded. 0.8.0 is still'], ['installed.'], null,
        FIX[0], FIX[1], null
      ], ['Retry', 'Later'], 1) },
    { id: 'failure-network', state: 'failure',
      label: 'Network failure: the same shape and the same fallback, a different first sentence.',
      mode: 'NORMAL', status: HINT.offer, focus: 'none', bar: draftBar, indicator: true,
      overlay: dialog('Update failed', null, [
        ['Network failure — nothing was installed.'], null,
        ['televim could not reach the release server.'], ['0.8.0 is still installed.'], null, null,
        FIX[0], FIX[1], null
      ], ['Retry', 'Later'], 1) },
    { id: 'dismissed', state: 'dismissed',
      label: 'After Later: the indicator remains, the dialog stays dismissed for 0.9.0, and the draft is kept.',
      mode: 'NORMAL', status: HINT.draft, focus: 'conv', bar: draftBar, indicator: true },
    { id: 'dismissed-relaunch', state: 'dismissed',
      label: 'A second launch: no dialog returns for 0.9.0; the quiet indicator remains.',
      mode: 'NORMAL', status: HINT.list, focus: 'list', bar: { title: 'Input', text: '', lit: false }, indicator: true },
    { id: 'restart', state: 'restart',
      label: 'Restart: the existing confirmation row, CONFIRM in the one red role, the dialog gone.',
      mode: 'CONFIRM', status: ' Restart televim? (y/n)', focus: 'conv', bar: draftBar, indicator: false }
  ];

  /* merge a cell grid into role runs, and check the two numbers this page exists to keep */
  function toRows(g) {
    if (g.length !== H) throw new Error('frame is ' + g.length + ' rows');
    return g.map((row, i) => {
      if (row.length !== W) throw new Error('row ' + i + ' is ' + row.length + ' cells');
      const out = [];
      row.forEach(([ch, role]) => {
        const last = out[out.length - 1];
        if (last && last[1] === role) last[0] += ch; else out.push([ch, role]);
      });
      return out;
    });
  }
  const frames = FRAMES.map((f) => Object.assign({}, f, { rows: toRows(screen(f)) }));
  window.US = { W, H, HINT, INDICATOR, frames };
})();
