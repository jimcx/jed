---
id: sidebar_scrollbars
name: Scroll bars in the side bar's lists
summary: the one-column scroll bar on the explorer, Search results and Source Control lists
status: done
priority: 0
dependencies: []
---

  - `scrollbar::Scrollable` (`scroll()`, `set_top`) implemented by `view::TextScroll`, `DiffTab`, `Explorer` (`row_count`), `grouped::GroupedScroll` (`ListState::scrollable`: Search results, Problems, Locations) and `scm::ScmScroll`; `layout::ScrollTarget::{Group, Side(SidePanel), Panel(PanelTab)}` keys `Hit::ScrollBar`, `Drag::ScrollBar { target, grab }`, `ui.scrollbar_hover` and `HitMap::scrollbar(target)`; `App::with_scrollable(target, f)` (`None` when that list isn't shown: a drag whose list goes away scrolls nothing); drawing via `ui::scrollbar::take_column` / `render`. A new list gets a bar by implementing `Scrollable`, drawing with `take_column`, reporting under a `ScrollTarget` and adding an arm in `with_scrollable` (CLAUDE.md's Scroll bars convention)
  - The list column is always reserved; counts, git letters and the `+` / `−` buttons sit left of it; the Search query / toggles, the branch line and the panel's tab strip keep the full width; no bar on empty-state messages or task tabs; the wheel over a bar scrolls its list
  - Deferred: the hover colour can stay on after switching side views until the pointer moves
  - Still to check by hand: the slider colours over `sideBar` and `panel`; the blank column between counts / letters and the bar; dragging the explorer thumb in a large tree
