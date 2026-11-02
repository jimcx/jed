//! How the Explorer's body divides between the tree and the sections under it ("FILE HISTORY",
//! then "GIT HISTORY"): pure arithmetic on heights, no state. A section is a divider row, a
//! header row and its body (a collapsed one is just the first two); the tree keeps
//! [`MIN_TREE_HEIGHT`] rows. `ui/side.rs` draws the result, `app/mouse.rs` drags the dividers.

/// Rows the tree keeps while any section shows.
pub const MIN_TREE_HEIGHT: u16 = 3;
/// A section's smallest height: its divider and header.
pub const MIN_SECTION_HEIGHT: u16 = 2;

/// One section's wish: its dragged height and whether it is collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub height: u16,
    pub collapsed: bool,
}

impl Section {
    /// The rows the section asks for.
    fn want(self) -> u16 {
        if self.collapsed {
            MIN_SECTION_HEIGHT
        } else {
            self.height.max(MIN_SECTION_HEIGHT)
        }
    }
}

/// Splits a body of `body` rows between the tree and `sections` (top to bottom): the height of
/// each section (0: hidden) and the tree's rows (the rest, at least [`MIN_TREE_HEIGHT`] unless no
/// section shows). When the minimums don't all fit the topmost sections hide first; when the
/// wishes don't fit the topmost shrink first, down to their divider and header. Heights are
/// clamped for this body only: the wishes are not changed.
pub fn split(body: u16, sections: &[Section]) -> (u16, Vec<u16>) {
    let mut heights = vec![0; sections.len()];
    // Hide from the top until the minimums fit.
    let mut first = 0;
    while first < sections.len() && body < MIN_TREE_HEIGHT + MIN_SECTION_HEIGHT * (sections.len() - first) as u16 {
        first += 1;
    }
    if first == sections.len() {
        return (body, heights);
    }
    let mut avail = body - MIN_TREE_HEIGHT;
    let shown = &sections[first..];
    // The rows above each section's minimum, handed out bottom-up: lower sections keep their wish
    // while the upper ones give way.
    let mut spare = avail - MIN_SECTION_HEIGHT * shown.len() as u16;
    for (i, s) in shown.iter().enumerate().rev() {
        let extra = (s.want() - MIN_SECTION_HEIGHT).min(spare);
        heights[first + i] = MIN_SECTION_HEIGHT + extra;
        spare -= extra;
    }
    avail = heights.iter().sum();
    (body - avail, heights)
}

/// The height a divider drag stores for section `i`: `want` rows, clamped so the tree and the
/// other shown sections (at `heights`, from [`split`]) keep their rows — those above can shrink
/// to their minimum, those below keep what they have. Unchanged (`want`) when `i` is hidden.
pub fn dragged_height(body: u16, heights: &[u16], i: usize, want: u16) -> u16 {
    if heights.get(i).is_none_or(|h| *h == 0) {
        return want;
    }
    let below: u16 = heights[i + 1..].iter().sum();
    let above = MIN_SECTION_HEIGHT * heights[..i].iter().filter(|h| **h > 0).count() as u16;
    let most = body
        .saturating_sub(MIN_TREE_HEIGHT + below + above)
        .max(MIN_SECTION_HEIGHT);
    want.clamp(MIN_SECTION_HEIGHT, most)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(height: u16) -> Section {
        Section {
            height,
            collapsed: false,
        }
    }
    fn shut(height: u16) -> Section {
        Section {
            height,
            collapsed: true,
        }
    }

    #[test]
    fn one_section_splits_as_git_history_always_did() {
        assert_eq!(split(20, &[open(10)]), (10, vec![10]));
        assert_eq!(split(4, &[open(10)]), (4, vec![0]));
        assert_eq!(split(8, &[open(10)]), (3, vec![5]));
        assert_eq!(split(20, &[open(100)]), (3, vec![17]));
        assert_eq!(split(20, &[open(0)]), (18, vec![2]));
        assert_eq!(split(20, &[shut(9)]), (18, vec![2]));
        assert_eq!(split(3, &[shut(9)]), (3, vec![0]));
        assert_eq!(split(5, &[shut(9)]), (3, vec![2]));
    }

    #[test]
    fn two_sections_share_the_body() {
        assert_eq!(split(30, &[open(6), open(10)]), (14, vec![6, 10]));
        // The upper one gives way first.
        assert_eq!(split(18, &[open(6), open(10)]), (3, vec![5, 10]));
        assert_eq!(split(12, &[open(6), open(10)]), (3, vec![2, 7]));
        // Collapsed: header only.
        assert_eq!(split(30, &[shut(6), open(10)]), (18, vec![2, 10]));
        assert_eq!(split(30, &[open(6), shut(10)]), (22, vec![6, 2]));
        // Not both minimums: the topmost hides.
        assert_eq!(split(6, &[open(6), open(10)]), (3, vec![0, 3]));
        assert_eq!(split(7, &[open(6), open(10)]), (3, vec![2, 2]));
        assert_eq!(split(4, &[open(6), open(10)]), (4, vec![0, 0]));
    }

    #[test]
    fn a_drag_keeps_the_tree_and_the_other_sections() {
        let body = 30;
        let secs = [open(6), open(10)];
        let (_, h) = split(body, &secs);
        assert_eq!(dragged_height(body, &h, 0, 100), 30 - 3 - 10);
        assert_eq!(dragged_height(body, &h, 0, 0), 2);
        assert_eq!(dragged_height(body, &h, 1, 100), 30 - 3 - 2);
        assert_eq!(dragged_height(body, &h, 1, 8), 8);
        assert_eq!(dragged_height(body, &[0, 5], 0, 9), 9, "hidden: unchanged");
    }
}
