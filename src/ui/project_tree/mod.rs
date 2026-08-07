//! Owned Project Tree component.

mod model;
mod state;
mod view;

pub(super) use state::{HorizontalDirection, State, VerticalDirection};
pub(super) use view::draw;

use super::{current_attention, snapshot_now_ms};
use crate::app::Snapshot;
use crate::domain::Session;
use model::{ProjectTreeKey, ProjectTreeRow};
use std::collections::HashSet;

fn rows<'a>(
    snap: &'a Snapshot,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) -> Vec<ProjectTreeRow<'a>> {
    let attention = current_attention(snap);
    model::rows(
        &snap.sessions,
        &attention,
        snapshot_now_ms(snap),
        collapsed,
        expanded_disclosures,
    )
}

pub(super) fn counts(snap: &Snapshot) -> (usize, usize) {
    model::tree_counts(snap)
}

pub(super) fn open_sessions(snap: &Snapshot) -> Vec<&Session> {
    model::sessions(snap)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
