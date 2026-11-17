//! Tab bar metrics shared by jmux and jed: one value says how tall a bar is and how much
//! padding, close target and scroll arrow its tabs get, so the touch layout is the same numbers
//! in both. A tab is `pad` cells, the title, `pad`, the `×`, `pad`; the `×` is the middle cell of
//! its `close` target, and the title sits on the bar's middle row, the other rows being filled
//! with the tab's background. A bar can end in a `+` (new tab) target, `new_tab` cells wide.

/// How a tab bar is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabMetrics {
    /// Rows the bar takes.
    pub rows: u16,
    /// Cells of padding at each end of a tab and either side of its `×`.
    pub pad: u16,
    /// Width of the close target, centred on the `×` (odd).
    pub close: u16,
    /// Width of each scroll arrow's target, the glyph in its middle.
    pub arrow: u16,
    /// Width of the `+` (new tab) target at the bar's right end, the glyph in its middle; 0 for
    /// none.
    pub new_tab: u16,
}

impl TabMetrics {
    /// One row, ` title × `, one-cell targets, no `+`.
    pub const NORMAL: TabMetrics = TabMetrics {
        rows: 1,
        pad: 1,
        close: 1,
        arrow: 1,
        new_tab: 0,
    };
    /// Three rows, `  title  ×  `, three-cell targets and a `+`, for a finger.
    pub const TOUCH: TabMetrics = TabMetrics {
        rows: 3,
        pad: 2,
        close: 3,
        arrow: 3,
        new_tab: 3,
    };

    /// Cells a tab takes for a title `title_width` cells wide.
    pub fn tab_width(&self, title_width: u16) -> u16 {
        title_width + 3 * self.pad + 1
    }

    /// The bar's row the title and `×` go on, in a bar of `height` rows.
    pub fn text_row(&self, height: u16) -> u16 {
        height / 2
    }

    /// Cells of a close target to the left of the `×` (the same to its right).
    pub fn close_reach(&self) -> u16 {
        self.close / 2
    }

    /// Cells of an arrow's target before its glyph (the same after it).
    pub fn arrow_reach(&self) -> u16 {
        self.arrow / 2
    }
}

impl Default for TabMetrics {
    fn default() -> Self {
        Self::NORMAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_is_todays_layout() {
        let m = TabMetrics::NORMAL;
        assert_eq!(m.tab_width(5), 9, "` title × ` is the title and 4");
        assert_eq!((m.rows, m.text_row(m.rows), m.close_reach(), m.new_tab), (1, 0, 0, 0));
    }

    #[test]
    fn touch_pads_and_widens_targets() {
        let m = TabMetrics::TOUCH;
        assert_eq!(m.tab_width(5), 12, "`  title  ×  `");
        assert_eq!((m.rows, m.text_row(m.rows)), (3, 1));
        assert_eq!((m.close_reach(), m.arrow_reach(), m.new_tab), (1, 1, 3));
    }
}
