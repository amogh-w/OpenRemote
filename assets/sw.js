// Service worker: keeps the app shell available when the PC is briefly unreachable.
// Only registered by the page in secure contexts (https or localhost); plain-HTTP LAN pages skip it.
const CACHE = 'or-shell-v1';

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(['/', '/icon-192.png'])).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

// Network first so a server upgrade shows up immediately; the cache is only a fallback.
self.addEventListener('fetch', (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== 'GET' || url.origin !== location.origin) return;
  if (url.pathname === '/ws' || url.pathname === '/admin' || url.pathname.startsWith('/manifest')) return;
  e.respondWith(
    fetch(req)
      .then((res) => {
        if (res.ok && (url.pathname === '/' || url.pathname.startsWith('/icon-'))) {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put(url.pathname === '/' ? '/' : req, copy));
        }
        return res;
      })
      .catch(() => caches.match(url.pathname === '/' ? '/' : req, { ignoreSearch: true }))
  );
});
