//! Pure queue-navigation and window-planning logic — no terminal IO, so the
//! focus state machine and overflow windowing can be unit-tested in isolation.

/// The most queued rows we ever render at once; beyond this the list windows
/// around the focused item with `…(+k more)` markers.
pub(super) const MAX_QUEUE_ROWS: usize = 6;

/// The number of chrome rows the queue table draws around its items: a header
/// bar (`┌─ N Queued ─┐`) on top and a bottom border (`└─┘`).
pub(super) const QUEUE_FRAME_ROWS: usize = 2;

/// Where keyboard focus sits: the bottom composer, or a 0-based queued item.
///
/// The queue is stacked *above* the composer (item 0 on top, the last item just
/// above the composer), so "up" walks toward index 0 and "down" walks back to
/// the composer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Focus {
    Composer,
    Item(usize),
}

impl Focus {
    /// Move focus up. From the composer, enter the list at the item nearest it
    /// (the last one); within the list, step toward the top, clamping at item 0.
    pub(super) fn up(self, len: usize) -> Focus {
        match self {
            Focus::Composer if len == 0 => Focus::Composer,
            Focus::Composer => Focus::Item(len - 1),
            Focus::Item(i) => Focus::Item(i.min(len.saturating_sub(1)).saturating_sub(1)),
        }
    }

    /// Move focus down. Within the list, step toward the bottom; stepping past
    /// the last item returns to the composer. The composer is the floor.
    pub(super) fn down(self, len: usize) -> Focus {
        match self {
            Focus::Composer => Focus::Composer,
            Focus::Item(i) => {
                let i = i.min(len.saturating_sub(1));
                if i + 1 >= len {
                    Focus::Composer
                } else {
                    Focus::Item(i + 1)
                }
            }
        }
    }

    /// Re-clamp focus after the queue length changes (e.g. a delete): a focus
    /// past the end snaps to the last item, and an empty queue falls back to the
    /// composer.
    pub(super) fn clamp(self, len: usize) -> Focus {
        match self {
            Focus::Composer => Focus::Composer,
            Focus::Item(_) if len == 0 => Focus::Composer,
            Focus::Item(i) => Focus::Item(i.min(len - 1)),
        }
    }

    /// The item index the visible window should center on. The composer anchors
    /// on the nearest items (the bottom of the list).
    pub(super) fn anchor(self, len: usize) -> usize {
        match self {
            Focus::Item(i) => i.min(len.saturating_sub(1)),
            Focus::Composer => len.saturating_sub(1),
        }
    }

    pub(super) fn item(self) -> Option<usize> {
        match self {
            Focus::Item(i) => Some(i),
            Focus::Composer => None,
        }
    }
}

/// One rendered row of the queue list: either a queued item (by index) or an
/// `…(+k more)` overflow marker standing in for `k` hidden items.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum QueueRow {
    Item(usize),
    More(usize),
}

/// Plan the queue rows to display for `len` items with the window centered on
/// `anchor`, showing at most `cap` rows. When the queue overflows `cap`, the
/// top and/or bottom slot becomes an `…(+k more)` marker; the anchored item is
/// always kept visible.
pub(super) fn plan_rows(len: usize, anchor: usize, cap: usize) -> Vec<QueueRow> {
    if len == 0 || cap == 0 {
        return Vec::new();
    }
    if len <= cap {
        return (0..len).map(QueueRow::Item).collect();
    }

    let mut start = anchor.saturating_sub(cap / 2);
    if start + cap > len {
        start = len - cap;
    }
    let mut rows: Vec<QueueRow> = (start..start + cap).map(QueueRow::Item).collect();

    let above = start;
    let below = len - (start + cap);
    // Replace the edge slots with markers, but never the slot holding the
    // anchor (so the focused item stays on screen).
    if above > 0 && start != anchor {
        rows[0] = QueueRow::More(above + 1);
    }
    let last = cap - 1;
    if below > 0 && start + last != anchor {
        rows[last] = QueueRow::More(below + 1);
    }
    rows
}

