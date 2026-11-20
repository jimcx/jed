//! The finger's gesture on a bar or a view: a swipe scrolls (`dx` columns, `dy` rows), a tap selects, a
//! hold picks the item up (a tab) or starts a selection / opens a menu (text). A pure
//! state machine with no clock: the app starts a `HOLD` timer at the press and calls `Press::held`
//! when it fires (a finger held still sends no events), and feeds it the motion and the release.
//! Motion within `SLOP` cells on either axis of the press is still a hold or a tap, so the jitter
//! of a finger doesn't turn a tap into a swipe.

use std::time::Duration;

/// How long a finger stays still before the item under it is picked up.
pub const HOLD: Duration = Duration::from_millis(450);

/// How far (cells, on either axis) a finger may stray from where it landed and still tap or hold.
pub const SLOP: u16 = 1;

/// What a press has turned into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Nothing to act on yet (or any more: a swipe's release, a stale `held`).
    Idle,
    /// Released within `SLOP` before `HOLD`.
    Tap,
    /// Moved past `SLOP` before `HOLD`; `dx` / `dy` are the columns / rows moved since the last motion.
    Swipe { dx: i32, dy: i32 },
    /// `HOLD` passed within `SLOP`: the item is picked up.
    Lifted,
    /// Motion after the lift.
    Drag,
    /// Release after the lift.
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Down,
    Swiping,
    Lifted,
}

/// One finger's press, from landing to release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    origin: (u16, u16),
    last_x: u16,
    last_y: u16,
    phase: Phase,
}

impl Press {
    /// A finger landed at `at` (column, row).
    pub fn new(at: (u16, u16)) -> Press {
        Press {
            origin: at,
            last_x: at.0,
            last_y: at.1,
            phase: Phase::Down,
        }
    }

    fn within_slop(&self, at: (u16, u16)) -> bool {
        at.0.abs_diff(self.origin.0) <= SLOP && at.1.abs_diff(self.origin.1) <= SLOP
    }

    /// The finger moved to `at`.
    pub fn moved(&mut self, at: (u16, u16)) -> Gesture {
        match self.phase {
            Phase::Lifted => Gesture::Drag,
            Phase::Down if self.within_slop(at) => Gesture::Idle,
            Phase::Down | Phase::Swiping => {
                self.phase = Phase::Swiping;
                let dx = i32::from(at.0) - i32::from(self.last_x);
                let dy = i32::from(at.1) - i32::from(self.last_y);
                self.last_x = at.0;
                self.last_y = at.1;
                Gesture::Swipe { dx, dy }
            }
        }
    }

    /// `HOLD` passed: `Lifted` if the finger is still within `SLOP`, else (a swipe, or already
    /// lifted) `Idle`.
    pub fn held(&mut self) -> Gesture {
        if self.phase == Phase::Down {
            self.phase = Phase::Lifted;
            Gesture::Lifted
        } else {
            Gesture::Idle
        }
    }

    /// The finger lifted at `at`.
    pub fn released(&mut self, at: (u16, u16)) -> Gesture {
        match self.phase {
            Phase::Lifted => Gesture::Drop,
            Phase::Down if self.within_slop(at) => Gesture::Tap,
            Phase::Down | Phase::Swiping => Gesture::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_within_slop_before_hold_is_a_tap() {
        let mut p = Press::new((10, 5));
        assert_eq!(p.released((10, 5)), Gesture::Tap);
        let mut p = Press::new((10, 5));
        assert_eq!(p.released((11, 4)), Gesture::Tap);
    }

    #[test]
    fn jitter_within_slop_is_not_a_swipe() {
        let mut p = Press::new((10, 5));
        assert_eq!(p.moved((11, 5)), Gesture::Idle);
        assert_eq!(p.moved((10, 6)), Gesture::Idle);
        assert_eq!(p.released((11, 6)), Gesture::Tap);
        let mut p = Press::new((10, 5));
        p.moved((9, 4));
        assert_eq!(p.held(), Gesture::Lifted);
    }

    #[test]
    fn motion_past_slop_swipes_by_the_delta_since_the_last_motion() {
        let mut p = Press::new((10, 5));
        assert_eq!(p.moved((7, 5)), Gesture::Swipe { dx: -3, dy: 0 });
        assert_eq!(p.moved((8, 5)), Gesture::Swipe { dx: 1, dy: 0 });
        assert_eq!(p.moved((8, 5)), Gesture::Swipe { dx: 0, dy: 0 });
        // Back near where it landed, it is still a swipe.
        assert_eq!(p.moved((10, 5)), Gesture::Swipe { dx: 2, dy: 0 });
    }

    #[test]
    fn vertical_motion_past_slop_swipes_by_dy() {
        let mut p = Press::new((10, 5));
        assert_eq!(p.moved((10, 8)), Gesture::Swipe { dx: 0, dy: 3 });
        assert_eq!(p.moved((9, 6)), Gesture::Swipe { dx: -1, dy: -2 });
    }

    #[test]
    fn a_swipe_does_not_tap_on_release_or_lift_on_hold() {
        let mut p = Press::new((10, 5));
        p.moved((14, 5));
        assert_eq!(p.held(), Gesture::Idle);
        assert_eq!(p.released((14, 5)), Gesture::Idle);
        // Released back on the spot is still no tap.
        let mut p = Press::new((10, 5));
        p.moved((14, 5));
        assert_eq!(p.released((10, 5)), Gesture::Idle);
    }

    #[test]
    fn a_release_far_away_without_motion_is_no_tap() {
        let mut p = Press::new((10, 5));
        assert_eq!(p.released((20, 5)), Gesture::Idle);
    }

    #[test]
    fn hold_lifts_then_motion_drags_and_release_drops() {
        let mut p = Press::new((10, 5));
        assert_eq!(p.held(), Gesture::Lifted);
        assert_eq!(p.moved((30, 9)), Gesture::Drag);
        assert_eq!(p.held(), Gesture::Idle);
        assert_eq!(p.released((30, 9)), Gesture::Drop);
    }

    #[test]
    fn released_without_moving_after_a_lift_drops() {
        let mut p = Press::new((10, 5));
        p.held();
        assert_eq!(p.released((10, 5)), Gesture::Drop);
    }

    #[test]
    fn the_constants() {
        assert_eq!(HOLD, Duration::from_millis(450));
        assert_eq!(SLOP, 1);
    }
}
