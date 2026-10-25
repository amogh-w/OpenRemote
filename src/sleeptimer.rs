//! Sleep timer, shared by OpenRemote and Kai Remote: edit it in the cuppa plugin repo (shared/) and run
//! harness.mjs sync --write, never in a project copy. The phone only starts, cancels and displays it; the daemon decides when the PC sleeps.
//!
//! One `State` behind a mutex holds a generation counter and the current timer (at most one). Starting, replacing and
//! cancelling bump the generation; a watcher thread carries the generation it was spawned with and exits quietly when
//! it is no longer current, so a replaced or cancelled timer can never fire.
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Longest timer a phone can ask for.
pub const MAX_SECS: u64 = 12 * 3600;
/// Extra time after an episode ends, for the credits and the player to settle.
const GRACE: Duration = Duration::from_secs(2);
/// An episode timer that finds more than this left (paused, or seeked back) moves its deadline instead of sleeping.
const MORE_LEFT: f64 = 3.0;
const TICK: Duration = Duration::from_millis(200);

/// What is playing right now.
pub struct Media {
    pub pos: f64,
    pub dur: f64,
    pub title: String,
}

/// The outside world: the player state, the power action and the way to tell the phones.
pub trait Host: Send + Sync + 'static {
    /// None when nothing is playing or the length is unknown.
    fn media(&self) -> Option<Media>;
    fn suspend(&self);
    /// Called with the new status when a timer fires, so every phone sees it end. Default: nobody to tell.
    fn notify(&self, _status: Value) {}
    /// The message-type key of the reply: `"t"` or `"k"`, whatever the app's protocol uses.
    fn key(&self) -> &'static str {
        "t"
    }
}

struct Timer {
    started: Instant,
    deadline: Instant,
    /// The playing title of an episode timer.
    title: Option<String>,
}

#[derive(Default)]
struct State {
    generation: u64,
    timer: Option<Timer>,
}

struct Inner {
    state: Mutex<State>,
    host: Box<dyn Host>,
    tick: Duration,
}

#[derive(Clone)]
pub struct SleepTimer(Arc<Inner>);

fn status(key: &str, s: &State, err: Option<&str>) -> Value {
    let mut v = match &s.timer {
        Some(t) => json!({
            "on": true, "episode": t.title.is_some(),
            "left": t.deadline.saturating_duration_since(Instant::now()).as_secs_f64(),
            "total": t.deadline.duration_since(t.started).as_secs_f64(),
        }),
        None => json!({ "on": false, "left": 0, "total": 0, "episode": false }),
    };
    v[key] = json!("sleep");
    if let Some(e) = err {
        v["err"] = json!(e);
    }
    v
}

impl SleepTimer {
    pub fn new(host: impl Host) -> Self {
        Self::with_tick(host, TICK)
    }

    fn with_tick(host: impl Host, tick: Duration) -> Self {
        Self(Arc::new(Inner { state: Mutex::default(), host: Box::new(host), tick }))
    }

    pub fn get(&self) -> Value {
        status(self.0.host.key(), &self.0.state.lock().unwrap(), None)
    }

    pub fn cancel(&self) -> Value {
        let mut s = self.0.state.lock().unwrap();
        s.generation += 1;
        s.timer = None;
        status(self.0.host.key(), &s, None)
    }

    /// `secs` is clamped to 12 hours and 0 is refused. With `episode` the timer ends with the playing episode.
    pub fn set(&self, secs: Option<u64>, episode: bool) -> Value {
        let now = Instant::now();
        let (after, title) = if episode {
            // The media call runs without the lock held.
            match self.0.host.media() {
                Some(m) if m.dur.is_finite() && m.dur > 0.0 => {
                    (Duration::from_secs_f64((m.dur - m.pos).max(0.0)) + GRACE, Some(m.title))
                }
                _ => return self.refuse("no-episode"),
            }
        } else {
            match secs {
                Some(s) if s > 0 => (Duration::from_secs(s.min(MAX_SECS)), None),
                _ => return self.refuse("bad-time"),
            }
        };
        self.start(Timer { started: now, deadline: now + after, title })
    }

    fn start(&self, timer: Timer) -> Value {
        let mut s = self.0.state.lock().unwrap();
        s.generation += 1;
        s.timer = Some(timer);
        let generation = s.generation;
        let out = status(self.0.host.key(), &s, None);
        drop(s);
        let this = self.clone();
        std::thread::spawn(move || this.watch(generation));
        out
    }

    /// A refused request leaves the running timer alone and reports it with the reason.
    fn refuse(&self, err: &str) -> Value {
        status(self.0.host.key(), &self.0.state.lock().unwrap(), Some(err))
    }

