//! Counting clicks on one cell in quick succession (single, double, triple).

use std::time::{Duration, Instant};

const MULTI_CLICK: Duration = Duration::from_millis(500);

/// Counts clicks on the same cell in quick succession (1, 2, 3).
#[derive(Default)]
pub struct ClickTracker {
    last: Option<(Instant, u16, u16)>,
    count: u8,
}

impl ClickTracker {
    pub fn click(&mut self, now: Instant, x: u16, y: u16) -> u8 {
        self.count = match self.last {
            Some((t, lx, ly)) if lx == x && ly == y && now.duration_since(t) < MULTI_CLICK => self.count % 3 + 1,
            _ => 1,
        };
        self.last = Some((now, x, y));
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_counting() {
        let mut c = ClickTracker::default();
        let t = Instant::now();
        assert_eq!(c.click(t, 1, 1), 1);
        assert_eq!(c.click(t + Duration::from_millis(100), 1, 1), 2);
        assert_eq!(c.click(t + Duration::from_millis(200), 1, 1), 3);
        assert_eq!(c.click(t + Duration::from_millis(300), 1, 1), 1, "wraps after triple");
        assert_eq!(c.click(t + Duration::from_millis(350), 2, 1), 1, "different cell");
        assert_eq!(c.click(t + Duration::from_secs(2), 2, 1), 1, "too slow");
    }
}
