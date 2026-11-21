use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(750);
const FIRST_RECHECK: Duration = Duration::from_secs(2);
const SETTLE: Duration = Duration::from_secs(5);
const MAX_SETTLE: Duration = Duration::from_secs(10);

/// The first notification cancels old work immediately. Only the probes are debounced.
pub(super) struct NetworkTransition {
    first: Instant,
    last: Instant,
    pub(super) next_check: Instant,
    saw_failure: bool,
    saw_unknown_auth: bool,
}

impl NetworkTransition {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            first: now,
            last: now,
            next_check: now + DEBOUNCE,
            saw_failure: false,
            saw_unknown_auth: false,
        }
    }

    pub(super) fn changed(&mut self, now: Instant, different_route: bool) -> bool {
        if different_route {
            *self = Self::new(now);
            return true;
        }
        let limit = self.first + MAX_SETTLE;
        if now >= limit {
            // Repeated interface hints must eventually let a stable failed route be confirmed.
            return false;
        }
        self.last = now;
        self.next_check = (now + DEBOUNCE).min(limit);
        self.saw_failure = false;
        self.saw_unknown_auth = false;
        true
    }

    pub(super) fn quiet_at(&self) -> Instant {
        (self.last + DEBOUNCE).min(self.first + MAX_SETTLE)
    }

    pub(super) fn defer_failure(
        &mut self,
        started: Instant,
        now: Instant,
        unknown_auth: bool,
    ) -> bool {
        let confirm_at = (self.last + SETTLE).min(self.first + MAX_SETTLE);
        let defer =
            started < confirm_at || !self.saw_failure || (unknown_auth && !self.saw_unknown_auth);
        self.saw_failure = true;
        self.saw_unknown_auth = unknown_auth;
        let first_recheck = (self.last + FIRST_RECHECK).min(confirm_at);
        self.next_check = if now < first_recheck {
            first_recheck
        } else {
            confirm_at.max(now)
        };
        defer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_bursts_but_bound_confirmation_on_a_stable_route() {
        let start = Instant::now();
        let mut t = NetworkTransition::new(start);
        assert!(t.changed(start + DEBOUNCE / 2, false));
        assert!(t.next_check > start + DEBOUNCE);
        for second in 1..10 {
            assert!(t.changed(start + Duration::from_secs(second), false));
        }
        let limit = start + MAX_SETTLE;
        assert!(t.next_check <= limit);
        assert!(!t.changed(limit, false));
        assert!(t.defer_failure(limit, limit, true));
        assert!(!t.changed(limit + DEBOUNCE, false));
        assert!(!t.defer_failure(limit, limit + DEBOUNCE, true));
        // A genuinely different route must still prove its own result.
        assert!(t.changed(limit + DEBOUNCE, true));
        assert!(t.defer_failure(limit + DEBOUNCE, limit + DEBOUNCE, true));
    }
}