    fn watch(&self, generation: u64) {
        let inner = &*self.0;
        loop {
            std::thread::sleep(inner.tick);
            let s = inner.state.lock().unwrap();
            if s.generation != generation {
                return;
            }
            let Some(t) = s.timer.as_ref() else { return };
            if Instant::now() < t.deadline {
                continue;
            }
            let Some(title) = t.title.clone() else {
                return self.fire(s);
            };
            drop(s);
            let media = inner.host.media();
            let mut s = inner.state.lock().unwrap();
            if s.generation != generation {
                return;
            }
            match media {
                Some(m) if m.title == title && m.dur - m.pos > MORE_LEFT => {
                    if let Some(t) = s.timer.as_mut() {
                        t.deadline = Instant::now() + Duration::from_secs_f64(m.dur - m.pos) + GRACE;
                    }
                }
                _ => return self.fire(s),
            }
        }
    }

    fn fire(&self, mut s: std::sync::MutexGuard<'_, State>) {
        s.timer = None;
        let off = status(self.0.host.key(), &s, None);
        drop(s);
        self.0.host.notify(off);
        self.0.host.suspend();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mock {
        media: Arc<Mutex<Option<Media>>>,
        slept: Arc<AtomicUsize>,
    }

    impl Host for Mock {
        fn media(&self) -> Option<Media> {
            self.media.lock().unwrap().as_ref().map(|m| Media { pos: m.pos, dur: m.dur, title: m.title.clone() })
        }
        fn suspend(&self) {
            self.slept.fetch_add(1, Ordering::SeqCst);
        }
        fn notify(&self, _: Value) {}
    }

    fn rig(media: Option<Media>) -> (SleepTimer, Arc<Mutex<Option<Media>>>, Arc<AtomicUsize>) {
        let media = Arc::new(Mutex::new(media));
        let slept = Arc::new(AtomicUsize::new(0));
        let t = SleepTimer::with_tick(Mock { media: media.clone(), slept: slept.clone() }, Duration::from_millis(5));
        (t, media, slept)
    }

    fn short(t: &SleepTimer, ms: u64) {
        t.start(Timer { started: Instant::now(), deadline: Instant::now() + Duration::from_millis(ms), title: None });
    }

    fn wait() {
        std::thread::sleep(Duration::from_millis(150));
    }

    #[test]
    fn plain_timer_sleeps_once_and_clears_itself() {
        let (t, _, slept) = rig(None);
        short(&t, 20);
        wait();
        assert_eq!(slept.load(Ordering::SeqCst), 1);
        assert_eq!(t.get()["on"], false);
    }

    #[test]
    fn cancel_and_replace_stop_the_old_timer() {
        let (t, _, slept) = rig(None);
        short(&t, 40);
        assert_eq!(t.cancel()["on"], false);
        wait();
        assert_eq!(slept.load(Ordering::SeqCst), 0, "a cancelled timer never fires");
        short(&t, 40);
        t.set(Some(3600), false);
        wait();
        assert_eq!(slept.load(Ordering::SeqCst), 0, "a replaced timer never fires");
        assert_eq!(t.get()["on"], true);
    }

    #[test]
    fn seconds_are_clamped_and_zero_refused() {
        let (t, _, _) = rig(None);
        let v = t.set(Some(0), false);
        assert_eq!((v["on"].clone(), v["err"].clone()), (json!(false), json!("bad-time")));
        assert_eq!(t.set(None, false)["err"], "bad-time");
        let v = t.set(Some(10 * 24 * 3600), false);
        assert_eq!(v["total"].as_f64().unwrap(), MAX_SECS as f64);
        // A refused request keeps the running timer.
        assert_eq!(t.set(Some(0), false)["on"], true);
    }

    #[test]
    fn episode_needs_a_known_length() {
        let (t, media, _) = rig(None);
        assert_eq!(t.set(None, true)["err"], "no-episode");
        *media.lock().unwrap() = Some(Media { pos: 0.0, dur: 0.0, title: "a".into() });
        assert_eq!(t.set(None, true)["err"], "no-episode");
        *media.lock().unwrap() = Some(Media { pos: 60.0, dur: 160.0, title: "a".into() });
        let v = t.set(None, true);
        assert_eq!(v["episode"], true);
        assert!((v["left"].as_f64().unwrap() - 102.0).abs() < 1.0, "dur - pos + 2 s grace");
    }

    #[test]
    fn episode_timer_waits_while_there_is_more_to_play() {
        let (t, media, slept) = rig(Some(Media { pos: 0.0, dur: 100.0, title: "a".into() }));
        t.start(Timer { started: Instant::now(), deadline: Instant::now(), title: Some("a".into()) });
        wait();
        assert_eq!(slept.load(Ordering::SeqCst), 0, "still 100 s left, so the deadline moves");
        let v = t.get();
        assert!(v["left"].as_f64().unwrap() > 90.0 && v["total"].as_f64().unwrap() > 90.0);
        // The title changed (next episode started): sleep.
        *media.lock().unwrap() = Some(Media { pos: 0.0, dur: 100.0, title: "b".into() });
        t.0.state.lock().unwrap().timer.as_mut().unwrap().deadline = Instant::now();
        wait();
        assert_eq!(slept.load(Ordering::SeqCst), 1);
    }
}
