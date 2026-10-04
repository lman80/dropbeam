//! Live transfer speed (audit T15): a rolling window over the bytes THIS
//! attempt moved. Dividing the cumulative `done` (which includes earlier
//! pushes of a folder and a resume's head start) by this attempt's elapsed
//! time showed GB/s on a resumed transfer.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct RollingRate {
    samples: VecDeque<(Instant, u64)>,
}

impl RollingRate {
    /// Record `done` (cumulative bytes) at `now`; the current bytes/second.
    pub(crate) fn observe(&mut self, now: Instant, done: u64) -> f64 {
        if self.samples.back().is_some_and(|&(_, last)| done < last) {
            // A new attempt restarted the count: start a fresh window.
            self.samples.clear();
        }
        self.samples.push_back((now, done));
        // Keep one sample at or beyond the window edge as the baseline.
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0) >= WINDOW {
            self.samples.pop_front();
        }
        let (t0, d0) = self.samples[0];
        let secs = now.duration_since(t0).as_secs_f64();
        if secs < 0.25 { return 0.0; }
        done.saturating_sub(d0) as f64 / secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t15_resume_head_start_is_not_speed() {
        let t = Instant::now();
        let mut r = RollingRate::default();
        // A resume starts at 9 GB already on the far side.
        assert_eq!(r.observe(t, 9_000_000_000), 0.0);
        let s = r.observe(t + Duration::from_secs(1), 9_010_000_000);
        assert!((s - 10_000_000.0).abs() < 1.0, "{s}");
        // A stall shows up within the window, not averaged away over the whole run.
        let mut last = 0.0;
        for i in 2..=10 { last = r.observe(t + Duration::from_secs(i), 9_010_000_000); }
        assert_eq!(last, 0.0);
        // A restarted count resets instead of going negative.
        assert_eq!(r.observe(t + Duration::from_secs(11), 5), 0.0);
    }
}
