//! Handoff Artifact validation and owner-only publication.

use anyhow::{anyhow, Result};

const REQUIRED_HEADINGS: [&str; 10] = [
    "# Handoff",
    "## Objective",
    "## Current State",
    "## Decisions",
    "## Completed",
    "## Remaining",
    "## Files Changed",
    "## Verification",
    "## Risks And Blockers",
    "## Next Action",
];

pub(crate) fn validate(candidate: &str) -> Result<()> {
    if candidate.trim().is_empty() {
        return Err(anyhow!("Handoff Artifact is empty"));
    }
    if candidate.contains("-----BEGIN PRIVATE KEY-----")
        || candidate.contains("-----BEGIN RSA PRIVATE KEY-----")
        || candidate.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
    {
        return Err(anyhow!("Handoff Artifact contains secret material"));
    }
    let lines = candidate.lines().collect::<Vec<_>>();
    if contains_heading(&lines, "transcript") {
        return Err(anyhow!("Handoff Artifact contains a Transcript section"));
    }
    if contains_heading(&lines, "tool output") {
        return Err(anyhow!("Handoff Artifact contains a Tool Output section"));
    }
    let required_headings = required_headings();
    let contract_headings = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("# ") || line.starts_with("## "))
        .collect::<Vec<_>>();
    if contract_headings.as_slice() != required_headings {
        return Err(anyhow!(
            "Handoff Artifact headings do not match the contract"
        ));
    }
    let mut positions = Vec::with_capacity(required_headings.len());
    let mut search_from = 0;
    for heading in required_headings {
        let relative = lines[search_from..]
            .iter()
            .position(|line| *line == heading)
            .ok_or_else(|| anyhow!("Handoff Artifact is missing {heading:?}"))?;
        let position = search_from + relative;
        positions.push(position);
        search_from = position + 1;
    }
    for heading_index in [1, 2, 5, 9] {
        let start = positions[heading_index] + 1;
        let end = positions
            .get(heading_index + 1)
            .copied()
            .unwrap_or(lines.len());
        if !lines[start..end].iter().any(|line| !line.trim().is_empty()) {
            return Err(anyhow!(
                "Handoff Artifact section {:?} is empty",
                required_headings[heading_index]
            ));
        }
    }
    Ok(())
}

pub(crate) fn required_headings() -> [&'static str; 10] {
    REQUIRED_HEADINGS
}

fn contains_heading(lines: &[&str], needle: &str) -> bool {
    lines.iter().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#')
            && trimmed
                .trim_start_matches('#')
                .trim()
                .to_ascii_lowercase()
                .contains(needle)
    })
}
