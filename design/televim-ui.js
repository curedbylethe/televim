/* Wires the engine to the page: switchers, real keyboard, and re-render on every keystroke. */
(function () {
  'use strict';
  const TV = window.TV;
  const app = { si: 0, vi: 0, theme: 'dark', s: null };
  const $ = (id) => document.getElementById(id);
  const MODE_CLASS = { NORMAL: 'N', INSERT: 'I', VISUAL: 'V', CONFIRM: 'X' };

  function btn(parent, label, onclick, extra) {
    const b = document.createElement('button');
    b.type = 'button'; b.textContent = label; if (extra) b.className = extra;
    b.addEventListener('mousedown', (e) => e.preventDefault()); /* keys keep going to the program */
    b.addEventListener('click', onclick);
    parent.appendChild(b); return b;
  }
  function pressed(container, i) { Array.from(container.children).forEach((b, j) => b.setAttribute('aria-pressed', j === i ? 'true' : 'false')); }

  function load(si, vi) { app.si = si; app.vi = vi; app.s = TV.scene(si, vi); draw(); }

  function buildVariants() {
    const c = $('variants'); c.textContent = '';
    TV.SCENES[app.si].variants.forEach((v, i) => btn(c, v.name, () => load(app.si, i)));
  }

  function draw() {
    const s = app.s, g = TV.render(s), html = TV.toHTML(g);
    const themes = app.theme === 'both' ? ['dark', 'light'] : [app.theme];
    const host = $('frames');
    host.textContent = '';
    themes.forEach((t) => {
      const fig = document.createElement('figure'); fig.className = 'fig';
      if (themes.length > 1) { const cap = document.createElement('figcaption'); cap.textContent = t + ' terminal'; fig.appendChild(cap); }
      const pre = document.createElement('pre');
      pre.className = 'screen'; pre.setAttribute('data-terminal', t); pre.setAttribute('data-od-id', 'frame-' + t);
      pre.setAttribute('role', 'img');
      pre.setAttribute('aria-label', 'televim, 80 by 24. Mode ' + TV.modeName(s) + '. Keys go to the ' + TV.FOCUS_NAME[s.focus] + '.');
      pre.innerHTML = html; fig.appendChild(pre); host.appendChild(fig);
    });
    const mode = TV.modeName(s);
    pressed($('screens'), app.si); pressed($('variants'), app.vi);
    Array.from($('modes').children).forEach((b) => b.setAttribute('aria-pressed', b.textContent === mode ? 'true' : 'false'));
    const where = TV.FOCUS_NAME[s.focus] || 'shell';
    $('caption').innerHTML = '<b>' + g[0].length + ' × ' + g.length + '</b> cells · mode <b>' + mode + '</b> · keys go to the <b>' + where + '</b>';
    document.title = 'televim: ' + mode + ' (80×24)';
    schedule();
  }

  /* a send resolves a moment later, and a sign-in request answers on its own: the reader's ⏎
     never hurries it. Nothing else in the interface repaints on a schedule. */
  function schedule() {
    if (app.s.view === 'signin' && app.s.signin && app.s.signin.checking) {
      const st = app.s;
      if (!st.signin.timer) st.signin.timer = setTimeout(() => {
        st.signin.timer = null; TV.answer(st); draw();
      }, 1200);
      return;
    }
    if (app.s.jump && !app.s.jumpTimer) {
      const st = app.s;
      st.jumpTimer = setTimeout(() => { st.jumpTimer = null; TV.answer(st); draw(); }, 900);
    }
    /* the peer's typing note runs out on the network tick, not on a clock of its own */
    const open = app.s.chats[app.s.chat];
    if (open && open.typing && !open.typingTimer) {
      const st = app.s;
      open.typingTimer = setTimeout(() => { open.typingTimer = null; if (app.s === st) { TV.answer(st); draw(); } }, 3000);
    }
    if (app.s.newchat && app.s.newchat.pending && !app.s.newchatTimer) {
      const st = app.s;
      st.newchatTimer = setTimeout(() => { st.newchatTimer = null; if (app.s === st) { TV.answer(st); draw(); } }, 1200);
    }
    if (app.s.view !== 'chat') return;
    app.s.chats.forEach((c) => c.msgs.forEach((m) => {
      if (m.live && m.status && !m.t) m.t = setTimeout(() => { m.status = null; m.live = false; draw(); }, 1400);
    }));
  }

  function toKey(e) {
    if (e.metaKey || e.altKey) return null;
    if (e.ctrlKey) { const c = e.key.toLowerCase(); return c.length === 1 && 'jwduoi'.includes(c) ? 'C-' + c : null; }
    const named = { Escape: 'Escape', Enter: e.shiftKey ? 'S-Enter' : 'Enter', Tab: 'Tab', Backspace: 'Backspace', Delete: 'Delete', ArrowLeft: 'Left', ArrowRight: 'Right', ArrowUp: 'Up', ArrowDown: 'Down', Home: 'Home', End: 'End' };
    if (named[e.key]) return named[e.key];
    return e.key.length === 1 ? e.key : null;
  }
  function press(seq) { (Array.isArray(seq) ? seq : [seq]).forEach((k) => TV.key(app.s, k)); draw(); }

  document.addEventListener('keydown', (e) => {
    const k = toKey(e);
    if (!k) return;
    e.preventDefault(); press(k);
  });

  /* switchers */
  TV.SCENES.forEach((sc, i) => btn($('screens'), (i + 1) + ' ' + sc.name, () => { load(i, 0); buildVariants(); pressed($('variants'), 0); }));
  ['NORMAL', 'INSERT', 'VISUAL', 'CONFIRM'].forEach((m) => btn($('modes'), m, () => {
    if (app.s.view !== 'chat') load(0, 0), buildVariants();
    TV.setMode(app.s, m); draw();
  }, 'mode-' + MODE_CLASS[m]));
  ['dark', 'light', 'both'].forEach((t) => btn($('themes'), t, () => { app.theme = t; pressed($('themes'), ['dark', 'light', 'both'].indexOf(t)); draw(); }));
  [['Esc', 'Escape'], ['Tab', 'Tab'], ['Enter', 'Enter'], ['Shift+Enter', 'S-Enter'],
   ['gd', ['g', 'd']], ['Ctrl+o', 'C-o'], ['Ctrl+i', 'C-i'], ['Ctrl+w h', ['C-w', 'h']], ['Ctrl+w l', ['C-w', 'l']],
   ['A', 'A'], ['S', 'S'], ['l', 'l'], ['h', 'h'], ['v', 'v'], ['y', 'y'], ['yy', ['y', 'y']], ['d', 'd'],
   ['/', ['Tab', '/']], [':new', [':', 'n', 'e', 'w', 'Enter']]
  ].forEach(([l, k]) => btn($('keys'), l, () => press(k)));

  pressed($('themes'), 0);
  buildVariants();
  load(0, 0);
})();
