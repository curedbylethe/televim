/* Global message search, the next five states: frames as 80-cell rows, the same grammar and the
   same roles global-search-frames.js uses (ft body ink, fd dim, fm match, fb border, fN/fI the mode
   label, rv the cursor row, ci the insert caret). Every row is checked to be exactly 80 cells and
   every frame to be 24 rows here, so a frame that does not fit fails at load, not in the page.
   Rows carry one or two snippet lines: `s` is an array, 1 or 2 entries, each at most 62 cells. */
(function () {
  'use strict';
  const W = 80, IW = 78, LIST_ROWS = 21, GUT = 16, SNIP = 62, STATUS_MAX = 69;
  const HINT = 'j/k: move  ⏎: open  { }: chat  gg/G: ends  Esc: close';
  const HINT_IDLE = 'Esc: close';
  const LABEL = { NORMAL: [' NORMAL  ', 'fN'], INSERT: [' INSERT  ', 'fI'] };

  const len = (s) => Array.from(s).length;
  const width = (segs) => segs.reduce((n, [t]) => n + len(t), 0);
  const rule = (n, role) => (n > 0 ? [['─'.repeat(n), role]] : []);

  function padTo(segs, n, role) {
    const k = n - width(segs);
    if (k < 0) throw new Error('row is ' + -k + ' cells too wide: ' + JSON.stringify(segs));
    return k ? segs.concat([[' '.repeat(k), role]]) : segs;
  }

  /* '<m>' opens a match and '</m>' closes it; everything else is body ink. */
  function snip(s) {
    const out = []; let on = false;
    s.split(/(<\/?m>)/).forEach((p) => {
      if (p === '<m>') on = true;
      else if (p === '</m>') on = false;
      else if (p) out.push([p, on ? 'fm' : 'ft']);
    });
    return out;
  }

  /* On the cursor row the whole row is reverse video. Its quieter text takes body ink, because a dim
     foreground under reverse video would become a dim background. A match stays a match. */
  function onRow(segs) { return segs.map(([t, r]) => [t, r === 'fm' ? 'rv fm' : 'rv ft']); }

  /* A hit is one selectable thing drawn on one or two rows, so the cursor reverses every row it
     occupies. Time and tag sit on the first row only; the second is indented under the snippet. */
  function message(m, cur) {
    const head = [['  ', 'ft'], [m.t.padEnd(5), 'fd'], ['  ', 'ft'], [m.who.padEnd(6), 'fd'], [' ', 'ft']];
    const rows = [padTo(head.concat(snip(m.s[0])), IW, 'ft')];
    if (m.s[1]) rows.push(padTo([[' '.repeat(GUT), 'ft']].concat(snip(m.s[1])), IW, 'ft'));
    return cur ? rows.map(onRow) : rows;
  }

  /* one rule per chat, the name set into it, the count dim: the day separator's shape */
  function header(name, n) {
    const h = [['──── ', 'fb'], [name, 'ft'], [' · ' + n + ' ', 'fd']];
    return h.concat(rule(IW - width(h), 'fb'));
  }

  /* The prompt is the box's own last interior row: the overlay owns rows 0 to 22, so the screen's bar
     cannot hold it. Two columns of indent, the query, the insert caret on the cell after it. */
  function promptRow(q) {
    return padTo([['  ', 'ft']].concat(snip(q), [[' ', 'ci']]), IW, 'ft');
  }

  function list(f) {
    const rows = [];
    if (f.prompt) {
      for (let i = 0; i < LIST_ROWS - 1; i++) rows.push(padTo([], IW, 'ft'));
      rows.push(promptRow(f.prompt));
      return rows;
    }
    let k = 0;
    f.groups.forEach((g) => {
      rows.push(header(g.name, g.n));
      g.msgs.forEach((m) => { message(m, k === f.cursor).forEach((r) => rows.push(r)); k++; });
    });
    if (rows.length > LIST_ROWS) throw new Error('the list holds ' + LIST_ROWS + ' rows, not ' + rows.length);
    while (rows.length < LIST_ROWS) rows.push(padTo([], IW, 'ft'));
    return rows;
  }

  function frame(f) {
    if (len(f.status) > STATUS_MAX) throw new Error('status is ' + len(f.status) + ' cells: ' + f.status);
    const title = 'Search';
    const top = [['┌─ ', 'fb'], [title, 'ft'], [' ', 'fb']].concat(rule(W - 5 - len(title), 'fb'), [['┐', 'fb']]);
    const bottom = [['└─ ', 'fb'], [f.hint, 'ft'], [' ', 'fb']].concat(rule(W - 5 - len(f.hint), 'fb'), [['┘', 'fb']]);
    const lb = LABEL[f.mode || 'NORMAL'];
    const label = [[lb[0], lb[1]], [' ', 'ft'], [f.status, 'ft']];
    const body = list(f).map((r) => [['│', 'fb']].concat(r, [['│', 'fb']]));
    const rows = [top].concat(body, [bottom], [padTo(label, W, 'ft')]);
    if (rows.length !== 24) throw new Error('frame is ' + rows.length + ' rows');
    rows.forEach((r, i) => { if (width(r) !== W) throw new Error('row ' + i + ' is ' + width(r) + ' cells'); });
    return rows;
  }

  /* ---------- the result set the existing overlay art draws, now two lines per hit ---------- */
  const ADA = [
    { t: '18:40', who: '[them]', s: ['I put the <m>tickets</m> in the blue folder on the shelf, so do not',
      'forget them on Friday. The train <m>tickets</m> are in the envelope.…'] },
    { t: '18:52', who: '[you]', s: ['Which <m>tickets</m>? I have the train ones and the concert ones, and',
      'the concert <m>tickets</m> are the ones you printed at the library.'] },
    { t: '19:03', who: '[them]', s: ['The concert <m>tickets</m>. The train ones stay on the fridge until',
      'Friday, when my sister comes round to fetch them.…'] },
    { t: '19:10', who: '[you]', s: ['<m>Tickets</m> are under the lamp. Folder found.'] }
  ];
  const GRACE = [
    { t: '20:14', who: '[them]', s: ['Did you keep the <m>tickets</m> for the late train? The gate shuts at',
      'nine, and the man on the barrier will not let you through.…'] },
    { t: '20:15', who: '[you]', s: ['Yes, the <m>tickets</m> are in my coat, and the receipt you asked for',
      'is with them. I will bring both <m>tickets</m> at half past seven.…'] },
    { t: '21:30', who: '[them]', s: ['Left the <m>tickets</m> on the bench by mistake. Went back at ten and',
      'they were still there, so nothing is lost at all.…'] }
  ];
  const MARGARET = [
    { t: '09:05', who: '[them]', s: ['Can you send the <m>tickets</m> for Saturday? The box office said the',
      'seats are held until Thursday, and the <m>tickets</m> are the proof.'] },
    { t: '10:21', who: '[you]', s: ['<m>Tickets</m> confirmed. See you Saturday.'] }
  ];
  const G = (name, n, msgs) => ({ name, n, msgs });
  const SAME = () => [G('Ada Lovelace', 4, ADA), G('Grace Hopper', 3, GRACE), G('Margaret Hamilton', 2, MARGARET)];

  /* ---------- the paging set: a page boundary the reader can see ---------- */
  const GA = [
    { t: '17:58', who: '[them]', s: ['The <m>gate</m> shuts at nine on weekdays, and at eight on Sundays,',
      'so leave by half past if you want to catch the last one.…'] },
    { t: '18:06', who: '[you]', s: ['I waited at the <m>gate</m> for twenty minutes. Nobody came, so I',
      'walked round to the side door and it was open the whole time.…'] },
    { t: '18:20', who: '[them]', s: ['<m>Gate</m> code changed again.'] },
    { t: '18:31', who: '[you]', s: ['Call the porter at the <m>gate</m> if the code does not work. His',
      'number is on the box by the door, under the plastic flap.…'] }
  ];
  const GG = [
    { t: '19:02', who: '[them]', s: ['Which <m>gate</m> is the concert one? The north <m>gate</m> was locked',
      'when we came past at seven, and nobody was in the hut.…'] },
    { t: '19:04', who: '[you]', s: ['The <m>gate</m> by the bins is always open after eight, whatever the',
      'sign says, so meet me on the other side of it at half seven.…'] },
    { t: '19:20', who: '[them]', s: ['Meet at the <m>gate</m> at half seven.'] }
  ];
  const GM = [
    { t: '08:12', who: '[them]', s: ['Security on the <m>gate</m> asked for the reference, so have the',
      'email open on your phone before you get to the barrier.…'] },
    { t: '08:40', who: '[you]', s: ['The <m>gate</m> is fine now. They fixed the lock this morning and it',
      'shuts quietly, which the neighbours will be pleased about.…'] }
  ];

  const FRAMES = [
    /* 1. two-line preview */
    { id: 'two-line', state: 'two-line', label: 'Two lines of context per hit. The cursor is on the first hit, and both of its rows are reversed.',
      mode: 'NORMAL', status: '?tickets — 9 results in 3 chats', cursor: 0, hint: HINT, groups: SAME() },
    { id: 'two-line-one', state: 'two-line', label: 'A hit whose body is one line draws one, and the cursor reverses one row.',
      mode: 'NORMAL', status: '?tickets — 9 results in 3 chats', cursor: 8, hint: HINT, groups: SAME() },

    /* 2. dropped count */
    { id: 'dropped-nine', state: 'dropped', label: 'Nine kept, three dropped: the same nine hits, the same cursor, one clause more.',
      mode: 'NORMAL', status: '?tickets — 9 results in 3 chats, 3 dropped', cursor: 0, hint: HINT, groups: SAME() },
    { id: 'dropped-none', state: 'dropped', label: 'Nothing kept, four dropped: no list, and the sentence says why.',
      mode: 'NORMAL', status: '?tickets — no matches, 4 dropped', cursor: -1, hint: HINT_IDLE, groups: [] },

    /* 3. filters */
    { id: 'filters-prompt', state: 'filters', label: 'The prompt, with media and two dates typed as tokens. The status names the vocabulary.',
      mode: 'INSERT', status: 'Filters: photo_video, document, music, voice, url — since:, until:',
      cursor: -1, hint: HINT_IDLE, prompt: 'cat media:photo_video since:2026-01-01 until:2026-02-01', groups: [] },
    { id: 'filters-results', state: 'filters', label: 'Filters active: the values are named in the status, before the count.',
      mode: 'NORMAL', status: '?tickets — photo_video, 2026-01-01 to 2026-02-01 — 9 results', cursor: 0, hint: HINT, groups: SAME() },
    { id: 'filters-refused', state: 'filters', label: 'A malformed token: the search is refused, the query stays, the flash names the token.',
      mode: 'INSERT', status: 'media:photo is not a media filter: try photo_video.',
      cursor: -1, hint: HINT_IDLE, prompt: 'cat media:photo since:2026-01-01', groups: [] },

    /* 4. rate limit */
    { id: 'wait-results', state: 'wait', label: 'FLOOD_WAIT with results: paused, counting down, the rows and the cursor untouched.',
      mode: 'NORMAL', status: '?tickets — 9 results in 3 chats, paused: retry in 42s', cursor: 4, hint: HINT, groups: SAME() },
    { id: 'wait-empty', state: 'wait', label: 'FLOOD_WAIT with nothing held: the wait is the whole state.',
      mode: 'NORMAL', status: '?tickets — paused: retry in 42s', cursor: -1, hint: HINT_IDLE, groups: [] },

    /* 5. the next page */
    { id: 'paging-flight', state: 'paging', label: 'The cursor reached the last hit of the page; the next one is in flight.',
      mode: 'NORMAL', status: '?gate — 7 results in 2 chats, loading more…', cursor: 6, hint: HINT,
      groups: [G('Ada Lovelace', 4, GA), G('Grace Hopper', 3, GG)] },
    { id: 'paging-done', state: 'paging', label: 'The last page is in: appended under the same cursor, and the count is the full total.',
      mode: 'NORMAL', status: '?gate — 9 results in 3 chats', cursor: 6, hint: HINT,
      groups: [G('Ada Lovelace', 4, GA), G('Grace Hopper', 3, GG), G('Margaret Hamilton', 2, GM)] }
  ];

  const frames = FRAMES.map((f) => Object.assign({}, f, { rows: frame(f) }));
  window.GS2 = { W, IW, HINT, HINT_IDLE, frames };
})();
