//! Compact explanation for a selected Project Session's attention marker.

use crate::application::attention::{AttentionCondition, AttentionSeverity, AttentionSnapshot};
use crate::domain::Session;
use unicode_width::UnicodeWidthStr;

pub(super) struct Summary {
    full_value: String,
    compact_value: String,
    pub(super) severity: AttentionSeverity,
}

impl Summary {
    pub(super) fn value(&self, width: usize) -> &str {
        if UnicodeWidthStr::width(self.full_value.as_str()) <= width {
            &self.full_value
        } else {
            &self.compact_value
        }
    }
}

/// Summarize the highest-priority marker-producing condition and count the rest.
///
/// `AttentionSnapshot` already owns the confirmed condition order. This component
/// deliberately filters out neutral conditions instead of recreating that order.
pub(super) fn for_session(attention: &AttentionSnapshot, session: &Session) -> Option<Summary> {
    let marker_conditions = attention
        .conditions_for(session)
        .iter()
        .filter(|condition| condition.marker_severity().is_some())
        .collect::<Vec<_>>();
    let primary = marker_conditions.first()?;
    let severity = primary.marker_severity()?;
    let (mut full_value, mut compact_value) = evidence(primary)?;
    let additional = marker_conditions.len().saturating_sub(1);
    if additional > 0 {
        full_value.push_str(&format!(" · +{additional}"));
        compact_value.push_str(&format!(" +{additional}"));
    }

    Some(Summary {
        full_value,
        compact_value,
        severity,
    })
}

fn evidence(condition: &AttentionCondition) -> Option<(String, String)> {
    match condition {
        AttentionCondition::ContextAlert {
            severity,
            percentage,
            ..
        } if percentage.is_finite() => {
            let label = match severity {
                AttentionSeverity::Critical => "Critical Context",
                AttentionSeverity::Warning => "Warning Context",
            };
            let compact_label = match severity {
                AttentionSeverity::Critical => "Crit Ctx",
                AttentionSeverity::Warning => "Warn Ctx",
            };
            Some((
                format!("{label} · {percentage:.0}%"),
                format!("{compact_label} · {percentage:.0}%"),
            ))
        }
        AttentionCondition::SharedProjectRisk {
            contributing_sessions,
            ..
        } if *contributing_sessions >= 2 => Some((
            format!("Shared Project Risk · {contributing_sessions} Sessions"),
            format!("Project Risk · {contributing_sessions}S"),
        )),
        AttentionCondition::ContextAlert { .. }
        | AttentionCondition::SharedProjectRisk { .. }
        | AttentionCondition::Idle { .. } => None,
    }
}
