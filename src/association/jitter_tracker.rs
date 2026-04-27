use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::time::Duration;
use std::time::Instant;

const DEFAULT_WINDOW: Duration = Duration::from_secs(30);
const DEFAULT_MAX_SAMPLES: usize = 256;
const MIN_SAMPLES_BEFORE_USE: usize = 16;
const PATH_CHANGE_RATIO_PERCENT: u64 = 150;

#[derive(Debug, Clone, Copy)]
struct Sample {
    recorded_at: Instant,
    delta_us: u64,
}

#[derive(Debug)]
pub(crate) struct JitterTracker {
    samples: VecDeque<Sample>,
    window: Duration,
    max_samples: usize,
    reference_min_rtt: Option<Duration>,
    /// Highest p95 ever observed on this association since the last path-change
    /// reset. Used by the floor computation as a cold-start anchor: when the
    /// 30-second sliding window decays back into the bulk after a quiet period,
    /// the dynamic floor still gets a lower bound of `historical_max_p95 / 2`,
    /// preventing the next jitter burst from re-paying the full warmup penalty.
    historical_max_p95: Option<u64>,
}

impl JitterTracker {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(DEFAULT_MAX_SAMPLES),
            window: DEFAULT_WINDOW,
            max_samples: DEFAULT_MAX_SAMPLES,
            reference_min_rtt: None,
            historical_max_p95: None,
        }
    }

    pub(crate) fn record(&mut self, now: Instant, delta_us: u64) {
        self.evict_old(now);
        self.samples.push_back(Sample {
            recorded_at: now,
            delta_us,
        });
        while self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
    }

    pub(crate) fn p95(&self) -> Option<u64> {
        if self.samples.len() < MIN_SAMPLES_BEFORE_USE {
            return None;
        }
        let mut buf: Vec<u64> = self.samples.iter().map(|s| s.delta_us).collect();
        buf.sort_unstable();
        let idx = (buf.len() * 95 / 100).saturating_sub(1).min(buf.len() - 1);
        Some(buf[idx])
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    /// Record the most recent p95 reading so the tracker can remember the
    /// historical maximum across sample-window decay. Caller fetches `p95()`
    /// and forwards the value here once per RACK pass.
    pub(crate) fn note_p95(&mut self, p95: u64) {
        if self.historical_max_p95.is_none_or(|h| p95 > h) {
            self.historical_max_p95 = Some(p95);
        }
    }

    pub(crate) fn historical_max_p95(&self) -> Option<u64> {
        self.historical_max_p95
    }

    pub(crate) fn reset(&mut self) {
        self.samples.clear();
        self.reference_min_rtt = None;
        self.historical_max_p95 = None;
    }

    pub(crate) fn maybe_reset_on_path_change(&mut self, current_min_rtt: Duration) {
        if current_min_rtt.is_zero() {
            return;
        }
        match self.reference_min_rtt {
            None => self.reference_min_rtt = Some(current_min_rtt),
            Some(ref_rtt) if ref_rtt.is_zero() => {
                self.reference_min_rtt = Some(current_min_rtt);
            }
            Some(ref_rtt) => {
                let cur = current_min_rtt.as_micros() as u64;
                let r = ref_rtt.as_micros() as u64;
                let ratio = if cur >= r {
                    cur.saturating_mul(100) / r.max(1)
                } else {
                    r.saturating_mul(100) / cur.max(1)
                };
                if ratio > PATH_CHANGE_RATIO_PERCENT {
                    self.samples.clear();
                    self.reference_min_rtt = Some(current_min_rtt);
                    self.historical_max_p95 = None;
                }
            }
        }
    }

    fn evict_old(&mut self, now: Instant) {
        let cutoff = now.checked_sub(self.window);
        if let Some(cutoff) = cutoff {
            while let Some(front) = self.samples.front() {
                if front.recorded_at < cutoff {
                    self.samples.pop_front();
                } else {
                    break;
                }
            }
        }
    }
}

