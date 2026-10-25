// The dashboard script. Shared by OpenRemote and Kai Remote: edit it in the cuppa plugin repo (shared/) and run
// harness.mjs sync --write, never in a project copy. Talks to /admin/api/* (see admin.rs).
(() => {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const NS = 'http://www.w3.org/2000/svg';

  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const icon = (name, cls) => {
    const s = document.createElementNS(NS, 'svg');
    s.setAttribute('class', cls || 'size-4');
    s.setAttribute('aria-hidden', 'true');
    const u = document.createElementNS(NS, 'use');
    u.setAttribute('href', '#' + name);
    s.appendChild(u);
    return s;
  };

  // The server only answers the dashboard's own requests: this header is what proves it (see admin.rs).
  async function api(path, body) {
    const res = await fetch('/admin/api/' + path, {
      method: body === undefined ? 'GET' : 'POST',
      headers: body === undefined ? {} : { 'Content-Type': 'application/json', 'X-Admin': '1' },
      body: body === undefined ? undefined : JSON.stringify(body),
      cache: 'no-store',
    });
    let data = {};
    try { data = await res.json(); } catch {}
    if (!res.ok) throw Object.assign(new Error(data.error || 'request failed'), { status: res.status });
    return data;
  }

  const ago = (now, then) => {
    const d = Math.max(0, now - then);
    if (d < 60) return 'just now';
    if (d < 3600) return Math.floor(d / 60) + ' min ago';
    if (d < 86400) return Math.floor(d / 3600) + ' h ago';
    return Math.floor(d / 86400) + ' days ago';
  };
  const grouped = (code) => (code.length === 6 ? code.slice(0, 3) + ' ' + code.slice(3) : code);

  let state = null;
  let offer = null;        // { code, secret, ttl, links, expires, deviceCount }
  let editing = null;      // device id being renamed
  let busy = false;

  // ------------------------------------------------------------ devices
  function renderDevices() {
    const list = $('devices');
    list.replaceChildren();
    const devices = state.devices;
    $('none').hidden = devices.length > 0;
    list.hidden = devices.length === 0;
    $('remove-all').hidden = devices.length < 2;
    $('count').textContent = devices.length ? '(' + devices.length + ')' : '';
    for (const d of devices) {
      const row = el('div', 'row row-plain');
      const tile = el('span', 'row-tile bg-accent');
      tile.appendChild(icon('smartphone'));
      row.appendChild(tile);

      const text = el('div', 'min-w-0 flex-1');
      if (editing === d.id) {
        const input = el('input', 'field h-9');
        input.value = d.name;
        input.maxLength = 40;
        input.setAttribute('aria-label', 'Device name');
        const save = async () => {
          const name = input.value.trim();
          editing = null;
          if (name && name !== d.name) {
            try { await api('rename', { id: d.id, name }); } catch { UI.toast('Could not rename that device.', 'error'); }
          }
          refresh();
        };
        input.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') save();
          if (e.key === 'Escape') { editing = null; renderDevices(); }
        });
        input.addEventListener('blur', save);
        text.appendChild(input);
        setTimeout(() => { input.focus(); input.select(); }, 0);
      } else {
        const name = el('div', 'truncate font-semibold', d.name);
        text.appendChild(name);
      }
      const meta = el('div', 't-foot text-subtext0', 'Added ' + ago(state.now, d.created) + ' · last seen ' + ago(state.now, d.last_seen));
      text.appendChild(meta);
      row.appendChild(text);

      if (d.active) row.appendChild(el('span', 'badge badge-tint', 'Active'));

      const rename = el('button', 'btn btn-gray btn-icon');
      rename.setAttribute('aria-label', 'Rename ' + d.name);
      rename.appendChild(icon('pencil', 'size-4'));
      rename.onclick = () => { editing = d.id; renderDevices(); };
      row.appendChild(rename);

      const remove = el('button', 'btn btn-danger-tinted btn-icon');
      remove.setAttribute('aria-label', 'Remove ' + d.name);
      remove.appendChild(icon('trash-2', 'size-4'));
      remove.onclick = async () => {
        const ok = await UI.confirm({
          icon: 'trash-2', danger: true, title: 'Remove ' + d.name + '?',
          text: 'It stops working right away and has to be paired again.', ok: 'Remove',
        });
        if (!ok) return;
        try { await api('revoke', { id: d.id }); UI.toast('Removed ' + d.name, 'success'); } catch { UI.toast('Could not remove it.', 'error'); }
        refresh();
      };
      row.appendChild(remove);
      list.appendChild(row);
    }
  }

  // ------------------------------------------------------------ the one-time offer
  function renderOffer() {
    const paused = state.paused;
    $('paused-note').hidden = !paused;
    $('new-code').disabled = paused;
    $('offer').hidden = !offer || paused;
    if (!offer || paused) return;
    const left = Math.max(0, Math.round((offer.expires - Date.now()) / 1000));
    $('code').textContent = left > 0 ? grouped(offer.code) : 'Expired';
    $('countdown').textContent = left > 0
      ? 'Works once, for another ' + Math.floor(left / 60) + ':' + String(left % 60).padStart(2, '0')
      : 'Press New code to get a fresh one.';
    const pick = Math.min(offer.pick || 0, Math.max(0, offer.links.length - 1));
    const first = offer.links[pick];
    // more than one network address (Wi-Fi, VPN, virtual switch): let the person pick the one the phone can reach
    const chips = $('addresses');
    chips.hidden = offer.links.length < 2;
    if (offer.links.length > 1 && chips.dataset.for !== offer.secret + ':' + pick) {
      chips.dataset.for = offer.secret + ':' + pick;
      chips.replaceChildren();
      offer.links.forEach((l, i) => {
        const b = el('button', 'chip', l.base.replace('http://', '').replace(/\/$/, ''));
        b.setAttribute('aria-pressed', i === pick);
        b.onclick = () => { offer.pick = i; renderOffer(); };
        chips.appendChild(b);
      });
    }
    $('address').textContent = first ? first.base : 'No network address found. Connect this PC to a network.';
    $('link').value = first ? first.url : '';
    $('qr').hidden = !first;
    if (first && $('qr').dataset.url !== first.url) {
      $('qr').dataset.url = first.url;
      $('qr').src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(first.qr);
    }
    $('qr').style.opacity = left > 0 ? '1' : '0.25';
  }

  async function newOffer() {
    if (busy) return;
    busy = true;
    try {
      const o = await api('offer', { name: $('new-name').value.trim() });
      offer = { ...o, expires: Date.now() + o.ttl * 1000, deviceCount: state ? state.devices.length : 0 };
      renderOffer();
    } catch (e) {
      UI.toast(e.status === 409 ? 'Pairing is paused.' : 'Could not make a code.', 'error');
    } finally { busy = false; }
  }

  // ------------------------------------------------------------ state
  function renderState() {
    $('meta').textContent = 'Version ' + state.version + ' · ' + state.hostname + ' · port ' + state.port;
    $('open-warning').hidden = state.required;
    $('add').hidden = !state.required;
    $('allow').checked = !state.paused;
    $('autostart').checked = state.autostart;
    renderDevices();
    renderOffer();
  }

  async function refresh() {
    try {
      state = await api('state');
      // a phone used the code: say so and clear it
      if (offer && state.devices.length > offer.deviceCount) {
        const added = state.devices[state.devices.length - 1];
        UI.toast((added ? added.name : 'A device') + ' is paired', 'success');
        offer = null;
        $('new-name').value = '';
      }
      if (editing === null || !document.activeElement || document.activeElement.tagName !== 'INPUT') renderState();
      else { $('meta').textContent = 'Version ' + state.version + ' · ' + state.hostname + ' · port ' + state.port; }
    } catch {
      $('meta').textContent = 'Cannot reach the app. Is it running?';
    }
  }

  // ------------------------------------------------------------ actions
  $('new-code').onclick = newOffer;
  $('new-name').addEventListener('keydown', (e) => { if (e.key === 'Enter') newOffer(); });
  $('copy-link').onclick = async () => {
    const link = $('link').value;
    if (!link) return;
    try { await navigator.clipboard.writeText(link); UI.toast('Link copied', 'success'); } catch { $('link').select(); UI.toast('Press Ctrl+C to copy', 'info'); }
  };
  $('remove-all').onclick = async () => {
    const ok = await UI.confirm({
      icon: 'trash-2', danger: true, title: 'Remove all devices?',
      text: 'Every paired device stops working right away and has to be paired again.', ok: 'Remove all',
    });
    if (!ok) return;
    try { await api('revoke-all', {}); UI.toast('All devices removed', 'success'); } catch { UI.toast('Could not remove them.', 'error'); }
    refresh();
  };
  $('allow').onchange = async (e) => {
    try { await api('pairing', { paused: !e.target.checked }); } catch { UI.toast('Could not change that.', 'error'); }
    refresh();
  };
  $('autostart').onchange = async (e) => {
    try { await api('autostart', { on: e.target.checked }); } catch { e.target.checked = !e.target.checked; UI.toast('Could not change startup.', 'error'); }
  };
  $('open-log').onclick = () => api('open-log', {}).catch(() => UI.toast('Could not open the folder.', 'error'));
  $('restart').onclick = async () => {
    const ok = await UI.confirm({
      icon: 'rotate-cw', title: 'Restart ' + (state ? state.name : 'the app') + '?',
      text: 'Phones reconnect by themselves in a few seconds.', ok: 'Restart',
    });
    if (!ok) return;
    try { await api('restart', {}); } catch { UI.toast('Could not restart.', 'error'); return; }
    UI.toast('Restarting...', 'info');
    // wait for the old copy to go, then for the new one to answer
    let gone = false;
    const wait = setInterval(async () => {
      try { await api('state'); if (gone) { clearInterval(wait); location.reload(); } } catch { gone = true; }
    }, 700);
  };

  setInterval(() => { if (offer) renderOffer(); }, 1000);
  setInterval(() => { if (!document.hidden) refresh(); }, 4000);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });

  refresh().then(() => { if (location.hash === '#add' && state && state.required && !state.paused) newOffer(); });
})();
