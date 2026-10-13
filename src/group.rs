//! Editor groups and their tabs. One group until milestone 3.

use crate::buffer::BufferId;
use crate::view::View;

pub struct Tab {
    pub buffer: BufferId,
    pub view: View,
}

#[derive(Default)]
pub struct Group {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// First tab shown in the tab bar.
    pub scroll: usize,
}

impl Group {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn find(&self, buffer: BufferId) -> Option<usize> {
        self.tabs.iter().position(|t| t.buffer == buffer)
    }

    /// Opens a tab to the right of the active one and activates it.
    pub fn open(&mut self, buffer: BufferId, view: View) -> usize {
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, Tab { buffer, view });
        self.active = at;
        at
    }

    /// Removes a tab; the tab to its right (or left, at the end) becomes active.
    pub fn close(&mut self, index: usize) -> Tab {
        let tab = self.tabs.remove(index);
        if index < self.active || (index == self.active && self.active >= self.tabs.len()) {
            self.active = self.active.saturating_sub(1);
        }
        tab
    }

    pub fn cycle(&mut self, delta: isize) {
        if !self.tabs.is_empty() {
            let n = self.tabs.len() as isize;
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(n: u64) -> Group {
        let mut g = Group::default();
        for id in 0..n {
            g.tabs.push(Tab {
                buffer: id,
                view: View::new(),
            });
        }
        g
    }

    #[test]
    fn closing_moves_active_sensibly() {
        let mut g = group(4);
        g.active = 1;
        g.close(1);
        assert_eq!(g.tabs[g.active].buffer, 2, "right neighbour");
        g.active = 2;
        g.close(2);
        assert_eq!(g.tabs[g.active].buffer, 2, "left neighbour at the end");
        g.close(0);
        assert_eq!(g.tabs[g.active].buffer, 2, "closing to the left keeps the same tab");
    }

    #[test]
    fn open_inserts_right_of_active_and_cycle_wraps() {
        let mut g = group(3);
        g.active = 0;
        g.open(9, View::new());
        assert_eq!(g.tabs.iter().map(|t| t.buffer).collect::<Vec<_>>(), vec![0, 9, 1, 2]);
        assert_eq!(g.active, 1);
        g.cycle(-2);
        assert_eq!(g.active, 3);
    }
}