impl Default for JitterTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn test_empty_returns_none() {
        let tracker = JitterTracker::new();
        assert_eq!(None, tracker.p95());
        assert_eq!(0, tracker.len());
    }

    #[test]
    fn test_below_min_samples_returns_none() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for i in 0..(MIN_SAMPLES_BEFORE_USE - 1) {
            tracker.record(now, 1000 + i as u64);
        }
        assert_eq!(MIN_SAMPLES_BEFORE_USE - 1, tracker.len());
        assert_eq!(None, tracker.p95());
    }

    #[test]
    fn test_at_min_samples_returns_value() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for i in 0..MIN_SAMPLES_BEFORE_USE {
            tracker.record(now, 1000 + i as u64);
        }
        assert_eq!(MIN_SAMPLES_BEFORE_USE, tracker.len());
        assert!(tracker.p95().is_some());
    }

    #[test]
    fn test_p95_correctness_uniform_distribution() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for _ in 0..100 {
            tracker.record(now, 1000);
        }
        assert_eq!(Some(1000), tracker.p95());
    }

    #[test]
    fn test_p95_correctness_skewed_distribution() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for i in 0..100 {
            tracker.record(now, 1000 + i as u64);
        }
        let p95 = tracker.p95().expect("enough samples");
        assert!(
            p95 >= 1090 && p95 <= 1099,
            "expected p95 in [1090, 1099], got {p95}"
        );
    }

    #[test]
    fn test_p95_with_outlier_tail() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for _ in 0..95 {
            tracker.record(now, 500);
        }
        for _ in 0..5 {
            tracker.record(now, 5000);
        }
        let p95 = tracker.p95().expect("enough samples");
        assert!(p95 == 500 || p95 == 5000, "got {p95}");
    }

    #[test]
    fn test_window_eviction_by_time() {
        let mut tracker = JitterTracker::new();
        let t = t0();
        for i in 0..MIN_SAMPLES_BEFORE_USE {
            tracker.record(t + Duration::from_millis(i as u64), 1000 + i as u64);
        }
        assert_eq!(MIN_SAMPLES_BEFORE_USE, tracker.len());

        let after_window = t + DEFAULT_WINDOW + Duration::from_secs(1);
        tracker.record(after_window, 9999);
        assert_eq!(1, tracker.len());
    }

    #[test]
    fn test_window_eviction_by_count() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for i in 0..(DEFAULT_MAX_SAMPLES + 50) {
            tracker.record(now, 1000 + i as u64);
        }
        assert_eq!(DEFAULT_MAX_SAMPLES, tracker.len());
    }

    #[test]
    fn test_path_change_resets_on_large_shift() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for _ in 0..MIN_SAMPLES_BEFORE_USE {
            tracker.record(now, 1000);
        }
        tracker.maybe_reset_on_path_change(Duration::from_millis(100));
        assert_eq!(MIN_SAMPLES_BEFORE_USE, tracker.len());

        tracker.maybe_reset_on_path_change(Duration::from_millis(300));
        assert_eq!(0, tracker.len());
    }

    #[test]
    fn test_path_change_no_reset_on_minor_jitter() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for _ in 0..MIN_SAMPLES_BEFORE_USE {
            tracker.record(now, 1000);
        }
        tracker.maybe_reset_on_path_change(Duration::from_millis(100));
        tracker.maybe_reset_on_path_change(Duration::from_millis(120));
        assert_eq!(MIN_SAMPLES_BEFORE_USE, tracker.len());
    }

    #[test]
    fn test_path_change_no_reset_on_zero_min_rtt() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for _ in 0..MIN_SAMPLES_BEFORE_USE {
            tracker.record(now, 1000);
        }
        tracker.maybe_reset_on_path_change(Duration::from_millis(100));
        tracker.maybe_reset_on_path_change(Duration::ZERO);
        assert_eq!(MIN_SAMPLES_BEFORE_USE, tracker.len());
    }

    #[test]
    fn test_reset_clears_state() {
        let mut tracker = JitterTracker::new();
        let now = t0();
        for _ in 0..MIN_SAMPLES_BEFORE_USE {
            tracker.record(now, 1000);
        }
        tracker.maybe_reset_on_path_change(Duration::from_millis(100));
        tracker.reset();
        assert_eq!(0, tracker.len());
        assert_eq!(None, tracker.p95());
        tracker.maybe_reset_on_path_change(Duration::from_millis(100));
        assert_eq!(0, tracker.len());
    }

    #[test]
    fn test_note_p95_tracks_running_maximum() {
        let mut tracker = JitterTracker::new();
        assert_eq!(None, tracker.historical_max_p95());
        tracker.note_p95(500);
        assert_eq!(Some(500), tracker.historical_max_p95());
        tracker.note_p95(1500);
        assert_eq!(Some(1500), tracker.historical_max_p95());
        tracker.note_p95(800);
        assert_eq!(Some(1500), tracker.historical_max_p95());
    }

    #[test]
    fn test_historical_max_cleared_on_reset() {
        let mut tracker = JitterTracker::new();
        tracker.note_p95(2_000_000);
        assert_eq!(Some(2_000_000), tracker.historical_max_p95());
        tracker.reset();
        assert_eq!(None, tracker.historical_max_p95());
    }

    #[test]
    fn test_historical_max_cleared_on_path_change() {
        let mut tracker = JitterTracker::new();
        tracker.note_p95(2_000_000);
        tracker.maybe_reset_on_path_change(Duration::from_millis(100));
        assert_eq!(Some(2_000_000), tracker.historical_max_p95());
        tracker.maybe_reset_on_path_change(Duration::from_millis(300));
        assert_eq!(None, tracker.historical_max_p95());
    }
}
