//! The single source of truth for the pinned bottom area's layout.
//!
//! Every repaint builds a [`PinnedPlan`]: the ordered pinned sections, each
//! carrying the exact lines it paints. Both the reserved-height computation
//! (which decides where the scroll region ends) and the paint walk (which
//! addresses each pinned row) derive from the same plan, so they can never
//! disagree — adding a pinned section is one entry here plus one build step in
//! [`Live::pinned_plan`], not a height sum *and* a row walk to keep in sync.
//!
//! [`Live::pinned_plan`]: super::Live::pinned_plan

/// The pinned sections, top to bottom. The variants document the layout order;
/// the plan carries whichever are present.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SectionKind {
    /// Blank breathing room directly under the scroll region.
    Spacer,
    /// The running fleet's lanes block (only while a fleet is on screen).
    Fleet,
    /// The in-place compression-savings line.
    Compression,
    /// The context-usage meters.
    Status,
    /// The faint full-width rule above the input area.
    Divider,
    /// The framed queue table (header + rows + footer).
    Queue,
    /// The frameless composer box, pinned to the bottom rows.
    Composer,
}

pub(super) struct Section {
    /// Which section this is — the paint walk doesn't need it (order is the
    /// plan's), but tests and future targeted repaints select by it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) kind: SectionKind,
    pub(super) lines: Vec<String>,
}

impl Section {
    pub(super) fn new(kind: SectionKind, lines: Vec<String>) -> Self {
        Self { kind, lines }
    }

    /// A section of `n` blank rows (painted as cleared lines).
    pub(super) fn blank(kind: SectionKind, n: usize) -> Self {
        Self {
            kind,
            lines: vec![String::new(); n],
        }
    }
}

/// The full pinned area for one paint: ordered sections, ready to walk.
pub(super) struct PinnedPlan {
    pub(super) sections: Vec<Section>,
}

impl PinnedPlan {
    /// Total rows the pinned area reserves below the scroll region.
    pub(super) fn rows(&self) -> u16 {
        self.sections.iter().map(|s| s.lines.len()).sum::<usize>() as u16
    }

    /// The scroll region's bottom row given this plan: everything above the
    /// reserved rows, but always at least one output row.
    pub(super) fn region_bottom(&self, term_rows: u16) -> u16 {
        term_rows.saturating_sub(self.rows()).max(1)
    }

    /// The lines of a section, for tests and targeted inspection.
    #[cfg(test)]
    pub(super) fn section(&self, kind: SectionKind) -> Option<&Section> {
        self.sections.iter().find(|s| s.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(sections: Vec<Section>) -> PinnedPlan {
        PinnedPlan { sections }
    }

    #[test]
    fn rows_is_the_sum_of_every_section_line() {
        let p = plan(vec![
            Section::blank(SectionKind::Spacer, 1),
            Section::new(SectionKind::Status, vec!["a".into(), "b".into()]),
            Section::new(SectionKind::Divider, vec!["─".into()]),
            Section::new(SectionKind::Composer, vec!["> ".into()]),
        ]);
        assert_eq!(p.rows(), 5);
        assert_eq!(p.region_bottom(24), 19);
    }

    #[test]
    fn region_bottom_never_reaches_zero() {
        let p = plan(vec![Section::new(
            SectionKind::Composer,
            vec![String::new(); 30],
        )]);
        assert_eq!(p.region_bottom(24), 1);
        assert_eq!(p.region_bottom(0), 1);
    }
}
