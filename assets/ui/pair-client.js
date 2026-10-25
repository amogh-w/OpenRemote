// The phone's side of pairing: the code screen, the `?pair=` link and unpairing. Shared by OpenRemote and Kai Remote:
// edit it in the cuppa plugin repo (shared/) and run harness.mjs sync --write, never in a project copy.
// It drives the markup in pair-screen.html (#pair, #pairform, #paircode, #pairmsg) and the routes in pairroutes.rs.
//
//   PairClient.start()          -> Promise<boolean>: true when this phone may connect now (otherwise the screen is up)
//   PairClient.status()         -> { required, paired, device, paused } or null when the PC cannot be reached
//   PairClient.unpair()         remove this phone and show the screen again
//   PairClient.onPaired = fn    called after a successful pairing (connect the app there)
const PairClient = (() => {
  const $ = (id) => document.getElementById(id);
  const say = (msg) => { $('pairmsg').textContent = msg || ''; };
  const deviceName = () => {
    const ua = navigator.userAgent;
    return /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android phone' : 'Browser';
  };

  async function status() {
    try {
      const r = await fetch('/pair/status', { cache: 'no-store' });
      return await r.json();
    } catch { return null; }
  }

  function show(msg) {
    say(msg);
    $('pair').hidden = false;
    setTimeout(() => $('paircode').focus(), 50);
  }

  async function pair(proof) {
    say('Pairing...');
    try {
      const r = await fetch('/pair', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...proof, name: deviceName() }),
      });
      if (r.ok) {
        $('pair').hidden = true;
        $('paircode').value = '';
        say('');
        if (api.onPaired) api.onPaired();
        return true;
      }
      say(
        r.status === 429 ? 'Too many wrong tries. Wait a few minutes, then ask the PC for a new code.'
        : r.status === 403 ? 'Pairing is paused on the PC. Ask for it to be allowed again.'
        : 'That code is wrong or has expired. Check the PC for a fresh one.',
      );
    } catch { say("Can't reach the PC. Is it running, on the same Wi-Fi?"); }
    return false;
  }

  async function unpair() {
    try { await fetch('/pair/forget', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }); } catch {}
    show('This phone was unpaired.');
  }

  async function start() {
    const secret = new URLSearchParams(location.search).get('pair');
    if (secret) {
      history.replaceState(null, '', location.pathname);
      if (await pair({ secret })) return false; // onPaired connects; nothing more to do here
    }
    const s = await status();
    if (s && s.required && !s.paired) { if (!secret) show(); return false; }
    return true;
  }

  $('pairform').onsubmit = (e) => {
    e.preventDefault();
    const code = $('paircode').value.replace(/\D/g, '');
    if (code.length !== 6) { say('The code has 6 digits.'); return; }
    pair({ code });
  };

  const api = { status, show, pair, unpair, start, onPaired: null };
  return api;
})();