/// Plan the queue rows for the current terminal, degrading gracefully on short
/// screens: the list is capped both by [`MAX_QUEUE_ROWS`] and by how many rows
/// are free above the composer (always leaving at least one output row). `frame`
/// is the number of non-list chrome rows the table draws around the items (the
/// header bar + bottom border), reserved so they never push output off-screen.
/// When there's no room, the list collapses to nothing (composer only).
pub(super) fn queue_rows(
    len: usize,
    focus: Focus,
    rows: u16,
    cap: usize,
    frame: usize,
) -> Vec<QueueRow> {
    let max_list = (rows as usize).saturating_sub(2 + frame);
    let effective = cap.min(max_list);
    if effective == 0 {
        return Vec::new();
    }
    plan_rows(len, focus.anchor(len), effective)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_up_enters_list_at_nearest_item_then_clamps_at_top() {
        assert_eq!(Focus::Composer.up(3), Focus::Item(2));
        assert_eq!(Focus::Item(2).up(3), Focus::Item(1));
        assert_eq!(Focus::Item(0).up(3), Focus::Item(0));
        // An empty queue keeps focus on the composer.
        assert_eq!(Focus::Composer.up(0), Focus::Composer);
    }

    #[test]
    fn focus_down_walks_back_to_the_composer() {
        assert_eq!(Focus::Item(0).down(3), Focus::Item(1));
        assert_eq!(Focus::Item(2).down(3), Focus::Composer);
        assert_eq!(Focus::Composer.down(3), Focus::Composer);
    }

    #[test]
    fn focus_clamp_snaps_into_range_after_a_change() {
        assert_eq!(Focus::Item(5).clamp(3), Focus::Item(2));
        assert_eq!(Focus::Item(0).clamp(0), Focus::Composer);
        assert_eq!(Focus::Composer.clamp(0), Focus::Composer);
    }

    #[test]
    fn focus_anchor_centers_window_on_focus_or_bottom() {
        assert_eq!(Focus::Item(4).anchor(10), 4);
        assert_eq!(Focus::Composer.anchor(10), 9);
        assert_eq!(Focus::Composer.anchor(0), 0);
    }

    #[test]
    fn plan_rows_shows_everything_within_cap() {
        assert_eq!(
            plan_rows(3, 0, 6),
            vec![QueueRow::Item(0), QueueRow::Item(1), QueueRow::Item(2)]
        );
        assert!(plan_rows(0, 0, 6).is_empty());
    }

    #[test]
    fn plan_rows_windows_with_overflow_markers() {
        // Anchored at the top: only a bottom `…(+k more)` marker.
        assert_eq!(
            plan_rows(10, 0, 6),
            vec![
                QueueRow::Item(0),
                QueueRow::Item(1),
                QueueRow::Item(2),
                QueueRow::Item(3),
                QueueRow::Item(4),
                QueueRow::More(5),
            ]
        );
        // Anchored at the bottom: only a top marker.
        assert_eq!(
            plan_rows(10, 9, 6),
            vec![
                QueueRow::More(5),
                QueueRow::Item(5),
                QueueRow::Item(6),
                QueueRow::Item(7),
                QueueRow::Item(8),
                QueueRow::Item(9),
            ]
        );
        // Anchored in the middle: markers on both ends, focus stays visible.
        let mid = plan_rows(10, 5, 6);
        assert_eq!(mid.len(), 6);
        assert_eq!(mid.first(), Some(&QueueRow::More(3)));
        assert_eq!(mid.last(), Some(&QueueRow::More(3)));
        assert!(mid.contains(&QueueRow::Item(5)));
    }

    #[test]
    fn queue_rows_degrades_on_short_terminals() {
        let frame = QUEUE_FRAME_ROWS;
        // Roomy terminal shows every item.
        assert_eq!(queue_rows(3, Focus::Composer, 24, 6, frame).len(), 3);
        // A short terminal has no room for the framed list (composer only).
        assert!(queue_rows(5, Focus::Composer, 3, 6, frame).is_empty());
        // The row cap is honored even on a tall screen with a long queue.
        assert_eq!(queue_rows(20, Focus::Composer, 50, 6, frame).len(), 6);
        // The header/footer frame is reserved out of the list budget: a 9-row
        // terminal leaves 9 - 2 - 2 = 5 rows for items, not 7.
        assert_eq!(queue_rows(20, Focus::Composer, 9, 6, frame).len(), 5);
    }
}
