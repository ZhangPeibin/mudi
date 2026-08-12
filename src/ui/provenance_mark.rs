//! Compact Project Tree suffix for externally observed Sessions.

use super::theme::Theme;
use crate::domain::SessionProvenance;
use ratatui::style::Style;

/// Keep both variants two cells wide so operational evidence stays aligned.
pub(super) fn suffix(provenance: SessionProvenance) -> &'static str {
    match provenance {
        SessionProvenance::Managed => "  ",
        SessionProvenance::Observed => " ↗",
    }
}

pub(super) fn style(theme: Theme, base: Style) -> Style {
    base.fg(theme.muted)
}
