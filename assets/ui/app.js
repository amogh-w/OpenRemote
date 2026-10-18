(() => {
  'use strict';
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const store = {
    get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
    set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
  };
  const NS = 'http://www.w3.org/2000/svg';
  // Icons are Lucide symbols from the inline sprite (see assets/ui/kit/icons.txt); names are Lucide names.
  function svg(name, cls) {
    const s = document.createElementNS(NS, 'svg');
    s.setAttribute('class', cls || 'size-5'); s.setAttribute('aria-hidden', 'true');
    const u = document.createElementNS(NS, 'use'); u.setAttribute('href', '#' + name); s.appendChild(u);
    return s;
  }
  const setIcon = (svgEl, name) => svgEl.querySelector('use').setAttribute('href', '#' + name);

  // ============================================================ settings
  const S = Object.assign({ theme: 'auto', accent: 'blue', speed: 15, scroll: 3, natural: true, haptics: true }, store.get('or_settings', {}));
  S.theme = { light: 'latte', dark: 'mocha' }[S.theme] || S.theme;          // values saved by older versions
  S.accent = { purple: 'mauve', orange: 'peach' }[S.accent] || S.accent;
  const FLAVORS = ['latte', 'frappe', 'macchiato', 'mocha'];
  const flavor = () => (FLAVORS.includes(S.theme) ? S.theme : matchMedia('(prefers-color-scheme: dark)').matches ? 'mocha' : 'latte');
  const saveS = () => store.set('or_settings', S);
  const buzz = () => { if (S.haptics && navigator.vibrate) navigator.vibrate(8); };
  const pct = (el) => ((el.value - el.min) / (el.max - el.min)) * 100 + '%';

  function paintSettings() {
    const root = document.documentElement;
    root.classList.remove(...FLAVORS); root.classList.add(flavor());
    root.style.setProperty('--accent', 'var(--ctp-' + S.accent + ')');
    const m = $('meta[name=theme-color]');
    if (m) m.content = getComputedStyle(root).getPropertyValue('--ctp-mantle').trim() || m.content;
    $$('#s-flavors .chip').forEach((b) => b.setAttribute('aria-pressed', b.dataset.v === S.theme));
    $$('#s-accents button').forEach((b) => b.classList.toggle('on', b.dataset.v === S.accent));
    [['#s-speed', '#v-speed', 'speed'], ['#s-scroll', '#v-scroll', 'scroll']].forEach(([i, v, k]) => {
      const el = $(i); el.value = S[k]; el.style.setProperty('--p', pct(el)); $(v).textContent = S[k];
    });
    [['#s-natural', 'natural'], ['#s-haptics', 'haptics']].forEach(([id, k]) => { $(id).checked = !!S[k]; });
  }
  $('#s-flavors').onclick = (e) => { const b = e.target.closest('.chip'); if (b) { S.theme = b.dataset.v; saveS(); paintSettings(); buzz(); } };
  $('#s-accents').onclick = (e) => { const b = e.target.closest('button'); if (b) { S.accent = b.dataset.v; saveS(); paintSettings(); buzz(); } };
  $('#s-speed').oninput = (e) => { S.speed = +e.target.value; saveS(); paintSettings(); };
  $('#s-scroll').oninput = (e) => { S.scroll = +e.target.value; saveS(); paintSettings(); };
  $('#s-natural').onchange = (e) => { S.natural = e.target.checked; saveS(); buzz(); };
  $('#s-haptics').onchange = (e) => { S.haptics = e.target.checked; saveS(); buzz(); };
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => { if (S.theme === 'auto') paintSettings(); });

  // ============================================================ toast
  let toastT = 0;
  function toast(msg, o = {}) {
    const el = $('#toast');
    $('span', el).textContent = msg;
    el.classList.toggle('bad', !!o.bad);
    setIcon($('svg', el), o.icon || (o.bad ? 'wifi-off' : 'info'));
    el.classList.add('show');
    clearTimeout(toastT); toastT = setTimeout(() => el.classList.remove('show'), o.ms || 2200);
  }

  // ============================================================ the PC
  // The page is served by the PC it controls, so it always connects straight back to its own origin.
  const pc = { base: location.origin, name: location.hostname };
  try { localStorage.removeItem('or_comps'); localStorage.removeItem('or_cur'); } catch {}   // from older versions

  // ============================================================ PWA
  (function pwa() {
    const l = document.createElement('link');
    l.rel = 'manifest';
    l.href = '/manifest.webmanifest';
    document.head.appendChild(l);
    if ('serviceWorker' in navigator && window.isSecureContext) navigator.serviceWorker.register('/sw.js').catch(() => {});
    const standalone = matchMedia('(display-mode: standalone)').matches || navigator.standalone;
    const ios = /iphone|ipad|ipod/i.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
    const help = $('#installhelp');
    if (standalone) help.textContent = 'Running as an installed app.';
    else if (ios) help.textContent = 'In Safari tap Share, then Add to Home Screen. It opens full screen like an app.';
    else if (window.isSecureContext) help.textContent = 'Use your browser menu: Install app, or tap Install.';
    else help.textContent = 'Browser menu, Add to Home screen. This page is plain HTTP, so Chrome only makes a shortcut. For a full-screen install on Android, open chrome://flags, enable "Insecure origins treated as secure" and add ' + location.origin + '.';
    addEventListener('beforeinstallprompt', (e) => { e.preventDefault(); const b = $('#installbtn'); b.hidden = false; b.onclick = () => e.prompt(); });
  })();

  // ============================================================ connection
  let ws = null, retry = 0, timer = 0, online = false, everOnline = false, wasLost = false, rtt = null, pingAt = 0, warned = false;
  let connState = 'wait';
  function setConn(state) {
    online = state === 'on';
    const d = $('#dot'); d.classList.toggle('on', online); d.classList.toggle('wait', state === 'wait');
    connState = state; paintStatus();
  }
  const paintName = () => paintStatus();
  function paintStatus() { $('#status').textContent = connState === 'on' ? 'Connected to ' + pc.name : connState === 'wait' ? 'Connecting to ' + pc.name + '...' : 'Offline, retrying'; }
  function connect() {
    clearTimeout(timer);
    if (ws) { ws.onclose = null; ws.onmessage = null; try { ws.close(); } catch {} ws = null; }
    rtt = null; setConn('wait'); paintName();
    const sock = new WebSocket(pc.base.replace(/^http/, 'ws') + '/ws');
    ws = sock;
    sock.onopen = () => { retry = 0; };
    sock.onmessage = (e) => {
      let m; try { m = JSON.parse(e.data); } catch { return; }
      if (m.t === 'hello') {
        pc.name = m.host || pc.name; paintName(); setConn('on');
        $('#helperstate').textContent = m.helper
          ? 'On. The mouse and keyboard also work on UAC prompts, administrator windows and the lock screen.'
          : 'Off. To use the mouse on UAC prompts, run "openremote --install-helper --yes" on the PC (see the README).';
        if (everOnline && wasLost) toast('Reconnected', { icon: 'wifi' });
        everOnline = true; wasLost = false; warned = false; appsRequested = false;
        if (page === 'apps') refreshApps();
        askMedia();
      } else if (m.t === 'pong') { rtt = Math.round(performance.now() - pingAt); if (online) setConn('on'); }
      else if (m.t === 'media') gotMedia(m);
      else if (m.t === 'clip') gotClip(m);
      else if (m.t === 'clip.ack') gotAck(m);
      else if (m.t === 'win.list') { winItems = m.items || []; winLoaded = true; renderWins(); }
      else if (m.t === 'app.list') { appItems = m.items || []; appsLoaded = true; renderApps(); }
    };
    sock.onclose = () => {
      if (online) { wasLost = true; toast('Connection lost', { bad: true }); }
      retry++;
      setConn(retry >= 3 ? 'off' : 'wait');
      if (retry === 3 && !everOnline && !warned) { warned = true; toast("Can't reach the PC. Is it running, on the same Wi-Fi?", { bad: true, ms: 4000 }); }
      timer = setTimeout(connect, Math.min(3000, 400 * retry));
      if (retry === 2) pairStatus().then((s) => { if (s && s.required && !s.paired) { clearTimeout(timer); ws = null; showPair('This phone is not paired (any more). Pair it again.'); } });
    };
  }
  const send = (o) => { if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); };
  setInterval(() => { if (ws && ws.readyState === 1 && !document.hidden) { pingAt = performance.now(); send({ t: 'ping' }); } }, 4000);
  if (navigator.wakeLock) {
    const wl = () => navigator.wakeLock.request('screen').catch(() => {});
    wl(); document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && wl());
  }
  document.addEventListener('gesturestart', (e) => e.preventDefault());

  // ============================================================ shared press helper
  // down=true: act immediately on press (keyboard). Default: act on release, so scrolling a row never fires keys.
  function bindPress(el, fn, { repeat = false, down = false } = {}) {
    let t1 = 0, t2 = 0, fired = false, active = false;
    const stop = () => { clearTimeout(t1); clearInterval(t2); active = false; };
    el.addEventListener('pointerdown', (e) => {
      active = true; fired = false;
      if (down) { e.preventDefault(); fn(); fired = true; buzz(); }
      if (repeat) t1 = setTimeout(() => { if (!active) return; fired = true; fn(); t2 = setInterval(fn, 55); }, 350);
    });
    el.addEventListener('pointerup', () => { if (active && !fired) { fn(); buzz(); } stop(); });
    ['pointercancel', 'pointerleave'].forEach((n) => el.addEventListener(n, stop));
  }

  // ============================================================ keys + sticky modifiers
  const mods = new Set();
  function paintMods() { $$('[data-mod]').forEach((b) => { const on = mods.has(b.dataset.mod); b.classList.toggle('on', on); b.setAttribute('aria-pressed', on); }); }
  const sendKey = (k) => { send({ t: 'key', k: [...mods, k].join('+') }); if (mods.size) { mods.clear(); paintMods(); } if (/^media_/.test(k)) setTimeout(askMedia, 400); };
  const typed = (s) => { if (mods.size && s.length === 1) sendKey(s.toLowerCase()); else send({ t: 'text', v: s }); };
  $$('[data-mod]').forEach((b) => { b.onclick = () => { const m = b.dataset.mod; mods.has(m) ? mods.delete(m) : mods.add(m); paintMods(); buzz(); }; });
  $$('[data-key]').forEach((b) => {
    const label = $('span', b);
    const orig = label && label.textContent;
    let armT = 0;
    const fire = () => {
      if (b.dataset.confirm) {
        if (!b.classList.contains('arm')) {
          b.classList.add('arm'); if (label) label.textContent = b.dataset.confirm;
          armT = setTimeout(() => { b.classList.remove('arm'); if (label) label.textContent = orig; }, 2500);
          return;
        }
        clearTimeout(armT); b.classList.remove('arm'); if (label) label.textContent = orig;
      }
      sendKey(b.dataset.key);
    };
    bindPress(b, fire, { repeat: b.hasAttribute('data-repeat') });
  });
  $$('[data-t]').forEach((b) => bindPress(b, () => typed(b.dataset.t)));
  const frow = $('#frow');
  [...Array(12).keys()].map((i) => 'f' + (i + 1)).concat(['printscreen', 'capslock', 'numlock', 'pause']).forEach((k) => {
    const b = document.createElement('button'); b.className = 'key fn';
    b.textContent = k.length <= 3 ? k.toUpperCase() : { printscreen: 'PrtSc', capslock: 'Caps', numlock: 'Num', pause: 'Pause' }[k];
    bindPress(b, () => sendKey(k)); frow.appendChild(b);
  });

  // ============================================================ mouse output (batched, one message per frame)
  // Raw finger movement (rx, ry) is scaled once per frame, by the speed of the whole vector over the real elapsed
  // time. (Scaling every touch event by its own size made the gain jump whenever the browser coalesced or split
  // events, and scaling each axis alone bent diagonal strokes.) Whole pixels go out; the remainder carries over.
  let rx = 0, ry = 0, ax = 0, ay = 0, sx = 0, sy = 0, queued = false, lastFlush = 0;
  function flush() {
    queued = false;
    if (ws && ws.readyState === 1 && ws.bufferedAmount > 256) { queue(); return; }   // link is backed up: keep summing instead of piling up stale moves
    const now = performance.now(), idle = now - lastFlush > 100;
    const dt = idle ? 16 : Math.min(50, Math.max(4, now - lastFlush));
    lastFlush = now;
    if (rx || ry) {
      const per16 = Math.hypot(rx, ry) * 16 / dt;   // finger speed in px per 16 ms
      const g = (S.speed / 10) * (1 + Math.min(per16, 30) / 15);
      ax += rx * g; ay += ry * g; rx = ry = 0;
    }
    const ix = Math.trunc(ax), iy = Math.trunc(ay);
    if (ix || iy) { ax -= ix; ay -= iy; send({ t: 'move', dx: ix, dy: iy }); }
    const jx = Math.trunc(sx), jy = Math.trunc(sy);
    if (jx || jy) { sx -= jx; sy -= jy; send({ t: 'scroll', dx: jx, dy: jy }); }
  }
  const queue = () => { if (!queued) { queued = true; requestAnimationFrame(flush); } };

  // ============================================================ touchpad
  function gchip(box, icon, text) {
    const c = $('.gchip', box); if (!c) return;
    if (c._txt !== text) { setIcon($('svg', c), icon); $('span', c).textContent = text; c._txt = text; }
    c.classList.add('show'); clearTimeout(c._t); c._t = setTimeout(() => { c.classList.remove('show'); c._txt = null; }, 900);
  }
  function attachPad(pad) {
    const box = pad.parentElement;
    let g = null, lastTap = 0;
    const centroid = (ts) => { let x = 0, y = 0; for (const t of ts) { x += t.clientX; y += t.clientY; } return { x: x / ts.length, y: y / ts.length }; };
    const ripple = (clientX, clientY) => {
      const r = box.getBoundingClientRect(), s = document.createElement('span');
      s.className = 'ripple'; s.style.left = clientX - r.left + 'px'; s.style.top = clientY - r.top + 'px';
      box.appendChild(s); s.addEventListener('animationend', () => s.remove());
    };
    pad.addEventListener('touchstart', (e) => {
      e.preventDefault();
      const now = performance.now();
      if (!g) {
        g = { t0: now, moved: 0, max: 0, drag: false, x: e.touches[0].clientX, y: e.touches[0].clientY };
        if (e.touches.length === 1 && now - lastTap < 320) { g.drag = true; send({ t: 'btn', b: 'left', s: 'down' }); pad.classList.add('hold'); gchip(box, 'move', 'Dragging'); buzz(); }
      }
      g.max = Math.max(g.max, e.touches.length);
      g.last = centroid(e.touches);
    }, { passive: false });
    pad.addEventListener('touchmove', (e) => {
      e.preventDefault();
      if (!g) return;
      const c = centroid(e.touches), dx = c.x - g.last.x, dy = c.y - g.last.y;
      g.last = c; g.moved += Math.abs(dx) + Math.abs(dy);
      if (e.touches.length >= 2) { const dir = S.natural ? -1 : 1; sy += dir * dy * S.scroll; sx += dir * dx * S.scroll; if (g.moved > 10) gchip(box, 'mouse', 'Scrolling'); }
      else { rx += dx; ry += dy; }
      queue();
    }, { passive: false });
    pad.addEventListener('touchend', (e) => {
      e.preventDefault();
      if (!g) return;
      if (e.touches.length) { g.last = centroid(e.touches); return; }
      const now = performance.now(), tap = g.moved < 10 && now - g.t0 < 250;
      if (g.drag) { send({ t: 'btn', b: 'left', s: 'up' }); pad.classList.remove('hold'); lastTap = 0; }
      else if (tap) {
        const right = g.max >= 2;
        send({ t: 'btn', b: right ? 'right' : 'left', s: 'click' }); buzz();
        const t = e.changedTouches[0]; ripple(t ? t.clientX : g.x, t ? t.clientY : g.y);
        if (right) gchip(box, 'hand', 'Right click');
        lastTap = right ? 0 : now;
      }
      g = null;
    }, { passive: false });
    pad.addEventListener('touchcancel', () => { if (g && g.drag) send({ t: 'btn', b: 'left', s: 'up' }); pad.classList.remove('hold'); g = null; });
    // desktop-browser fallback
    pad.addEventListener('mousemove', (e) => { if (e.buttons === 1) { rx += e.movementX; ry += e.movementY; queue(); } });
    pad.addEventListener('mousedown', (e) => { if (e.button === 0) pad._md = performance.now(); });
    pad.addEventListener('mouseup', (e) => { if (e.button === 0 && pad._md && performance.now() - pad._md < 200) { send({ t: 'btn', b: 'left', s: 'click' }); ripple(e.clientX, e.clientY); } });
    pad.addEventListener('wheel', (e) => { e.preventDefault(); sy += e.deltaY; sx += e.deltaX; queue(); }, { passive: false });
  }
  attachPad($('#pad')); attachPad($('#pad2'));

  // scroll strip: drag = scroll like a scrollbar; tap an end arrow = one notch
  (function strip() {
    const el = $('#strip'); let sg = null;
    el.addEventListener('pointerdown', (e) => { el.setPointerCapture(e.pointerId); el.classList.add('drag'); sg = { y: e.clientY, moved: 0 }; });
    el.addEventListener('pointermove', (e) => { if (!sg) return; const dy = e.clientY - sg.y; sg.y = e.clientY; sg.moved += Math.abs(dy); sy += dy * S.scroll * 1.5; queue(); });
    const end = (e) => {
      if (!sg) return;
      if (sg.moved < 6) { const r = el.getBoundingClientRect(); send({ t: 'scroll', dy: e.clientY < r.top + r.height / 2 ? -120 : 120 }); buzz(); }
      sg = null; el.classList.remove('drag');
    };
    el.addEventListener('pointerup', end);
    el.addEventListener('pointercancel', () => { sg = null; el.classList.remove('drag'); });
  })();

  // mouse buttons: pressed while held
  $$('[data-btn]').forEach((b) => {
    const up = () => { if (b.classList.contains('down')) { b.classList.remove('down'); send({ t: 'btn', b: b.dataset.btn, s: 'up' }); } };
    b.addEventListener('pointerdown', (e) => { b.setPointerCapture(e.pointerId); b.classList.add('down'); send({ t: 'btn', b: b.dataset.btn, s: 'down' }); buzz(); });
    b.addEventListener('pointerup', up); b.addEventListener('pointercancel', up);
  });

  // coach card
  const coach = $('#coach');
  const showCoach = () => coach.classList.add('show');
  $('#coach-ok').onclick = () => { coach.classList.remove('show'); store.set('or_seen_hint', true); };
  $('#help').onclick = () => { go('pad'); showCoach(); };
  $('#showhelp').onclick = () => { closeSheet(); go('pad'); showCoach(); };

  // ============================================================ on-screen keyboard
  const LAYOUTS = {
    abc: [
      '1234567890'.split(''), 'qwertyuiop'.split(''), 'asdfghjkl'.split(''), ['shift', ...'zxcvbnm'.split(''), 'bksp'],
    ],
    sym: [
      '1234567890'.split(''), ['-', '/', ':', ';', '(', ')', '$', '&', '@', '"'], ['[', ']', '{', '}', '#', '%', '^', '*', '+', '='],
      ['_', '\\', '|', '~', '<', '>', '.', ',', 'bksp'],
    ],
  };
  let layout = 'abc', shift = false, caps = false, lastShift = 0;
  const kbdEl = $('#kbd');
  let pop = null;
  function buildKbd() {
    kbdEl.textContent = '';
    const rows = LAYOUTS[layout].concat([[layout === 'abc' ? 'sym' : 'abc', 'sys', 'space', 'enter']]);
    rows.forEach((row, ri) => {
      const r = document.createElement('div'); r.className = 'kr';
      if (ri === 2 && layout === 'abc') r.style.padding = '0 5%';
      row.forEach((k) => {
        const b = document.createElement('button'); b.className = 'key'; b.dataset.k = k;
        if (ri === 0) b.classList.add('num');
        if (k === 'shift') { b.classList.add('w15', 'fn'); b.appendChild(svg(caps ? 'arrow-big-up-dash' : 'arrow-big-up', 'size-6')); if (caps) b.classList.add('caps'); else if (shift) b.classList.add('on'); b.setAttribute('aria-label', 'Shift'); }
        else if (k === 'bksp') { b.classList.add('w15', 'fn'); b.appendChild(svg('delete', 'size-6')); b.setAttribute('aria-label', 'Backspace'); }
        else if (k === 'sym' || k === 'abc') { b.classList.add('w14', 'fn'); b.textContent = k === 'sym' ? '?123' : 'ABC'; }
        else if (k === 'sys') { b.classList.add('w14', 'fn'); b.appendChild(svg('globe', 'size-6')); b.setAttribute('aria-label', 'System keyboard'); }
        else if (k === 'space') { b.classList.add('w4'); b.setAttribute('aria-label', 'Space'); }
        else if (k === 'enter') { b.classList.add('w2', 'go'); b.textContent = 'return'; }
        else b.textContent = layout === 'abc' && (shift || caps) ? k.toUpperCase() : k;
        bindPress(b, () => press(k), { down: true, repeat: k === 'bksp' });
        if (k.length === 1) {
          b.addEventListener('pointerdown', () => showPop(b));
          ['pointerup', 'pointercancel', 'pointerleave'].forEach((n) => b.addEventListener(n, hidePop));
        }
        r.appendChild(b);
      });
      kbdEl.appendChild(r);
    });
    pop = document.createElement('div'); pop.className = 'keypop'; kbdEl.appendChild(pop);
  }
  function showPop(b) {
    if (!pop) return;
    const kr = kbdEl.getBoundingClientRect(), r = b.getBoundingClientRect();
    pop.textContent = b.textContent; pop.style.left = r.left - kr.left + r.width / 2 + 'px'; pop.style.top = r.top - kr.top + 'px';
    pop.classList.add('show');
  }
  const hidePop = () => { if (pop) pop.classList.remove('show'); };
  function press(k) {
    if (k === 'shift') { const n = performance.now(); if (n - lastShift < 350) { caps = !caps; shift = caps; } else { shift = !shift; caps = false; } lastShift = n; return buildKbd(); }
    if (k === 'sym' || k === 'abc') { layout = k; return buildKbd(); }
    if (k === 'sys') { $('#sysin').focus(); return; }
    if (k === 'bksp') return sendKey('backspace');
    if (k === 'enter') return sendKey('enter');
    if (k === 'space') return sendKey('space');
    typed(layout === 'abc' && (shift || caps) ? k.toUpperCase() : k);
    if (shift && !caps) { shift = false; buildKbd(); }
  }
  buildKbd();

  // hidden textarea: system keyboard (emoji, other languages, swipe typing)
  (function sysKeyboard() {
    const kb = $('#sysin'); let composing = false;
    const specials = { Backspace: 'backspace', Enter: 'enter', Tab: 'tab', Escape: 'esc', ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down', Delete: 'delete' };
    kb.addEventListener('keydown', (e) => { if (specials[e.key] && !e.isComposing) { e.preventDefault(); sendKey(specials[e.key]); } });
    kb.addEventListener('beforeinput', (e) => {
      if (e.isComposing) return;
      if (e.inputType === 'insertText' && e.data) { e.preventDefault(); typed(e.data); }
      else if (e.inputType === 'deleteContentBackward') { e.preventDefault(); sendKey('backspace'); }
      else if (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph') { e.preventDefault(); sendKey('enter'); }
    });
    kb.addEventListener('compositionstart', () => { composing = true; });
    kb.addEventListener('compositionend', (e) => { composing = false; if (e.data) typed(e.data); kb.value = ''; });
    kb.addEventListener('input', () => { if (!composing) kb.value = ''; });
  })();

  // ============================================================ pairing
  const pairEl = $('#pair');
  const deviceName = () => (/iPhone/.test(navigator.userAgent) ? 'iPhone' : /iPad/.test(navigator.userAgent) ? 'iPad' : /Android/.test(navigator.userAgent) ? 'Android phone' : 'Browser');
  async function pairStatus() {
    try { const r = await fetch('/pair/status', { cache: 'no-store' }); return await r.json(); } catch { return null; }
  }
  function paintPairing(s) { $('#pairrow').hidden = !(s && s.required && s.device); }
  function showPair(msg) { $('#pairmsg').textContent = msg || ''; pairEl.hidden = false; setTimeout(() => $('#paircode').focus(), 50); }
  async function doPair(proof) {
    $('#pairmsg').textContent = 'Pairing...';
    try {
      const r = await fetch('/pair', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...proof, name: deviceName() }) });
      if (r.ok) { pairEl.hidden = true; $('#paircode').value = ''; paintPairing(await pairStatus()); connect(); return true; }
      $('#pairmsg').textContent = r.status === 429 ? 'Too many wrong tries. Wait a few minutes, then ask the PC for a new code.' : 'That code is wrong or has expired. Check the PC for a fresh one.';
    } catch { $('#pairmsg').textContent = "Can't reach the PC. Is it running, on the same Wi-Fi?"; }
    return false;
  }
  $('#pairform').onsubmit = (e) => { e.preventDefault(); const c = $('#paircode').value.replace(/\D/g, ''); if (c.length !== 6) { $('#pairmsg').textContent = 'The code has 6 digits.'; return; } doPair({ code: c }); };
  $('#unpair').onclick = async () => {
    try { await fetch('/pair/forget', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }); } catch {}
    if (ws) { ws.onclose = null; try { ws.close(); } catch {} ws = null; } clearTimeout(timer);
    closeSheet(); paintPairing(null); showPair('This phone was unpaired.');
  };
  async function boot() {
    const qp = new URLSearchParams(location.search).get('pair');
    if (qp) { history.replaceState(null, '', location.pathname); if (await doPair({ secret: qp })) return; }
    const s = await pairStatus();
    paintPairing(s);
    if (s && s.required && !s.paired) return showPair();
    connect();
  }

  // ============================================================ apps: windows + launcher
  let appsMode = 'open', winItems = [], appItems = [], winLoaded = false, appsLoaded = false, appsRequested = false;
  const iconUrl = (key) => {
    return key ? '/icon/' + key + '.png' : '';
  };
  function iconBox(key) {
    const s = document.createElement('span'); s.className = 'aic';
    s.appendChild(svg('app-window', 'fb'));
    const u = iconUrl(key);
    if (u) {
      const img = document.createElement('img'); img.alt = ''; img.decoding = 'async'; img.loading = 'lazy';
      img.onload = () => img.classList.add('ld'); img.onerror = () => s.classList.add('fail'); img.src = u;
      s.prepend(img);
    } else s.classList.add('fail');
    return s;
  }
  function refreshApps() {
    if (appsMode === 'open') send({ t: 'win.list' });
    else if (!appsRequested) { appsRequested = true; send({ t: 'app.list' }); }
  }

  // segmented control with a sliding thumb
  function seg(el, onChange) {
    const btns = $$('button', el);
    const set = (v, fire) => { btns.forEach((b) => b.setAttribute('aria-pressed', b.dataset.v === v)); el.dispatchEvent(new Event('sync')); if (fire) onChange(v); };   // 'sync' moves the kit's sliding thumb
    btns.forEach((b) => { b.onclick = () => set(b.dataset.v, true); });
    return set;
  }
  const appsSeg = seg($('#apps-seg'), (v) => setAppsMode(v));
  function setAppsMode(mode) {
    appsMode = mode;
    $('#winlist').hidden = mode !== 'open'; $('#applist').hidden = mode !== 'all'; $('#searchbox').hidden = mode !== 'all';
    $('#appsbody').scrollTop = 0;
    if (mode === 'open') renderWins(); else renderApps();
    refreshApps();
  }

  // --- windows: keyed update so the 3 s refresh never flickers or resets scroll
  const winEls = new Map();
  function buildWin() {
    const el = document.createElement('div'); el.className = 'wcard'; el.setAttribute('role', 'button'); el.tabIndex = 0;
    const ti = document.createElement('div'); ti.className = 'ti';
    const b = document.createElement('b');
    const sm = document.createElement('small'); const exe = document.createElement('span'); const tag = document.createElement('span'); tag.className = 'tag'; tag.textContent = 'Active';
    sm.append(exe, tag); ti.append(b, sm);
    const acts = document.createElement('div'); acts.className = 'acts';
    const mk = (a, icon, label, cls) => { const x = document.createElement('button'); x.className = 'btn btn-icon btn-gray' + (cls ? ' ' + cls : ''); x.dataset.a = a; x.setAttribute('aria-label', label); x.appendChild(svg(icon, 'size-4')); return x; };
    const min = mk('min', 'minus', 'Minimize'), max = mk('max', 'maximize-2', 'Maximize'), close = mk('close', 'x', 'Close', 'danger');
    acts.append(min, max, close);
    el.append(document.createElement('span'), ti, acts);
    el._r = { b, exe, max, ti };
    return el;
  }
  function updateWin(el, w) {
    const r = el._r;
    el.dataset.id = w.id;
    r.b.textContent = w.title;                                   // titles are untrusted: text only
    r.exe.textContent = (w.exe || 'window') + (w.minimized ? ' · minimized' : '');
    el.classList.toggle('fg', !!w.foreground); el.classList.toggle('min', !!w.minimized);
    if (el._key !== w.icon) { const nb = iconBox(w.icon); el.replaceChild(nb, el.firstChild); el._key = w.icon; }
    const big = !!(w.minimized || w.maximized);   // the button toggles: maximize, then back to the normal size
    r.max.dataset.a = big ? 'restore' : 'max';
    setIcon($('svg', r.max), big ? 'minimize-2' : 'maximize-2'); r.max.setAttribute('aria-label', big ? 'Restore' : 'Maximize');
  }
  function renderWins() {
    const box = $('#winlist');
    if (!winLoaded) { if (!box.firstChild) for (let i = 0; i < 4; i++) { const s = document.createElement('div'); s.className = 'skel h-16 rounded-card'; box.appendChild(s); } return; }
    $$('.skel, .empty', box).forEach((n) => n.remove());
    const seen = new Set(); let idx = 0;
    winItems.forEach((w) => {
      let el = winEls.get(w.id);
      if (!el) { el = buildWin(); el.classList.add('new'); el.style.setProperty('--n', Math.min(idx, 8)); el.addEventListener('animationend', () => el.classList.remove('new'), { once: true }); winEls.set(w.id, el); }
      updateWin(el, w);
      if (box.children[idx] !== el) box.insertBefore(el, box.children[idx] || null);
      seen.add(w.id); idx++;
    });
    for (const [id, el] of winEls) if (!seen.has(id)) { el.classList.add('leave'); setTimeout(() => el.remove(), 160); winEls.delete(id); }
    if (!winItems.length) {
      const e = document.createElement('div'); e.className = 'empty'; e.appendChild(svg('app-window', 'size-9'));
      const t = document.createElement('div'); t.textContent = 'No open windows found'; e.appendChild(t); box.appendChild(e);
    }
  }
  $('#winlist').onclick = (e) => {
    const card = e.target.closest('.wcard'); if (!card) return;
    const act = e.target.closest('button[data-a]');
    send({ t: 'win.act', id: Number(card.dataset.id), a: act ? act.dataset.a : 'focus' });
    buzz(); setTimeout(() => send({ t: 'win.list' }), 450);
  };

  // --- launcher
  const recent = () => store.get('or_recent', []);
  function appBtn(a) {
    const b = document.createElement('button'); b.className = 'app'; b.dataset.id = a.id; b.dataset.name = a.name;
    const n = document.createElement('span'); n.className = 'n'; n.textContent = a.name;
    b.append(iconBox(a.icon), n);
    return b;
  }
  function renderApps() {
    const box = $('#applist'); box.textContent = '';
    if (!appsLoaded) {
      const g = document.createElement('div'); g.className = 'agrid';
      for (let i = 0; i < 12; i++) { const s = document.createElement('div'); s.className = 'skel sq'; g.appendChild(s); }
      box.appendChild(g); return;
    }
    const q = $('#appsearch').value.trim().toLowerCase();
    const list = appItems.filter((a) => !q || a.name.toLowerCase().includes(q));
    if (!list.length) {
      const e = document.createElement('div'); e.className = 'empty'; e.appendChild(svg('search', 'size-9'));
      const t = document.createElement('div'); t.textContent = q ? 'No apps match "' + $('#appsearch').value.trim() + '"' : 'No apps found'; e.appendChild(t); box.appendChild(e); return;
    }
    const head = (t) => { const h = document.createElement('div'); h.className = 'sec'; h.textContent = t; return h; };
    if (q) { const g = document.createElement('div'); g.className = 'agrid'; list.forEach((a) => g.appendChild(appBtn(a))); box.appendChild(g); return; }
    const rec = recent().map((id) => appItems.find((a) => a.id === id)).filter(Boolean).slice(0, 10);
    if (rec.length) { box.appendChild(head('RECENT')); const r = document.createElement('div'); r.className = 'hrow'; rec.forEach((a) => r.appendChild(appBtn(a))); box.appendChild(r); }
    let letter = '', grid = null;
    list.forEach((a) => {
      const ch = a.name.trim().charAt(0).toUpperCase(), L = /[A-Z]/.test(ch) ? ch : '#';
      if (L !== letter) { letter = L; box.appendChild(head(L)); grid = document.createElement('div'); grid.className = 'agrid'; box.appendChild(grid); }
      grid.appendChild(appBtn(a));
    });
  }
  $('#applist').onclick = (e) => {
    const b = e.target.closest('.app'); if (!b || b.classList.contains('busy')) return;
    send({ t: 'app.launch', id: b.dataset.id }); buzz();
    store.set('or_recent', [b.dataset.id, ...recent().filter((x) => x !== b.dataset.id)].slice(0, 12));
    b.classList.add('busy'); toast('Launching ' + b.dataset.name, { icon: 'circle-check' });
    setTimeout(() => b.classList.remove('busy'), 1300);
    setTimeout(() => { if (page === 'apps' && appsMode === 'all') appsSeg('open', true); }, 1500);
  };
  const syncClear = () => { $('#searchclear').hidden = !$('#appsearch').value; };
  $('#appsearch').oninput = () => { syncClear(); renderApps(); };
  $('#searchclear').onclick = () => { $('#appsearch').value = ''; syncClear(); renderApps(); $('#appsearch').focus(); };
  syncClear();
  setInterval(() => { if (page === 'apps' && appsMode === 'open' && online && !document.hidden) send({ t: 'win.list' }); }, 3000);
  renderWins();

  // ============================================================ media: now playing
  // While the Media page is open, ask the PC what is playing every 2 s; the progress bar advances locally in between.
  let np = null, npAt = 0, npKey = '', npPoll = 0, npTick = 0;
  const fmtT = (s) => Math.floor(s / 60) + ':' + String(Math.floor(s % 60)).padStart(2, '0');
  function askMedia() { if (online && page === 'media') send({ t: 'media.get' }); }
  function startMedia() { stopMedia(); askMedia(); paintMedia(); npPoll = setInterval(askMedia, 2000); npTick = setInterval(paintMedia, 500); }
  function stopMedia() { clearInterval(npPoll); clearInterval(npTick); }
  function gotMedia(m) { np = m.state === 'none' ? null : m; npAt = performance.now(); paintMedia(); }
  const npImg = $('#np-img');
  npImg.onload = () => { npImg.hidden = false; };
  npImg.onerror = () => { npImg.hidden = true; };
  function paintMedia() {
    const has = !!np && !!(np.title || np.artist);
    const playing = has && np.state === 'playing';
    $('#np-title').textContent = has ? np.title || 'Unknown title' : 'Nothing playing';
    $('#np-artist').textContent = has ? np.artist || np.app : 'Start something on your PC';
    $('#np-app').textContent = has && np.artist ? np.app : '';
    $('#np').classList.toggle('idle', !has);
    $('#pp use').setAttribute('href', playing ? '#pause' : '#play');
    const key = has ? [np.title, np.artist, np.app, np.art].join('|') : '';
    if (key !== npKey) {   // a new track: reload the cover
      npKey = key; npImg.hidden = true;
      if (has && np.art) npImg.src = '/media/art?v=' + Date.now(); else npImg.removeAttribute('src');
    }
    const show = has && np.dur > 0;
    $('#np-prog').hidden = !show;
    if (show) {
      const pos = Math.min(np.dur, np.pos + (playing ? (performance.now() - npAt) / 1000 : 0));
      $('#np-pos').textContent = fmtT(pos); $('#np-dur').textContent = fmtT(np.dur);
      $('#np-fill').style.width = (pos / np.dur * 100).toFixed(2) + '%';
    }
  }

  // ============================================================ browser / clipboard / power
  $('#urlform').onsubmit = (e) => {
    e.preventDefault();
    let u = $('#urlin').value.trim(); if (!u) return;
    if (!/^https?:\/\//i.test(u)) u = 'https://' + u;
    send({ t: 'open', url: u }); $('#urlin').value = ''; toast('Opening on the computer', { icon: 'external-link' }); buzz();
  };
  // ---- clipboard: one box to send (text or a photo), one button to fetch
  const clipText = $('#cliptext'), say = (msg, kind) => { const el = $('#clipmsg'); el.textContent = msg; el.classList.toggle('bad', kind === 'bad'); el.classList.toggle('ok', kind === 'ok'); };
  const fmtBytes = (b) => (b > 1048576 ? (b / 1048576).toFixed(1) + ' MB' : Math.max(1, Math.round(b / 1024)) + ' KB');
  const MAX_SIDE_PX = 2560, MAX_UPLOAD = 24 * 1024 * 1024;
  let photo = null, prevUrl = '';   // the PNG waiting to be sent

  function setPhoto(blob, w, h) {
    if (prevUrl) URL.revokeObjectURL(prevUrl);
    photo = blob; prevUrl = blob ? URL.createObjectURL(blob) : '';
    $('#attach').hidden = !blob;
    if (blob) { $('#imgprev').src = prevUrl; $('#attachinfo').textContent = 'Photo ' + w + ' x ' + h + ' (' + fmtBytes(blob.size) + ')'; }
  }
  async function loadBitmap(file) {
    if (window.createImageBitmap) { try { return await createImageBitmap(file); } catch {} }
    return new Promise((res, rej) => {
      const u = URL.createObjectURL(file), im = new Image();
      im.onload = () => { URL.revokeObjectURL(u); res(im); };
      im.onerror = () => rej(new Error('This image format is not supported'));
      im.src = u;
    });
  }
  // Any photo (JPEG/HEIC from the library, screenshots, ...) becomes a PNG the PC can use, scaled down if needed.
  async function toPng(file) {
    const bmp = await loadBitmap(file);
    let scale = Math.min(1, MAX_SIDE_PX / Math.max(bmp.width, bmp.height));
    for (let i = 0; i < 4; i++) {
      const cv = document.createElement('canvas');
      cv.width = Math.max(1, Math.round(bmp.width * scale)); cv.height = Math.max(1, Math.round(bmp.height * scale));
      cv.getContext('2d').drawImage(bmp, 0, 0, cv.width, cv.height);
      const blob = await new Promise((r) => cv.toBlob(r, 'image/png'));
      if (!blob) throw new Error('This image format is not supported');
      if (blob.size <= MAX_UPLOAD) return { blob, w: cv.width, h: cv.height };
      scale *= 0.7;
    }
    throw new Error('The image is too large to send');
  }
  async function attach(file) {
    say('Preparing photo...');
    try { const r = await toPng(file); setPhoto(r.blob, r.w, r.h); say('Tap Send to copy it to the PC.'); }
    catch (e) { say('Could not use that image: ' + (e && e.message ? e.message : e), 'bad'); }
  }
  $('#imgfile').onchange = (e) => { const f = e.target.files && e.target.files[0]; e.target.value = ''; if (f) attach(f); };
  $('#imgclear').onclick = () => { setPhoto(null); say(''); };
  // pasting an image into the box (long-press, Paste) attaches it; pasting text works as usual
  clipText.addEventListener('paste', (e) => {
    const cd = e.clipboardData; if (!cd) return;
    let f = [...cd.files].find((x) => x.type.startsWith('image/'));
    if (!f) { const it = [...cd.items].find((i) => i.kind === 'file' && i.type.startsWith('image/')); f = it && it.getAsFile(); }
    if (f) { e.preventDefault(); attach(f); }
  });

  $('#clipsend').onclick = async () => {
    if (!online) return say('Not connected to the PC.', 'bad');
    if (photo) {
      say('Sending ' + fmtBytes(photo.size) + '...');
      try {
        const r = await fetch('/clip/image', { method: 'POST', headers: { 'Content-Type': 'image/png' }, body: photo });
        const j = await r.json().catch(() => ({}));
        if (!r.ok || !j.ok) throw new Error(j.error || 'the PC answered ' + r.status);
        say('Sent. The photo is on the PC clipboard.', 'ok'); toast('Photo copied on the PC', { icon: 'circle-check' }); buzz(); setPhoto(null);
      } catch (e) { say('Could not send the photo: ' + (e && e.message ? e.message : e), 'bad'); }
      return;
    }
    if (!clipText.value) return say('Type or paste some text, or add a photo first.', 'bad');
    say('Sending...'); send({ t: 'clip.set', text: clipText.value });
  };
  function gotAck(m) {
    if (m.ok) { say('Sent. The text is on the PC clipboard.', 'ok'); toast('Text copied on the PC', { icon: 'circle-check' }); buzz(); }
    else say('The PC could not set its clipboard (another app may be holding it, or the text is too long). Try again.', 'bad');
  }

  $('#clipget').onclick = () => {
    if (!online) return say('Not connected to the PC.', 'bad');
    say('Asking the PC...'); send({ t: 'clip.get' });
  };
  function gotClip(m) {
    const img = m.image, text = (m.text || '').trim() ? m.text : '';   // whitespace alone is not worth showing
    $('#pctext').textContent = text;
    const im = $('#imgpc');
    im.hidden = !img;
    if (img) im.src = '/clip/image.png?v=' + Date.now();
    $('#pcres').hidden = !text && !img;
    $('#clipcopy').hidden = !text;
    const dims = img ? img.w + ' x ' + img.h + ' px' : '';
    if (text && img) say('The PC copied text and an image (' + dims + '). Press and hold the image to copy or save it.', 'ok');
    else if (text) say('Tap Copy text to put it on your phone.', 'ok');
    else if (img) say('The PC copied an image (' + dims + '). Press and hold it, then choose Copy or Add to Photos.', 'ok');
    else say('The PC clipboard is empty.', 'bad');
  }
  $('#imgpc').onerror = () => say('Could not load the image from the PC.', 'bad');
  // Copy must run inside the tap itself: iOS refuses clipboard writes that happen after a network reply.
  $('#clipcopy').onclick = async () => {
    const text = $('#pctext').textContent;
    const within = (p, ms) => Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error('timeout')), ms))]);
    let ok = false;
    try { if (navigator.clipboard && navigator.clipboard.writeText) { await within(navigator.clipboard.writeText(text), 1500); ok = true; } } catch {}
    if (!ok) {
      const ta = document.createElement('textarea'); ta.value = text; ta.readOnly = true; ta.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0';
      document.body.appendChild(ta); ta.focus(); ta.setSelectionRange(0, text.length);
      try { ok = document.execCommand('copy'); } catch {} ta.remove();
    }
    if (ok) { say('Copied to your phone.', 'ok'); toast('Copied to your phone', { icon: 'circle-check' }); buzz(); }
    else say('Your browser blocked the copy. Press and hold the text and choose Copy.', 'bad');
  };
  $$('[data-power]').forEach((t) => {
    const small = $('small', t), orig = small.textContent; let armT = 0;
    t.onclick = () => {
      const a = t.dataset.power;
      if (a === 'lock' || t.classList.contains('arm')) {
        clearTimeout(armT); t.classList.remove('arm'); small.textContent = orig;
        send({ t: 'power', a }); toast($('b', t).textContent + ' sent', { icon: 'circle-check' }); buzz(); closeSheet(); return;
      }
      t.classList.add('arm'); small.textContent = 'Tap again to confirm';
      armT = setTimeout(() => { t.classList.remove('arm'); small.textContent = orig; }, 3000);
    };
  });

  // ============================================================ bottom sheet
  // A native <dialog class="sheet">: the kit (kit/ui.js) handles the backdrop tap and the drag-down dismiss.
  const sheet = $('#sheet');
  const TITLES = { more: 'More', browser: 'Browser', keys: 'Special keys', clip: 'Clipboard', power: 'Power', settings: 'Settings' };
  let sheetOpen = false, stack = ['more'];
  function showView(id) {
    $$('.sh-view').forEach((v) => v.classList.toggle('on', v.id === 'v-' + id));
    $('#sh-title').textContent = TITLES[id]; sheet.classList.toggle('deep', stack.length > 1);
    sheet.scrollTop = 0;
  }
  function openSheet(id = 'more') {
    stack = id === 'more' ? ['more'] : ['more', id];
    showView(id); sheetOpen = true; if (!sheet.open) sheet.showModal(); paintNav();
  }
  function closeSheet() { if (sheet.open) sheet.close(); }
  sheet.addEventListener('close', () => { sheetOpen = false; paintNav(); });   // also fires for Escape, backdrop tap and drag-down
  $$('[data-view]').forEach((t) => { t.onclick = () => { stack.push(t.dataset.view); showView(t.dataset.view); }; });
  $('#sh-back').onclick = () => { if (stack.length > 1) { stack.pop(); showView(stack[stack.length - 1]); } };
  $('#sh-close').onclick = closeSheet;

  // ============================================================ pages + bottom nav
  const PAGES = ['pad', 'type', 'apps', 'media'];
  let page = store.get('or_page', 'pad'); if (!PAGES.includes(page)) page = 'pad';
  function paintNav() {
    $$('#nav [data-go]').forEach((b) => b.setAttribute('aria-selected', !sheetOpen && b.dataset.go === page));
    $('#nav [data-more]').setAttribute('aria-selected', sheetOpen);
  }
  function go(name) {
    const idx = PAGES.indexOf(name);
    PAGES.forEach((p, i) => { const el = $('#pg-' + p); el.classList.toggle('on', p === name); el.style.setProperty('--off', i < idx ? '-28px' : i > idx ? '28px' : '0px'); });
    page = name; store.set('or_page', name); paintNav();
    if (name === 'apps') refreshApps();
    if (name === 'media') startMedia(); else stopMedia();
  }
  $$('#nav [data-go]').forEach((b) => { b.onclick = () => { closeSheet(); go(b.dataset.go); buzz(); }; });
  $('#nav [data-more]').onclick = () => { sheetOpen ? closeSheet() : openSheet('more'); buzz(); };

  // ============================================================ start
  paintSettings(); paintMods();
  go(page);
  boot();
  if (!store.get('or_seen_hint', false) && page === 'pad') setTimeout(showCoach, 700);
})();
