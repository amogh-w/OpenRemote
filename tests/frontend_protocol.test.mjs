// node --test "tests/*.test.mjs"    (run by scripts/check.ps1)
// The phone page may only send messages that tests/fixtures/protocol.json lists, with enum values the server knows.
// tests/protocol.rs proves the fixture matches the Rust parser, so together they catch the page and the server
// drifting apart.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const here = (p) => new URL(p, import.meta.url);
const fixture = JSON.parse(readFileSync(here("fixtures/protocol.json"), "utf8"));
const app = readFileSync(here("../assets/ui/app.js"), "utf8");
const body = readFileSync(here("../assets/ui/body.html"), "utf8");
const known = new Set(fixture.valid.map((m) => m.t));
const values = (tag, field) => new Set(fixture.valid.filter((m) => m.t === tag).map((m) => m[field]));

test("every message the page sends is one the server parses", () => {
  // Object literals that start with a `t:` tag: the send() calls and the ping the connection helper builds.
  const sent = new Set([...app.matchAll(/\{\s*t:\s*'([a-z.]+)'/g)].map((m) => m[1]));
  assert.ok(sent.size >= 14, `found only ${sent.size} messages in app.js; did the send() calls move?`);
  for (const name of sent) assert.ok(known.has(name), `app.js sends "${name}" but protocol.json has no such message`);
});

test("power actions in the page are ones the server knows", () => {
  const actions = [...body.matchAll(/data-power="([a-z]+)"/g)].map((m) => m[1]);
  assert.ok(actions.length >= 6, `found only ${actions.length} power buttons`);
  for (const a of actions) assert.ok(values("power", "a").has(a), `body.html offers power action "${a}"`);
});

test("mouse buttons and window actions in the page are ones the server knows", () => {
  for (const b of [...body.matchAll(/data-btn="([a-z]+)"/g)].map((m) => m[1])) {
    assert.ok(values("btn", "b").has(b), `body.html has mouse button "${b}"`);
  }
  for (const a of [...app.matchAll(/data-a="([a-z]+)"/g), ...body.matchAll(/data-a="([a-z]+)"/g)].map((m) => m[1])) {
    assert.ok(values("win.act", "a").has(a), `the page has window action "${a}"`);
  }
});
