//! Owned Project Tree component.

mod model;
mod state;
mod view;

pub(super) use model::ProjectOrder;
pub(super) use state::{HorizontalDirection, State, VerticalDirection};
pub(super) use view::draw;

use super::{current_attention, snapshot_now_ms};
#[cfg(test)]
use crate::domain::Session;
use crate::presentation::app::Snapshot;
use model::{ProjectTreeKey, ProjectTreeRow};
use std::collections::HashSet;

fn rows<'a>(
    snap: &'a Snapshot,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
    project_order: &ProjectOrder,
) -> Vec<ProjectTreeRow<'a>> {
    let attention = current_attention(snap);
    model::rows_with_order(
        &snap.sessions,
        &attention,
        snapshot_now_ms(snap),
        collapsed,
        expanded_disclosures,
        project_order,
    )
}

pub(super) fn counts(snap: &Snapshot) -> (usize, usize) {
    model::tree_counts(snap)
}

#[cfg(test)]
pub(super) fn open_sessions(snap: &Snapshot) -> Vec<&Session> {
    model::sessions(snap)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
