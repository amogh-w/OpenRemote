/* remote-client.js - the phone's one WebSocket link to the PC. Shared by OpenRemote and Kai Remote:
   edit it in the cuppa plugin repo (shared/) and run harness.mjs sync --write, never in a project copy.

   const link = RemoteLink({
     url: () => "ws://host/ws",          // string or function, read on every attempt
     onOpen() {}, onMessage(msg) {},     // msg is parsed JSON; anything that is not JSON is dropped
     onState(state, { retry }) {},       // "connecting" | "open" | "down" (down after `downAfter` failed attempts)
     onClose({ retry, wasOpen }) {},     // after every close, before onState; call link.stop() here to stop reconnecting
     pingMs: 4000, ping: () => ({ t: "ping" }),   // optional app-level ping, sent while the page is visible
     staleMs: 12000,                     // optional: no message at all for this long means a dead link, reconnect
   });
   link.start(); link.send(obj); link.stop(); link.reconnectNow(); link.isOpen(); link.socket

   Guarantees: at most one socket exists; reconnects use capped, jittered backoff; coming back to the page or the
   network reconnects at once; a socket that went silent (phone slept, Wi-Fi switched) is dropped, not trusted. */
(function (root) {
  "use strict";

  function RemoteLink(options) {
    const o = Object.assign({ min: 400, max: 3000, downAfter: 3, pingMs: 0, staleMs: 0 }, options);
    const url = () => (typeof o.url === "function" ? o.url() : o.url);
    let ws = null;
    let retry = 0;
    let timer = null;
    let beat = null;
    let running = false;
    let wasOpen = false;
    let lastRx = 0;

    const state = (s) => o.onState && o.onState(s, { retry });
    const hidden = () => typeof document !== "undefined" && document.hidden;

    function detach() {
      if (!ws) return;
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      try {
        ws.close();
      } catch (e) {
        /* already closed */
      }
      ws = null;
    }

    function connect() {
      clearTimeout(timer);
      timer = null;
      detach();
      if (!running) return;
      wasOpen = false;
      state(retry >= o.downAfter ? "down" : "connecting");
      const sock = new (o.WebSocket || root.WebSocket)(url());
      ws = sock;
      sock.onopen = () => {
        if (ws !== sock) return;
        wasOpen = true;
        retry = 0;
        lastRx = Date.now();
        state("open");
        if (o.onOpen) o.onOpen();
      };
      sock.onmessage = (ev) => {
        if (ws !== sock) return;
        lastRx = Date.now();
        let msg;
        try {
          msg = JSON.parse(ev.data);
        } catch (e) {
          return;
        }
        if (o.onMessage) o.onMessage(msg);
      };
      sock.onclose = () => {
        if (ws !== sock) return;
        ws = null;
        const info = { retry: ++retry, wasOpen };
        wasOpen = false;
        if (o.onClose) o.onClose(info); // before the state changes, so the page can tell "was connected" from "still trying"
        if (!running || timer) return;
        state(retry >= o.downAfter ? "down" : "connecting");
        const base = Math.min(o.max, o.min * retry);
        timer = setTimeout(connect, Math.round(base * (0.8 + Math.random() * 0.4)));
      };
    }

    // Ping while the page is visible, and drop a link that has gone silent. Hidden pages are left alone: browsers
    // throttle their timers, and the page checks the link again the moment it is shown.
    function heartbeat() {
      if (!ws || ws.readyState !== 1 || hidden()) return;
      if (o.staleMs && Date.now() - lastRx > o.staleMs) return dropAndRetry();
      if (o.ping) {
        const p = o.ping();
        if (p) link.send(p);
      }
    }

    function dropAndRetry() {
      const sock = ws;
      if (!sock) return connect();
      sock.onclose(); // schedules the reconnect with backoff, exactly like a real close
      try {
        sock.close();
      } catch (e) {
        /* already closed */
      }
    }

    function wake() {
      if (!running || hidden()) return;
      if (!ws || ws.readyState > 1) return connect();
      if (ws.readyState === 1 && o.staleMs && Date.now() - lastRx > o.staleMs) connect();
    }

    const link = {
      start() {
        if (running) return;
        running = true;
        retry = 0;
        connect();
        if (o.pingMs || o.staleMs) beat = setInterval(heartbeat, o.pingMs || Math.max(1000, o.staleMs / 3));
        if (typeof document !== "undefined") document.addEventListener("visibilitychange", wake);
        if (typeof root.addEventListener === "function") {
          root.addEventListener("online", wake);
          root.addEventListener("pageshow", wake);
        }
      },
      stop() {
        running = false;
        clearTimeout(timer);
        timer = null;
        clearInterval(beat);
        beat = null;
        detach();
        if (typeof document !== "undefined") document.removeEventListener("visibilitychange", wake);
        if (typeof root.removeEventListener === "function") {
          root.removeEventListener("online", wake);
          root.removeEventListener("pageshow", wake);
        }
      },
      /** Connect now instead of waiting for the backoff timer (also restarts a stopped link). */
      reconnectNow() {
        if (!running) return link.start();
        retry = 0;
        connect();
      },
      send(obj) {
        if (!ws || ws.readyState !== 1) return false;
        ws.send(JSON.stringify(obj));
        return true;
      },
      isOpen: () => !!ws && ws.readyState === 1,
      get socket() {
        return ws;
      },
    };
    return link;
  }

  if (typeof module !== "undefined" && module.exports) module.exports = { RemoteLink };
  else root.RemoteLink = RemoteLink;
})(typeof window !== "undefined" ? window : globalThis);
