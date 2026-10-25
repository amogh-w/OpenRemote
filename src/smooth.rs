//! Pointer smoothing.
//!
//! A phone sends finger movement in whole-pixel chunks once per screen frame, and Wi-Fi delivers those chunks
//! unevenly: two arrive together, then nothing for 30 ms. Injecting them as they land makes the cursor jump.
//! The `Smoother` instead collects what has arrived and hands it to Windows in small steps every few
//! milliseconds, draining the backlog exponentially: each tick releases the same share of what is left, so
//! a burst becomes a glide. `tau_ms` is the time constant: about 63% of a chunk is out after one `tau`, and the
//! added delay is roughly `tau`. Nothing is lost; sub-pixel remainders carry over to the next tick.
//!
//! Clicks, drops and wheel turns call [`Smoother::flush`] first, so a tap always lands where the finger stopped.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How often the pacer thread releases movement while there is some pending.
const TICK: Duration = Duration::from_millis(4);
/// Upper bound on queued movement, so a stalled consumer cannot build an hour of cursor travel.
const MAX_PENDING: f64 = 4000.0;

#[derive(Default)]
struct State {
    /// Movement received but not yet released.
    px: f64,
    py: f64,
    /// Released, but less than a whole pixel so far.
    ax: f64,
    ay: f64,
}

pub struct Smoother {
    tau_ms: f64,
    state: Mutex<State>,
    wake: Condvar,
}

/// The whole pixels in `v`, tolerant of float error (19.999999999 counts as 20).
fn whole(v: f64) -> f64 {
    (v + v.signum() * 1e-9).trunc()
}

impl Smoother {
    pub fn new(tau_ms: f64) -> Self {
        Self { tau_ms: tau_ms.max(1.0), state: Mutex::new(State::default()), wake: Condvar::new() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue relative movement.
    pub fn push(&self, dx: i32, dy: i32) {
        let mut s = self.lock();
        s.px += f64::from(dx);
        s.py += f64::from(dy);
        let len = s.px.hypot(s.py);
        if len > MAX_PENDING {
            let k = MAX_PENDING / len;
            s.px *= k;
            s.py *= k;
        }
        drop(s);
        self.wake.notify_one();
    }

    /// Release one tick's share of the queue. `emit` runs while the lock is held, so movement and the clicks
    /// that follow it can never be reordered.
    pub fn step(&self, dt_ms: f64, emit: &mut dyn FnMut(i32, i32)) {
        let mut s = self.lock();
        let k = 1.0 - (-dt_ms / self.tau_ms).exp();
        let (tx, ty) = (s.px * k, s.py * k);
        s.px -= tx;
        s.py -= ty;
        s.ax += tx;
        s.ay += ty;
        // The tail of an exponential never quite ends; under half a pixel there is nothing left to spread.
        if s.px.abs() < 0.5 {
            s.ax += s.px;
            s.px = 0.0;
        }
        if s.py.abs() < 0.5 {
            s.ay += s.py;
            s.py = 0.0;
        }
        Self::release(&mut s, emit);
    }

    /// Release everything queued right now (before a click, a drop or a scroll).
    pub fn flush(&self, emit: &mut dyn FnMut(i32, i32)) {
        let mut s = self.lock();
        s.ax += s.px;
        s.ay += s.py;
        s.px = 0.0;
        s.py = 0.0;
        Self::release(&mut s, emit);
    }

    fn release(s: &mut State, emit: &mut dyn FnMut(i32, i32)) {
        let (ix, iy) = (whole(s.ax), whole(s.ay));
        s.ax -= ix;
        s.ay -= iy;
        if ix != 0.0 || iy != 0.0 {
            emit(ix as i32, iy as i32);
        }
    }

    /// Start the pacer thread. It sleeps until movement is queued, then ticks every few milliseconds until the
    /// queue is empty again, so an idle phone costs no CPU.
    pub fn spawn(self: &Arc<Self>, emit: impl Fn(i32, i32) + Send + 'static) {
        let me = Arc::clone(self);
        let started = std::thread::Builder::new().name("pointer-smoother".into()).spawn(move || {
            let mut last = Instant::now();
            loop {
                {
                    let mut s = me.lock();
                    while s.px == 0.0 && s.py == 0.0 {
                        s = me.wake.wait(s).unwrap_or_else(|e| e.into_inner());
                        last = Instant::now();
                    }
                }
                std::thread::sleep(TICK);
                let now = Instant::now();
                let dt = now.duration_since(last).as_secs_f64() * 1000.0;
                last = now;
                me.step(dt.clamp(1.0, 50.0), &mut |dx, dy| emit(dx, dy));
            }
        });
        if let Err(e) = started {
            crate::applog::error(&format!("cannot start the pointer smoother: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(s: &Smoother, ticks: usize) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            s.step(4.0, &mut |dx, dy| out.push((dx, dy)));
        }
        out
    }

    #[test]
    fn a_burst_is_spread_over_several_steps_and_nothing_is_lost() {
        let s = Smoother::new(12.0);
        s.push(40, -20);
        let out = drain(&s, 100);
        assert!(out.len() > 5, "expected a glide, got {out:?}");
        assert!(out[0].0 < 40, "the first step must not release the whole burst: {out:?}");
        let (sx, sy) = out.iter().fold((0, 0), |a, o| (a.0 + o.0, a.1 + o.1));
        assert_eq!((sx, sy), (40, -20));
    }

    #[test]
    fn steps_shrink_as_the_backlog_drains() {
        let s = Smoother::new(12.0);
        s.push(60, 0);
        let out = drain(&s, 12);
        assert!(out.first().unwrap().0 >= out.last().unwrap().0, "{out:?}");
    }

    #[test]
    fn a_steady_stream_comes_out_steady() {
        let s = Smoother::new(12.0);
        let mut out = Vec::new();
        for _ in 0..200 {
            s.push(1, 0); // 1 px per 4 ms tick
            s.step(4.0, &mut |dx, _| out.push(dx));
        }
        let late = &out[100..];
        assert!(late.iter().all(|&d| d <= 1), "steps should be 0 or 1 px: {late:?}");
        assert!(late.iter().sum::<i32>() >= 95, "most of the stream should arrive: {late:?}");
    }

    #[test]
    fn flush_releases_everything_at_once() {
        let s = Smoother::new(50.0);
        s.push(33, 7);
        let mut out = Vec::new();
        s.flush(&mut |dx, dy| out.push((dx, dy)));
        assert_eq!(out, [(33, 7)]);
        s.flush(&mut |dx, dy| out.push((dx, dy)));
        assert_eq!(out.len(), 1, "nothing left to flush");
    }

    #[test]
    fn queued_movement_is_capped() {
        let s = Smoother::new(12.0);
        for _ in 0..100 {
            s.push(5000, 0);
        }
        let mut out = Vec::new();
        s.flush(&mut |dx, dy| out.push((dx, dy)));
        assert!(out[0].0 <= MAX_PENDING as i32);
    }

    #[test]
    fn float_error_does_not_swallow_the_last_pixel() {
        assert_eq!(whole(19.999999999999), 20.0);
        assert_eq!(whole(-19.999999999999), -20.0);
        assert_eq!(whole(0.4), 0.0);
    }
}
