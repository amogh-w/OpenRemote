//! The phone -> PC message contract: `tests/fixtures/protocol.json` lists what parses and what is refused. The
//! same file is read by `tests/frontend_protocol.test.mjs`, which checks that the phone page only sends messages
//! listed here. Change a message in `src/command.rs`, this file's `name()` and the fixture together.
use openremote::command::{is_safe_url, Msg};
use serde_json::Value;
use std::collections::BTreeSet;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/protocol.json")).unwrap()
}

fn items(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture has no {key} list")).clone()
}

/// Exhaustive on purpose: adding a `Msg` variant stops this compiling until the contract is updated.
fn name(m: &Msg) -> &'static str {
    match m {
        Msg::Move { .. } => "move",
        Msg::Btn { .. } => "btn",
        Msg::Scroll { .. } => "scroll",
        Msg::Key { .. } => "key",
        Msg::Text { .. } => "text",
        Msg::Power { .. } => "power",
        Msg::Open { .. } => "open",
        Msg::ClipSet { .. } => "clip.set",
        Msg::ClipGet => "clip.get",
        Msg::WinList => "win.list",
        Msg::WinAct { .. } => "win.act",
        Msg::AppList => "app.list",
        Msg::AppLaunch { .. } => "app.launch",
        Msg::MediaGet => "media.get",
        Msg::SleepSet { .. } => "sleep.set",
        Msg::SleepCancel => "sleep.cancel",
        Msg::SleepGet => "sleep.get",
        Msg::Ping => "ping",
    }
}

/// Keep in step with `name()` above.
const ALL: [&str; 18] = [
    "move",
    "btn",
    "scroll",
    "key",
    "text",
    "power",
    "open",
    "clip.set",
    "clip.get",
    "win.list",
    "win.act",
    "app.list",
    "app.launch",
    "media.get",
    "sleep.set",
    "sleep.cancel",
    "sleep.get",
    "ping",
];

#[test]
fn every_valid_example_parses_and_the_set_covers_every_message() {
    let mut seen = BTreeSet::new();
    for v in items(&fixture(), "valid") {
        let msg: Msg = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("should parse: {v}: {e}"));
        assert_eq!(v["t"], name(&msg), "the tag in {v} names a different message");
        seen.insert(name(&msg));
    }
    let all: BTreeSet<&str> = ALL.into_iter().collect();
    assert_eq!(seen, all, "the fixture needs a valid example of every message");
}

#[test]
fn every_invalid_example_is_refused() {
    for v in items(&fixture(), "invalid") {
        assert!(serde_json::from_value::<Msg>(v.clone()).is_err(), "should be refused: {v}");
    }
}

#[test]
fn only_plain_web_urls_may_be_opened() {
    let f = fixture();
    for u in items(&f, "unsafe_urls") {
        assert!(!is_safe_url(u.as_str().unwrap()), "should be unsafe: {u}");
    }
    for u in items(&f, "safe_urls") {
        assert!(is_safe_url(u.as_str().unwrap()), "should be safe: {u}");
    }
}
