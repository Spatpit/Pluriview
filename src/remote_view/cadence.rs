use std::time::{Duration, Instant};

/// Advance an absolute deadline without accumulating each frame's wake-up delay.
pub fn next(deadline: Instant, now: Instant, interval: Duration) -> Instant {
    if deadline > now {
        return deadline;
    }
    let steps = (now.duration_since(deadline).as_nanos() / interval.as_nanos() + 1)
        .min(u32::MAX as u128) as u32;
    deadline + interval * steps
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_wakeups_do_not_reduce_thirty_fps_or_produce_catch_up_bursts() {
        let start = Instant::now();
        let interval = Duration::from_nanos(1_000_000_000 / 30);
        let mut deadline = start;
        for _ in 0..300 {
            deadline = next(deadline, deadline + Duration::from_millis(7), interval);
        }
        assert_eq!(deadline.duration_since(start), interval * 300);
        let stalled = start + Duration::from_secs(20);
        let resumed = next(deadline, stalled, interval);
        assert!(resumed > stalled && resumed - stalled <= interval);
    }
}
