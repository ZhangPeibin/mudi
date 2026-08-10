//! Terminal-safe, shape-distinct Session management provenance marks.

use super::theme::Theme;
use crate::domain::SessionProvenance;
use ratatui::style::Style;

pub(super) const MANAGED: &str = "▦";
pub(super) const OBSERVED: &str = "⌁";

pub(super) fn icon(provenance: SessionProvenance) -> &'static str {
    match provenance {
        SessionProvenance::Managed => MANAGED,
        SessionProvenance::Observed => OBSERVED,
    }
}

pub(super) fn style(provenance: SessionProvenance, theme: Theme, base: Style) -> Style {
    base.fg(match provenance {
        SessionProvenance::Managed => theme.accent,
        SessionProvenance::Observed => theme.muted,
    })
}
